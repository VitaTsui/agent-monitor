//! 会话备注：给会话起一个自己认得出的名字，替代自动生成的标题。
//!
//! **备注挂在号位锚上，不挂会话 id。** 理由与号位（见 crate::slots）完全一样，也必须一样：
//! 会话 id 就是那个 jsonl 文件名，`/clear` 一下、`--resume` 一次就换新的。备注若挂在它
//! 上面，用户刚起的名字会在下一次 `/clear` 时凭空消失 —— 而在用户眼里，那还是同一个终端
//! 窗口里的同一件事。号位锚 `(machine_id, shell_pid, shell_start)` 认的是**终端窗口**，
//! 跨 `/clear`、`--resume`、agent 重启都不变，正是「这条会话」在用户心里的身份。
//!
//! 反过来也成立：终端窗口关掉再开，shell pid 变了 → 锚变了 → 那是一个新会话，不该继承
//! 上一个的名字。`shell_start` 还挡住了 Windows 的 pid 重用，不会张冠李戴。
//!
//! **备注是用户私有的**：存储按 `用户名 → (锚 → 备注)` 分层，与号位表同构。协助共享的设备
//! 上，主人和访客各起各的名字，互不覆盖 —— 名字是给自己看的，没有「谁能改谁的」问题。
//!
//! 存储沿用 hub 既有的那一套：常驻内存 + JSON 原子写（与 crate::slots / crate::history 同
//! 形），不引入数据库。**但落盘时机不同**：号位与历史是高频自动产生的（每轮上报都在续期），
//! 所以攒着由 tick 循环 ~60s 刷一次；备注是低频的人工动作，用户改完名字就可能去重启 hub，
//! 攒 60s 等于「改了个名字，重启一下就没了」。所以这里每次改动直接落盘。

use crate::state::{now_secs, SharedState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 备注长度上限（**字符**数，不是字节 —— 中文一个字算一个）。
///
/// 备注是列表里的一行标题，不是描述字段。100 字已经远超一行能显示的量，
/// 再长只会被前端截断显示，用户还以为存坏了。超限直接报错而不是静默截断：
/// 悄悄砍掉一半正文，比明说「太长了」难受得多。
pub const NOTE_MAX_CHARS: usize = 100;

/// 单个用户最多保留多少条备注，超出按「最久没改过的先丢」。
///
/// 备注不会随会话消失而清理（见模块头：终端窗口还在，只是 agent 没跑），所以需要一个
/// 上界防止无限增长。200 对「同时开着的终端窗口」来说是几十倍余量 —— 号池上限本身才 30。
const MAX_NOTES_PER_USER: usize = 200;

/// 一条备注
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNote {
    /// 用户起的名字（已归一化：无控制字符、无首尾空白）
    pub text: String,
    /// 最后一次修改时刻（epoch 秒），超额淘汰按它排
    pub at: u64,
}

/// 一个用户的备注表：号位锚 → 备注
pub type NoteTable = HashMap<String, SessionNote>;

/// 全量备注：用户名 → 备注表（落盘于 data_dir/session_notes.json）
pub type NoteStore = HashMap<String, NoteTable>;

/// 归一化并校验用户输入。
///
/// - 控制字符（含换行、制表符）一律换成空格：备注渲染成列表里的一行，混进换行会把布局撑坏，
///   而多行文本本来也不是「名字」。这是规整而非丢弃，用户看得见结果。
/// - 连续空白折成一个空格、首尾空白去掉。
/// - 空串（或全是空白）= **清除备注**，返回空串交给调用方删条目。
/// - 超过 [`NOTE_MAX_CHARS`] 直接报错，不截断。
pub fn normalize(raw: &str) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        let c = if ch.is_control() { ' ' } else { ch };
        if c.is_whitespace() {
            // 前面还没有实际内容就不记空格，等于顺手去掉了首部空白
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    // pending_space 收尾时不落笔 = 去掉尾部空白
    let n = out.chars().count();
    if n > NOTE_MAX_CHARS {
        return Err(format!("备注最长 {NOTE_MAX_CHARS} 个字，当前 {n} 个"));
    }
    Ok(out)
}

/// 从数据目录加载（读不到/解析失败都当空表：备注丢了不影响任何功能，会话照常可用）
pub fn load(data_dir: &std::path::Path) -> NoteStore {
    std::fs::read_to_string(data_dir.join("session_notes.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// 落盘（原子写）。调用方必须**持着 notes 的写锁**再调，写才不会互相盖。
fn write_to(data_dir: &std::path::Path, store: &NoteStore) {
    let Ok(txt) = serde_json::to_string(store) else {
        return;
    };
    let path = data_dir.join("session_notes.json");
    let tmp = data_dir.join("session_notes.json.tmp");
    if std::fs::write(&tmp, &txt).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &path);
}

/// 超额淘汰：最久没改过的先丢（同秒则按锚排，保证结果确定）
fn evict_over_cap(table: &mut NoteTable) {
    while table.len() > MAX_NOTES_PER_USER {
        let Some(oldest) = table
            .iter()
            .min_by(|a, b| a.1.at.cmp(&b.1.at).then(a.0.cmp(b.0)))
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        table.remove(&oldest);
    }
}

/// 写入备注。`text` 必须已经过 [`normalize`]；空串 = 清除。
/// 返回落库后的备注（清除时为 `None`）。
pub async fn set(state: &SharedState, user: &str, anchor: &str, text: &str) -> Option<String> {
    let mut store = state.notes.write().await;
    let table = store.entry(user.to_string()).or_default();
    let out = if text.is_empty() {
        table.remove(anchor);
        None
    } else {
        table.insert(
            anchor.to_string(),
            SessionNote {
                text: text.to_string(),
                at: now_secs(),
            },
        );
        evict_over_cap(table);
        Some(text.to_string())
    };
    // 空表不留在文件里，免得用户清完备注还占一个空对象
    if table.is_empty() {
        store.remove(user);
    }
    write_to(&state.config.data_dir, &store);
    out
}

/// 取某用户的「锚 → 备注文本」，供会话列表/详情逐条回填
pub async fn map_for(state: &SharedState, user: &str) -> HashMap<String, String> {
    state
        .notes
        .read()
        .await
        .get(user)
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.clone(), v.text.clone()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str, at: u64) -> SessionNote {
        SessionNote {
            text: text.into(),
            at,
        }
    }

    /// 换行/制表符会把列表里的一行撑成多行 —— 归一化成空格，并折掉连续空白
    #[test]
    fn folds_control_chars_and_whitespace() {
        assert_eq!(normalize("  重构\n\t 支付模块  ").unwrap(), "重构 支付模块");
        assert_eq!(normalize("a\r\n\r\nb").unwrap(), "a b");
    }

    /// 全空白 = 清除备注（返回空串，由 set 删条目）
    #[test]
    fn blank_means_clear() {
        assert_eq!(normalize("   \n\t ").unwrap(), "");
        assert_eq!(normalize("").unwrap(), "");
    }

    /// 超长按**字符**数判，中文不能按字节算（33 个汉字 = 99 字节，不该被拒）
    #[test]
    fn length_limit_counts_chars_not_bytes() {
        let cn = "备".repeat(NOTE_MAX_CHARS);
        assert!(cn.len() > NOTE_MAX_CHARS, "构造用例应是多字节");
        assert!(normalize(&cn).is_ok(), "刚好到上限应通过");
        assert!(
            normalize(&"备".repeat(NOTE_MAX_CHARS + 1)).is_err(),
            "超一个字就该拒"
        );
    }

    /// 超长报错而不是悄悄截断（截断等于替用户丢内容）
    #[test]
    fn over_limit_reports_actual_length() {
        let e = normalize(&"x".repeat(NOTE_MAX_CHARS + 5)).unwrap_err();
        assert!(e.contains(&(NOTE_MAX_CHARS + 5).to_string()), "得到：{e}");
    }

    /// 超额淘汰丢最久没改的，刚写的那条必须留住
    #[test]
    fn evicts_least_recently_updated() {
        let mut t: NoteTable = (0..=MAX_NOTES_PER_USER)
            .map(|i| (format!("m|sh:{i}@1"), note("n", 1000 + i as u64)))
            .collect();
        evict_over_cap(&mut t);
        assert_eq!(t.len(), MAX_NOTES_PER_USER);
        assert!(!t.contains_key("m|sh:0@1"), "最久没改的那条该被丢掉");
        assert!(
            t.contains_key(&format!("m|sh:{MAX_NOTES_PER_USER}@1")),
            "最新的那条必须留住"
        );
    }

    /// 真落一次盘再读回来：hub 重启后备注还在
    #[test]
    fn survives_disk_roundtrip() {
        let dir = std::env::temp_dir().join(format!("am-notes-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = NoteStore::new();
        store
            .entry("alice".into())
            .or_default()
            .insert("m1|sh:42@1700".into(), note("支付重构", 1700));
        write_to(&dir, &store);
        assert_eq!(load(&dir), store, "重启后读回的备注应与写入时一致");
        // 落盘必须是原子替换：tmp 文件不能留下
        assert!(!dir.join("session_notes.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 读不到文件当空表，不能 panic（首次部署、数据目录被清都会走到这里）
    #[test]
    fn missing_file_loads_empty() {
        let dir = std::env::temp_dir().join(format!("am-notes-{}", uuid::Uuid::new_v4().simple()));
        assert!(load(&dir).is_empty());
    }
}
