//! 会话历史：每个会话**结束时**留一条最终产出，供事后回看。
//!
//! 为什么只记结束时的一条：会话进行中的完整对话本来就在客户端的 jsonl 里（claude 自己管），
//! hub 再存一份是重复数据源、还要处理同步。而「人不在电脑前」时真正需要的是
//! ——「我派出去的那些活，最后都出了什么结果」，终端关掉、机器关机之后也查得到。
//!
//! 体量很小（一条几 KB、总量设上限），所以直接 JSON 落盘，不引入数据库。

use crate::state::SharedState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

/// 保留的最大条数（全用户合计）。超出丢最旧的 —— 历史是「回看最近做了什么」，
/// 不是审计日志；真要长期留存应该另做导出。
const MAX_RECORDS: usize = 1000;

/// 单条产出正文的存储上限。超长的会话结果没必要整段留着，回看时看不完，
/// 完整内容本来就在本机 jsonl 里。
const RESULT_LIMIT: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// 会话 id（jsonl 文件名），同一会话重复结束时用它去重
    pub id: String,
    /// 归属账号
    pub owner: String,
    pub hostname: String,
    /// 项目短名
    pub project: String,
    /// 会话标题（首个用户提示词）
    pub title: String,
    /// 最后一条真实用户提示词 —— 「这个会话最后在干什么」
    #[serde(default)]
    pub prompt: String,
    /// 最终产出（末条 assistant 消息，已截断）
    pub result: String,
    /// 代理展示名（Claude Code / Codex …）
    #[serde(default)]
    pub provider: String,
    /// 会话开始时间（ISO8601，来自扫描）
    #[serde(default)]
    pub started_at: Option<String>,
    /// 记录写入时刻（epoch 秒）= 判定结束的时刻
    pub ended_at: u64,
    /// 结束时该会话在钉钉里的号位（`@9` 的 9）。留着是为了让网页历史和钉钉里看到的编号
    /// 对得上 —— 「哦，这是我 9 号那个终端做的活」。号位绑终端锚，终端关掉超过保留期才回收，
    /// 所以多数情况仍查得到；查不到就是 None。
    #[serde(default)]
    pub slot: Option<u32>,
}

/// 从数据目录加载（读不到/解析失败都当空：历史丢了不影响任何功能）
pub fn load(data_dir: &std::path::Path) -> Vec<SessionRecord> {
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

/// 记一条会话结束。
///
/// **同一会话只留最新的一条**：配对抖动可能让同一个会话被判「结束」不止一次
/// （消失→重现→再消失），重复追加会把历史刷满同一个会话。
pub async fn record(state: &SharedState, mut rec: SessionRecord) {
    rec.result = rec.result.chars().take(RESULT_LIMIT).collect();
    rec.title = rec.title.chars().take(200).collect();
    rec.prompt = rec.prompt.chars().take(500).collect();
    let mut h = state.history.write().await;
    h.retain(|r| r.id != rec.id);
    h.push(rec);
    // 按结束时间倒序，最新在前；超出上限丢最旧
    h.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
    h.truncate(MAX_RECORDS);
    state.history_dirty.store(true, Ordering::Relaxed);
}

/// 某账号的历史（最新在前）
pub async fn list_for(state: &SharedState, owner: &str, limit: usize) -> Vec<SessionRecord> {
    state
        .history
        .read()
        .await
        .iter()
        .filter(|r| r.owner == owner)
        .take(limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ended: u64) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            owner: "u".into(),
            hostname: "h".into(),
            project: "p".into(),
            title: "t".into(),
            prompt: String::new(),
            result: "r".into(),
            provider: "Claude Code".into(),
            started_at: None,
            ended_at: ended,
            slot: Some(9),
        }
    }

    /// 同一会话被判结束多次时只留最新一条，不能把历史刷满同一个会话
    #[test]
    fn same_session_deduped() {
        let mut h = vec![rec("a", 100), rec("b", 200)];
        let new = rec("a", 300);
        h.retain(|r| r.id != new.id);
        h.push(new);
        h.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].id, "a");
        assert_eq!(h[0].ended_at, 300);
    }

    /// 超出上限丢最旧的
    #[test]
    fn truncates_oldest() {
        let mut h: Vec<SessionRecord> = (0..5).map(|i| rec(&format!("s{i}"), i as u64)).collect();
        h.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
        h.truncate(3);
        assert_eq!(h.iter().map(|r| r.ended_at).collect::<Vec<_>>(), vec![4, 3, 2]);
    }
}
