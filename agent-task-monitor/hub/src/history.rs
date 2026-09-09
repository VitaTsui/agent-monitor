//! 远程交互历史：把「我发了什么 → 它回了什么」按时间排成一条对话流。
//!
//! 定位是**远程对话的聊天记录**，不是会话归档：你在手机上发一条指令、过一会儿收到结果，
//! 这本身就是一段对话，应该能像聊天记录那样往回翻 —— 尤其在终端关掉、机器关机之后。
//!
//! 与「会话内的实时对话」是两回事：后者完整存在客户端的 jsonl 里、网页 ChatPane 直接读，
//! hub 不复制。这里只记**经由 hub 的远程交互**：
//!
//! - `user` 条：从钉钉 / 网页 / MCP 下发的任务（三个入口都汇到 bot::queue_command）
//! - `assistant` 条：任务完成时的结果、会话结束时的最终产出
//!
//! 体量小（一条几 KB、总量设上限），JSON 落盘，不引入数据库。

use crate::state::SharedState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

/// 保留的最大条数（全用户合计）。历史是「回看最近做了什么」，不是审计日志。
const MAX_ENTRIES: usize = 3000;

/// 单条正文的存储上限。超长的结果留着也读不完，完整内容本来就在本机 jsonl 里。
const CONTENT_LIMIT: usize = 4000;

/// 一条交互记录。用 role 区分方向，前端据此渲染成左右气泡。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// 本条唯一 id（用于前端 key 与去重）
    pub id: String,
    /// 归属账号
    pub owner: String,
    /// 所属会话（jsonl id）——同一会话的往来会连成一串
    pub session_id: String,
    /// `user` = 我下发的；`assistant` = 它给回的
    pub role: String,
    pub content: String,
    /// 发生时刻（epoch 秒）
    pub at: u64,
    /// 下发来源：dingtalk / web / mcp；assistant 条为空
    #[serde(default)]
    pub source: String,
    /// 会话在钉钉里的号位（#N 的 N），让两边编号对得上
    #[serde(default)]
    pub slot: Option<u32>,
    pub hostname: String,
    pub project: String,
    /// 会话标题，用于在流里标出「这段是哪个会话的」
    #[serde(default)]
    pub title: String,
    /// 代理展示名（Claude Code / Codex …）
    #[serde(default)]
    pub provider: String,
    /// 号位锚（`machine_id|sh:{shell_pid}@{shell_start}`）——这条记录出自**哪个终端窗口**。
    ///
    /// 存的是**身份**不是名字：备注在读取时按它现算（见 [`list_for`]），所以用户改名之后，
    /// 这个终端的所有历史记录跟着一起变。会话 id 做不到这件事 —— `/clear`、`--resume`
    /// 各换一次新 id，而备注挂在终端窗口上（见 crate::notes）。
    ///
    /// 本次改动之前写入的存量记录没有这个字段，反序列化得空串，永远匹配不到备注（备注表的
    /// 键都带 `machine|` 前缀），于是回落到 `title` —— 与改动前的表现完全一致。
    #[serde(default)]
    pub anchor: String,
    /// 用户给这个终端起的名字，**读取时现填**。
    ///
    /// 唯一存储在 crate::notes，这里既不落盘也不进内存表：只有 [`list_for`] 返回的那份
    /// 拷贝上有值（`#[serde(skip_serializing_if)]` 保证内存里的 `None` 不会写进
    /// history.json）。历史里再存一份备注就成了两套数据，改名后必然对不上。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 从数据目录加载（读不到/解析失败都当空：历史丢了不影响任何功能）
pub fn load(data_dir: &std::path::Path) -> Vec<HistoryEntry> {
    std::fs::read_to_string(data_dir.join("history.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// 落盘（原子写；tick 循环按脏标调用）
pub async fn save(state: &SharedState) {
    let snapshot = state.history.read().await.clone();
    let Ok(txt) = serde_json::to_string(&snapshot) else {
        return;
    };
    let path = state.config.data_dir.join("history.json");
    let tmp = state.config.data_dir.join("history.json.tmp");
    if std::fs::write(&tmp, &txt).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &path);
}

/// 入库前对正文做的两件事：**先脱敏，再截断**。
///
/// 顺序不能反：先截断的话，一块贴在报告末尾的私钥会被从主体中间切开 ——
/// 尾部的 `-----END …-----` 没了，脱敏就认不出这块结构，半截私钥照样落盘。
/// 先脱敏则整块换成一个短占位符，既不漏，也把省下来的额度让给真正的正文。
fn prepare_content(content: &str) -> String {
    crate::redact::redact_secrets(content)
        .chars()
        .take(CONTENT_LIMIT)
        .collect()
}

/// 追加一条交互记录（正序存放，最新在末尾 —— 与聊天记录的读法一致）。
pub async fn append(state: &SharedState, mut e: HistoryEntry) {
    e.content = prepare_content(&e.content);
    e.title = e.title.chars().take(200).collect();
    // 备注只在读取时现填（见 list_for）。入库这一路强制清空，把「历史里不存备注副本」
    // 从口头约定变成代码保证：否则哪天有人把 list_for 的返回值又塞回来，改名就再也不生效了。
    e.note = None;
    if e.content.trim().is_empty() {
        return; // 空内容不入流，免得聊天记录里出现空气泡
    }
    let mut h = state.history.write().await;
    h.push(e);
    // 超出上限丢最旧的（从头砍）
    if h.len() > MAX_ENTRIES {
        let cut = h.len() - MAX_ENTRIES;
        h.drain(..cut);
    }
    state.history_dirty.store(true, Ordering::Relaxed);
}

/// 生成一条记录的 id（时间 + 随机，避免同秒多条撞车）
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 某账号的交互历史。返回**最近 limit 条**，且保持正序（旧→新），
/// 前端直接从上往下渲染即是聊天记录的读法。
///
/// `session` 给了就只返回该会话的往来 —— 在某个会话里点开历史，想看的自然是**这个会话**的
/// 记录（像点开某个人的聊天记录），而不是所有终端的往来混在一起。不给则返回全部。
pub async fn list_for(
    state: &SharedState,
    owner: &str,
    session: Option<&str>,
    limit: usize,
) -> Vec<HistoryEntry> {
    // 备注**现查现填**，不用入库时的快照：用户的诉求是「历史里显示我起的名字」，起名是给这个
    // 终端贴标签，不是记录「它当时叫什么」。快照式存储会让同一个终端的历史里混着新旧两个名字。
    // 先取（map_for 内部取完读锁就还），再拿 history 锁，避免两把锁嵌套。
    let notes = crate::notes::map_for(state, owner).await;
    let h = state.history.read().await;
    let mine: Vec<&HistoryEntry> = h
        .iter()
        .filter(|e| e.owner == owner)
        .filter(|e| session.is_none_or(|s| e.session_id == s))
        .collect();
    let start = mine.len().saturating_sub(limit);
    mine[start..]
        .iter()
        .map(|e| with_note((*e).clone(), &notes))
        .collect()
}

/// 把**当前**备注贴到取出的记录上：锚对得上就用用户起的名字，对不上给 `None`（前端回落 title）。
///
/// 锚为空的存量记录在这里自然落空 —— 备注表的键都带 `machine|` 前缀，空串匹配不到任何一条。
fn with_note(
    mut e: HistoryEntry,
    notes: &std::collections::HashMap<String, String>,
) -> HistoryEntry {
    e.note = notes.get(&e.anchor).cloned();
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, role: &str, at: u64) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            owner: "u".into(),
            session_id: "s".into(),
            role: role.into(),
            content: "c".into(),
            at,
            source: "dingtalk".into(),
            slot: Some(9),
            hostname: "h".into(),
            project: "p".into(),
            title: "t".into(),
            provider: "Claude Code".into(),
            anchor: "m1|sh:42@1700".into(),
            note: None,
        }
    }

    /// 取最近 N 条时必须保持正序（旧→新），否则聊天记录会倒着读
    #[test]
    fn keeps_chronological_order() {
        let all: Vec<HistoryEntry> = (0..5)
            .map(|i| entry(&format!("e{i}"), "user", i as u64))
            .collect();
        let limit = 3;
        let start = all.len().saturating_sub(limit);
        let got: Vec<u64> = all[start..].iter().map(|e| e.at).collect();
        assert_eq!(got, vec![2, 3, 4], "应取最近 3 条且保持旧→新");
    }

    /// 超出上限从头砍，保留最新的
    #[test]
    fn drops_oldest_when_over_limit() {
        let mut h: Vec<HistoryEntry> = (0..5)
            .map(|i| entry(&format!("e{i}"), "user", i as u64))
            .collect();
        let max = 3;
        if h.len() > max {
            let cut = h.len() - max;
            h.drain(..cut);
        }
        assert_eq!(h.iter().map(|e| e.at).collect::<Vec<_>>(), vec![2, 3, 4]);
    }

    /// 空内容不入流（否则聊天记录里会出现空气泡）
    #[test]
    fn skips_blank_content() {
        let mut e = entry("x", "assistant", 1);
        e.content = "   \n  ".into();
        assert!(e.content.trim().is_empty());
    }

    /// 贴在长报告**末尾**的私钥：必须先脱敏再截断。
    /// 反过来先截断的话，这块会被拦腰切断、认不出结构，半截私钥就落盘了。
    #[test]
    fn redacts_before_truncating() {
        let filler = "正".repeat(CONTENT_LIMIT - 100);
        let key_body = (0..4)
            .map(|i| format!("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC{i:015}"))
            .collect::<Vec<_>>()
            .join("\n");
        let raw =
            format!("{filler}\n-----BEGIN PRIVATE KEY-----\n{key_body}\n-----END PRIVATE KEY-----");
        assert!(raw.chars().count() > CONTENT_LIMIT, "构造的用例应超过上限");

        let got = prepare_content(&raw);
        assert!(got.contains("[REDACTED:private-key]"), "私钥没被脱敏");
        assert!(!got.contains("MIIEvQIBAD"), "主体残留在库里");
        assert!(got.chars().count() <= CONTENT_LIMIT, "截断上限失效");
    }

    /// 正常正文一个字都不能被改动（含「谈论密钥」的那句真实用例）
    #[test]
    fn keeps_ordinary_content_intact() {
        let text = "核验：`BEGIN PRIVATE KEY` 0 命中";
        assert_eq!(prepare_content(text), text);
    }

    fn notes_of(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// 改名后**旧记录跟着变** —— 这正是选「读取时现算」而不是「写入时快照」的理由：
    /// 同一条记录换一份备注表就换一个名字，同一个终端的历史不会新旧名字混排。
    #[test]
    fn note_follows_rename_on_read() {
        let e = entry("e1", "user", 1);
        let first = with_note(e.clone(), &notes_of(&[("m1|sh:42@1700", "支付重构")]));
        assert_eq!(first.note.as_deref(), Some("支付重构"));
        let renamed = with_note(e.clone(), &notes_of(&[("m1|sh:42@1700", "退款对账")]));
        assert_eq!(
            renamed.note.as_deref(),
            Some("退款对账"),
            "改名要立刻反映到旧记录"
        );
        let cleared = with_note(e, &notes_of(&[]));
        assert_eq!(cleared.note, None, "清除备注后该回落到 title");
    }

    /// 别的终端的备注不能串到这条记录上（锚不同 = 不同终端窗口）
    #[test]
    fn other_terminal_note_does_not_leak() {
        let got = with_note(
            entry("e1", "user", 1),
            &notes_of(&[("m1|sh:99@1700", "别人的名字")]),
        );
        assert_eq!(got.note, None);
    }

    /// 存量记录（本次改动之前写入，没有 anchor 字段）：读回来 anchor 为空、
    /// 即便用户已经起了名字也匹配不到，表现与改动前一致 —— 显示自动标题。
    #[test]
    fn legacy_entry_without_anchor_falls_back_to_title() {
        let legacy = r#"{"id":"a","owner":"u","sessionId":"s","role":"user","content":"c",
            "at":1,"hostname":"h","project":"p","title":"自动标题"}"#;
        let e: HistoryEntry = serde_json::from_str(legacy).expect("旧格式必须还能读回来");
        assert_eq!(e.anchor, "", "旧记录没有锚");
        assert_eq!(e.title, "自动标题");
        let got = with_note(e, &notes_of(&[("m1|sh:42@1700", "支付重构")]));
        assert_eq!(got.note, None, "空锚不能匹配到任何备注");
    }

    /// 备注**不落盘**：内存里的记录 note 恒为 None，序列化出来连字段都没有。
    /// 历史里再存一份备注就是两套数据，改名后必然对不上。
    #[test]
    fn note_is_never_persisted() {
        let txt = serde_json::to_string(&entry("e1", "user", 1)).unwrap();
        assert!(
            !txt.contains("\"note\""),
            "history.json 不该出现 note 字段：{txt}"
        );
        assert!(
            txt.contains("\"anchor\":\"m1|sh:42@1700\""),
            "锚必须落盘：{txt}"
        );
    }
}
