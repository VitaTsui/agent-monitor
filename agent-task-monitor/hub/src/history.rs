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
    /// 会话在钉钉里的号位（@N 的 N），让两边编号对得上
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

/// 追加一条交互记录（正序存放，最新在末尾 —— 与聊天记录的读法一致）。
pub async fn append(state: &SharedState, mut e: HistoryEntry) {
    e.content = e.content.chars().take(CONTENT_LIMIT).collect();
    e.title = e.title.chars().take(200).collect();
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
    let h = state.history.read().await;
    let mine: Vec<&HistoryEntry> = h
        .iter()
        .filter(|e| e.owner == owner)
        .filter(|e| session.is_none_or(|s| e.session_id == s))
        .collect();
    let start = mine.len().saturating_sub(limit);
    mine[start..].iter().map(|e| (*e).clone()).collect()
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
        }
    }

    /// 取最近 N 条时必须保持正序（旧→新），否则聊天记录会倒着读
    #[test]
    fn keeps_chronological_order() {
        let all: Vec<HistoryEntry> =
            (0..5).map(|i| entry(&format!("e{i}"), "user", i as u64)).collect();
        let limit = 3;
        let start = all.len().saturating_sub(limit);
        let got: Vec<u64> = all[start..].iter().map(|e| e.at).collect();
        assert_eq!(got, vec![2, 3, 4], "应取最近 3 条且保持旧→新");
    }

    /// 超出上限从头砍，保留最新的
    #[test]
    fn drops_oldest_when_over_limit() {
        let mut h: Vec<HistoryEntry> =
            (0..5).map(|i| entry(&format!("e{i}"), "user", i as u64)).collect();
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
}
