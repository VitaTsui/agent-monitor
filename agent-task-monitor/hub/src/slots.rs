//! 会话号位：钉钉「@N / 发 N / 暂停 N」里的那个 N。
//!
//! 号位绑定「终端窗口」，不是「列表位置」。位置序号会随排序键漂移（title/prompt 一下发任务
//! 就变、会话增减、hub 重启丢掉冻结表），用户手上的编号随时失效 —— 实测过一次 `@2` 打到了
//! 列表第 5 位那个会话。
//!
//! 锚 = `(machine_id, shell_pid, shell_start)`：终端 shell 的 pid 跨 claude 的 /clear、
//! --resume、重启都不变（见 `am_core::model::ProcessInfo::shell_pid`），`shell_start` 再挡住
//! Windows 的 pid 重用。号位落盘，跨 hub 重启也不变。于是「@2」永远是同一个终端窗口。

use crate::state::{now_secs, SharedState};
use am_core::model::Task;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

/// 终端锚消失（终端关掉）后号位保留多久才允许被新终端复用。号位空着几乎没成本，立刻复用
/// 却会让用户照着旧列表发的「@N」打到一个刚开的、毫不相干的终端 —— 所以宁可留久些。
///
/// 一周：下班关机、周末不开机（周五到周一约 64 小时）都不该让号位重排。回收只为防号池膨胀。
const SLOT_KEEP_SECS: u64 = 7 * 24 * 3600;

/// 占位锚（还没配对到进程的会话，见 [`anchor_of`]）消失后的保留期 —— 只给 10 分钟。
///
/// 这种锚是短命的：会话一配对上进程就换成终端锚，旧的占位锚立刻成孤儿。若也按周计保留，
/// 终端反复重开几次号位就被孤儿占满、号从 2/4/5 跳成 2/7/13。它本来也投不进指令（pid 为空），
/// 不值得长期占号。
const PLACEHOLDER_KEEP_SECS: u64 = 600;

/// seen 续期到差多久才值得标脏落盘（避免每条钉钉消息都写一次盘）
const SEEN_FLUSH_SECS: u64 = 3600;

/// 一个用户的号位表（落盘于 data_dir/bot_slots.json）
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct SlotTable {
    /// 锚 → 号位
    #[serde(default)]
    slots: HashMap<String, u32>,
    /// 锚 → 最近一次见到它的 epoch 秒（锚消失后据此延迟回收号位）
    #[serde(default)]
    seen: HashMap<String, u64>,
    /// 当前「连续对话」锁定的号位：发过一次 `@N` 之后，不带 `@` 的普通文本都投给它。
    /// 只被下一个 `@M` 改变，不超时。跟着号位表一起落盘 —— 否则 hub 每次重启（部署）
    /// 都要重新 `@N` 一次。
    #[serde(default)]
    sticky: Option<u32>,
}

impl SlotTable {
    /// 给这批锚确保号位：已有的沿用旧号，新锚取最小空闲号；顺手回收过期号位。
    /// 返回「锚 → 号」与「表是否有值得落盘的变化」。
    fn assign(&mut self, keys: &[String], now: u64) -> (HashMap<String, u32>, bool) {
        let mut dirty = false;
        // 重启后 seen 可能比 slots 少（老版本表 / 手工改过）：缺的补成「本次首见」，
        // 让它照常走 SLOT_KEEP_SECS 的回收时钟，而不是永远占着号位。
        let missing: Vec<String> =
            self.slots.keys().filter(|k| !self.seen.contains_key(*k)).cloned().collect();
        for k in missing {
            self.seen.insert(k, now);
            dirty = true;
        }
        // 回收：锚很久没出现过 → 释放号位，免得号池无限膨胀（占位锚回收得快得多）
        let stale: Vec<String> = self
            .seen
            .iter()
            .filter(|(k, &at)| now.saturating_sub(at) > keep_secs(k))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            self.slots.remove(k);
            self.seen.remove(k);
            dirty = true;
        }

        let mut used: HashSet<u32> = self.slots.values().copied().collect();
        let mut out = HashMap::with_capacity(keys.len());
        for key in keys {
            let no = match self.slots.get(key) {
                Some(&n) => n,
                None => {
                    let mut n = 1u32;
                    while used.contains(&n) {
                        n += 1;
                    }
                    used.insert(n);
                    self.slots.insert(key.clone(), n);
                    dirty = true;
                    n
                }
            };
            // 续期：只有跨过 SEEN_FLUSH_SECS 才标脏，日常刷列表不写盘
            match self.seen.get(key) {
                Some(&at) if now.saturating_sub(at) < SEEN_FLUSH_SECS => {}
                _ => dirty = true,
            }
            self.seen.insert(key.clone(), now);
            out.insert(key.clone(), no);
        }
        (out, dirty)
    }
}

/// 号位锚的字面构造：有终端锚就用它，否则退化到会话 id。
fn anchor_key(machine_id: &str, shell: Option<(u32, u64)>, task_id: &str) -> String {
    match shell {
        Some((shell_pid, shell_start)) => format!("{machine_id}|sh:{shell_pid}@{shell_start}"),
        None => format!("{machine_id}|task:{task_id}"),
    }
}

/// 这个锚是「还没配对到进程」的占位锚吗（machine_id 里不含 `|`，取第一段之后判断）
fn is_placeholder(key: &str) -> bool {
    key.split_once('|').is_some_and(|(_, rest)| rest.starts_with("task:"))
}

/// 该锚消失后号位应保留多久
fn keep_secs(key: &str) -> u64 {
    if is_placeholder(key) {
        PLACEHOLDER_KEEP_SECS
    } else {
        SLOT_KEEP_SECS
    }
}

/// 会话的号位锚：优先「终端窗口」身份；没配对到进程的占位会话退化用会话 id
/// （这种会话 pid 为空、命令本来也投不进去，给个号只为能在列表里被指名）。
pub fn anchor_of(t: &Task) -> String {
    let shell =
        t.process.as_ref().and_then(|p| p.shell_pid.map(|sp| (sp, p.shell_start.unwrap_or(0))));
    anchor_key(&t.machine_id, shell, &t.id)
}

/// 给这批会话确保号位，返回「锚 → 号」，调用方用 [`anchor_of`] 反查。
pub async fn ensure(state: &SharedState, username: &str, tasks: &[Task]) -> HashMap<String, u32> {
    let keys: Vec<String> = tasks.iter().map(anchor_of).collect();
    let now = now_secs();
    let mut all = state.bot_slots.write().await;
    let (out, dirty) = all.entry(username.to_string()).or_default().assign(&keys, now);
    if dirty {
        state.bot_slots_dirty.store(true, Ordering::Relaxed);
    }
    out
}

/// 读「连续对话」当前锁定的号位
pub async fn sticky_of(state: &SharedState, username: &str) -> Option<u32> {
    state.bot_slots.read().await.get(username)?.sticky
}

/// 设「连续对话」锁定的号位（与当前值相同则不标脏，免得每条消息都写盘）
pub async fn set_sticky(state: &SharedState, username: &str, no: u32) {
    let mut all = state.bot_slots.write().await;
    let t = all.entry(username.to_string()).or_default();
    if t.sticky != Some(no) {
        t.sticky = Some(no);
        state.bot_slots_dirty.store(true, Ordering::Relaxed);
    }
}

/// 从数据目录加载号位表（读不到/解析失败都当空表：号位会重新分配，不影响可用性）
pub fn load(data_dir: &std::path::Path) -> HashMap<String, SlotTable> {
    let Ok(txt) = std::fs::read_to_string(data_dir.join("bot_slots.json")) else {
        return HashMap::new();
    };
    serde_json::from_str(&txt).unwrap_or_default()
}

/// 号位表落盘（原子写；tick 循环按脏标定期调用）
pub async fn save(state: &SharedState) {
    let snapshot = state.bot_slots.read().await.clone();
    let Ok(txt) = serde_json::to_string(&snapshot) else {
        return;
    };
    let path = state.config.data_dir.join("bot_slots.json");
    let tmp = state.config.data_dir.join("bot_slots.json.tmp");
    if std::fs::write(&tmp, &txt).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锚只认 shell pid + start：claude 换了 pid、会话 id 换了（/clear、--resume）仍同一个锚
    #[test]
    fn anchor_survives_session_id_change() {
        assert_eq!(
            anchor_key("m1", Some((100, 9)), "aaa"),
            anchor_key("m1", Some((100, 9)), "bbb")
        );
    }

    /// Windows 重用 shell pid：start 不同就是不同终端，不能共号
    #[test]
    fn anchor_rejects_shell_pid_reuse() {
        assert_ne!(
            anchor_key("m1", Some((100, 9)), "aaa"),
            anchor_key("m1", Some((100, 77)), "aaa")
        );
    }

    /// 同一 shell pid 在两台机器上互不相干
    #[test]
    fn anchor_scoped_by_machine() {
        assert_ne!(
            anchor_key("m1", Some((100, 9)), "aaa"),
            anchor_key("m2", Some((100, 9)), "aaa")
        );
    }

    /// 没配对到进程的占位会话按会话 id 各自成锚
    #[test]
    fn anchor_falls_back_to_task_id() {
        assert_ne!(anchor_key("m1", None, "aaa"), anchor_key("m1", None, "bbb"));
    }

    /// 号位一旦分配就钉住：会话集合变了、顺序反了，老锚的号都不动
    #[test]
    fn slots_are_stable_across_reorder() {
        let (a, b, c) = ("m|sh:1@1".to_string(), "m|sh:2@2".to_string(), "m|sh:3@3".to_string());
        let mut t = SlotTable::default();
        let first = t.assign(&[a.clone(), b.clone()], 1000).0;
        assert_eq!((first[&a], first[&b]), (1, 2));
        // 顺序反过来 + 新增一个 c：a/b 的号必须不变，c 拿下一个空闲号
        let second = t.assign(&[c.clone(), b.clone(), a.clone()], 1001).0;
        assert_eq!((second[&a], second[&b], second[&c]), (1, 2, 3));
    }

    /// 终端关掉：号位空着不复用，直到超过保留期才回收给新终端
    #[test]
    fn closed_terminal_slot_is_held_then_recycled() {
        let (a, b, fresh) =
            ("m|sh:1@1".to_string(), "m|sh:2@2".to_string(), "m|sh:9@9".to_string());
        let mut t = SlotTable::default();
        t.assign(&[a.clone(), b.clone()], 1000);
        // b 的终端关了（不再出现）：保留期内新终端不得占用 2 号
        let held = t.assign(&[a.clone(), fresh.clone()], 1000 + SLOT_KEEP_SECS - 1).0;
        assert_eq!(held[&fresh], 3, "保留期内 2 号仍属已关终端，新终端应拿 3");
        // 过了保留期：b 的号位回收，最小空闲号重新可用
        let late = 1000 + 2 * SLOT_KEEP_SECS + 1;
        assert_eq!(t.assign(&[a.clone()], late).0[&a], 1, "还在的终端号位不受回收影响");
        let newcomer = "m|sh:7@7".to_string();
        assert_eq!(t.assign(&[newcomer.clone()], late).0[&newcomer], 2, "回收后 2 号重新可分配");
    }

    /// 占位锚（没配对到进程）是短命的：消失后很快让出号位，别让孤儿把号池顶飞
    #[test]
    fn placeholder_slot_recycles_fast() {
        let ghost = "m|task:aaa".to_string();
        let mut t = SlotTable::default();
        assert_eq!(t.assign(&[ghost.clone()], 1000).0[&ghost], 1);
        // 会话配对上进程后换成终端锚，占位锚成孤儿 —— 保留期内还占着 1 号
        let real = "m|sh:5@5".to_string();
        assert_eq!(t.assign(&[real.clone()], 1000 + PLACEHOLDER_KEEP_SECS - 1).0[&real], 2);
        // 过了这 10 分钟，1 号就该让出来（终端锚同期还远没到回收线）
        let newcomer = "m|sh:6@6".to_string();
        let late = 1000 + PLACEHOLDER_KEEP_SECS + 1;
        assert_eq!(t.assign(&[real.clone(), newcomer.clone()], late).0[&newcomer], 1);
        assert_eq!(t.assign(&[real.clone()], late).0[&real], 2, "终端锚的号不受占位回收影响");
    }

    /// 占位锚只按 machine_id 之后的那段判定，别被 hostname 里的字样带偏
    #[test]
    fn placeholder_detection_is_scoped() {
        assert!(is_placeholder("m1|task:aaa"));
        assert!(!is_placeholder("m1|sh:100@9"));
        assert!(!is_placeholder("task:weird-hostname|sh:100@9"));
    }

    /// 落盘脏标：只在号位真变或 seen 跨过续期阈值时置位，日常刷列表不写盘
    #[test]
    fn dirty_only_on_real_change() {
        let a = "m|sh:1@1".to_string();
        let mut t = SlotTable::default();
        assert!(t.assign(&[a.clone()], 1000).1, "首次分配应标脏");
        assert!(!t.assign(&[a.clone()], 1010).1, "紧接着再刷不该标脏");
        // 阈值从上一次续期（1010）起算，不是从首次分配起算
        assert!(t.assign(&[a.clone()], 1010 + SEEN_FLUSH_SECS + 1).1, "续期跨阈值应标脏");
    }
}
