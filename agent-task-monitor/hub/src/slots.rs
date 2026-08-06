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

/// 号池上限：号位总数摸到它，就不再干等 [`SLOT_KEEP_SECS`]，提前回收已经不在的终端。
///
/// 光靠一周的保留期回收不住 —— 一周里开过的终端窗口累计几十个，号位一路涨到 80 多，
/// 而同时活着的不过十来个。「@83」既难记又难念，等一周才回收等于没有回收。
///
/// 30 给「实际十来个终端」留了两倍余量：日常根本摸不到这条线（行为与从前完全一致），
/// 只有号池真的膨胀了才触发。活着的终端**永远不回收**，超过 30 个也照常发号。
const SLOT_MAX: usize = 30;

/// 提前回收的最短静默期：不在本批活跃列表、且至少这么久没露过面，才准回收。
///
/// 活跃列表来自客户端上报，一轮抖动（网络、客户端重启、扫描慢了一拍）就可能少几个会话。
/// 没有这道闸门，一次抖动就会把还开着的终端的号收走、转手发给别人 —— 用户照旧列表发的
/// 「@N」于是打进毫不相干的终端，正是号位锚设计要避免的那件事。半小时足够盖住任何抖动。
const SLOT_MIN_IDLE_SECS: u64 = 1800;

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
    /// 锁定的会话最近一次「说过话」的 epoch 秒。超过 STICKY_COOLDOWN_SECS 没动静，
    /// 下一条不带 `@` 的消息先确认再下发（见 sticky_cooled）。
    #[serde(default)]
    sticky_at: u64,
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

        // 号池上限：还差多少个号位才装得下这一批，就提前回收多少个「已经不在的」终端。
        //
        // 只挑本批活跃锚之外、静默够久（SLOT_MIN_IDLE_SECS）、**且所属设备此刻在线**的，
        // 按最久没见到的先收。一个活着的锚都不动：活跃会话超过 SLOT_MAX 时收不到候选，
        // 照常往上发号。
        let alive: HashSet<&str> = keys.iter().map(String::as_str).collect();
        // 本批出现过的设备 = 此刻在上报的设备。**只有它们的会话清单才是可信的**。
        //
        // 活跃列表来自各设备的上报，设备一离线，它名下的会话就整批从列表里消失 —— 那不代表
        // 那些终端关了，只代表没人在汇报。少了这道判据，另一台机器关机半小时，它的号位就会
        // 被这边的新终端抢走；等它回来，用户手上的「@N」已经指向别人，钉钉回执照样说成功
        // （hub 确实入队了），命令却进了另一台设备的队列，终端毫无反应。
        // SLOT_KEEP_SECS 那一周的保留期本就是为「下班关机、周末不开机」留的，不能被这里绕过。
        let online: HashSet<&str> =
            alive.iter().filter_map(|k| k.split('|').next()).collect();
        let fresh = alive.iter().filter(|k| !self.slots.contains_key(**k)).count();
        let over = (self.slots.len() + fresh).saturating_sub(SLOT_MAX);
        if over > 0 {
            let mut dead: Vec<(u64, &String)> = self
                .slots
                .keys()
                .filter(|k| !alive.contains(k.as_str()))
                .filter(|k| k.split('|').next().is_some_and(|m| online.contains(m)))
                .filter_map(|k| {
                    let at = self.seen.get(k).copied().unwrap_or(0);
                    (now.saturating_sub(at) >= SLOT_MIN_IDLE_SECS).then_some((at, k))
                })
                .collect();
            dead.sort_unstable();
            let doomed: Vec<String> =
                dead.into_iter().take(over).map(|(_, k)| k.clone()).collect();
            for k in doomed {
                // 号位马上要转给新终端了，「连续对话」还锁着它就会串台：
                // 用户以为在跟原来那个终端说话，实际发进了刚顶上来的陌生会话。解锁，让他重新 @N。
                if let Some(no) = self.slots.remove(&k) {
                    if self.sticky == Some(no) {
                        self.sticky = None;
                    }
                }
                self.seen.remove(&k);
                dirty = true;
            }
        }

        // 锁定的终端关掉了就把锁放开。
        //
        // sticky 只被下一个 `@M` 改变、不会超时（那是为了让活跃对话期间完全无感），可它指着
        // 一个已经不在的终端时，之后每条不带 `@` 的消息都投不出去，而从钉钉那头完全看不出
        // 锁还在 —— 用户只看到「发了没反应」。号位本身要留一周（SLOT_KEEP_SECS，防 @N 打错），
        // 但「锁」没有理由陪着一起留。
        //
        // 判据同上面的回收：只在**设备在线**时才敢断定终端真的关了。设备离线只说明没人汇报，
        // 那时对它名下的锚一律不做判断，免得关机一次锁就掉。
        match self.slots.iter().find(|(_, &n)| Some(n) == self.sticky) {
            // 号位还在表里：锚不在活跃列表且其设备在线 → 那个终端确实关了
            Some((k, _))
                if !alive.contains(k.as_str())
                    && k.split('|').next().is_some_and(|m| online.contains(m)) =>
            {
                self.sticky = None;
                dirty = true;
            }
            // 号位压根不在表里（早被回收，或指向一个从未分配过的号）：锁也就没有着落了
            None if self.sticky.is_some() => {
                self.sticky = None;
                dirty = true;
            }
            _ => {}
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

/// 连续对话的「冷却」阈值：锁定的会话这么久没说过话，下一条不带 `@` 的消息不直接下发，
/// 先回一句「当前锁的是 N 号」等确认。
///
/// 隔了小半天随手发一句时，很可能已经忘了当前锁着哪个终端 —— 这一次确认，比事后发现任务
/// 打进了别的项目便宜得多。活跃对话期间完全无感（每条消息都会续期）。
const STICKY_COOLDOWN_SECS: u64 = 2 * 3600;

/// 按终端锚直接查号位（不分配）。用于给**已结束**的会话补号位 —— 那时会话已不在活跃列表里，
/// `ensure` 走不通，但锚还在表里（终端关掉要过保留期才回收）。
pub async fn slot_of(state: &SharedState, username: &str, anchor: &str) -> Option<u32> {
    state.bot_slots.read().await.get(username)?.slots.get(anchor).copied()
}

/// 读「连续对话」当前锁定的号位
pub async fn sticky_of(state: &SharedState, username: &str) -> Option<u32> {
    state.bot_slots.read().await.get(username)?.sticky
}

/// 锁定是否已「冷却」：太久没对话，下发前该先确认一次。没有锁定时返回 false。
pub async fn sticky_cooled(state: &SharedState, username: &str) -> bool {
    let all = state.bot_slots.read().await;
    let Some(t) = all.get(username) else { return false };
    if t.sticky.is_none() {
        return false;
    }
    now_secs().saturating_sub(t.sticky_at) > STICKY_COOLDOWN_SECS
}

/// 设「连续对话」锁定的号位，并续期活跃时间。
/// 号位没变、且上次续期就在不久前时不标脏，免得每条消息都写盘。
pub async fn set_sticky(state: &SharedState, username: &str, no: u32) {
    let now = now_secs();
    let mut all = state.bot_slots.write().await;
    let t = all.entry(username.to_string()).or_default();
    let changed = t.sticky != Some(no) || now.saturating_sub(t.sticky_at) > 600;
    t.sticky = Some(no);
    t.sticky_at = now;
    if changed {
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

    /// 造一张已经装满 SLOT_MAX 个号位的表：`live_n` 个还活着，其余是很久没露面的僵尸。
    /// 返回（表，活锚集合，僵尸锚集合）。
    fn packed_table(live_n: usize, born: u64) -> (SlotTable, Vec<String>, Vec<String>) {
        let all: Vec<String> = (0..SLOT_MAX).map(|i| format!("m|sh:{i}@{i}")).collect();
        let mut t = SlotTable::default();
        t.assign(&all, born);
        let (live, dead) = all.split_at(live_n);
        (t, live.to_vec(), dead.to_vec())
    }

    /// 号池摸到上限：提前回收已经不在的终端，新终端捡回小号 —— 号位不再一路涨到 80 多
    #[test]
    fn slot_pool_recycles_dead_at_cap() {
        let (mut t, live, dead) = packed_table(10, 1000);
        // 僵尸静默够久（> SLOT_MIN_IDLE_SECS），此时来个新终端
        let now = 1000 + SLOT_MIN_IDLE_SECS + 1;
        let newcomer = "m|sh:999@999".to_string();
        let mut batch = live.clone();
        batch.push(newcomer.clone());
        let out = t.assign(&batch, now);

        assert!(out.0[&newcomer] <= SLOT_MAX as u32, "新终端应捡回已释放的小号，而不是 31");
        assert!(t.slots.len() <= SLOT_MAX, "号池不得超过上限");
        for k in &live {
            assert!(t.slots.contains_key(k), "活着的锚 {k} 不该被回收");
        }
        assert!(dead.iter().any(|k| !t.slots.contains_key(k)), "该回收掉一些僵尸");
    }

    /// 活着的锚号位钉死：触发上限回收也不能动它们的号
    #[test]
    fn cap_recycle_never_touches_live_slots() {
        let (mut t, live, _) = packed_table(10, 1000);
        let before: Vec<u32> = live.iter().map(|k| t.slots[k]).collect();
        let now = 1000 + SLOT_MIN_IDLE_SECS + 1;
        let mut batch = live.clone();
        batch.push("m|sh:999@999".to_string());
        t.assign(&batch, now);
        let after: Vec<u32> = live.iter().map(|k| t.slots[k]).collect();
        assert_eq!(before, after, "活跃终端的号位必须原样不动");
    }

    /// 上报抖动保护：刚从列表里消失一轮的锚（静默不足 SLOT_MIN_IDLE_SECS）不许提前回收 ——
    /// 收了就等于把还开着的终端的号转手发给别人，用户的「@N」会打进陌生会话
    #[test]
    fn cap_recycle_spares_recently_seen() {
        let (mut t, live, dead) = packed_table(10, 1000);
        // 只过了几秒，僵尸其实是「刚抖没的」
        let now = 1000 + 10;
        let newcomer = "m|sh:999@999".to_string();
        let mut batch = live.clone();
        batch.push(newcomer.clone());
        t.assign(&batch, now);
        for k in &dead {
            assert!(t.slots.contains_key(k), "静默不足的锚 {k} 不该被回收");
        }
        assert_eq!(t.slots[&newcomer], SLOT_MAX as u32 + 1, "收不到候选就照常往上发号");
    }

    /// 设备离线保护：另一台机器整批从活跃列表消失（关机/断网/客户端没跑）时，它的号位
    /// 一个都不许收 —— 活跃列表只反映「谁在上报」，不代表那些终端关了。收了就等于把号
    /// 转手给本机新终端，那台机器回来后用户手上的「@N」已经指向别人。
    #[test]
    fn cap_recycle_spares_offline_machines() {
        // m1 一台把号池占满
        let m1: Vec<String> = (0..SLOT_MAX).map(|i| format!("m1|sh:{i}@{i}")).collect();
        let mut t = SlotTable::default();
        t.assign(&m1, 1000);
        let before: Vec<u32> = m1.iter().map(|k| t.slots[k]).collect();
        // m1 整台离线（一个锚都不在本批），只剩 m2 在上报，且早已过了静默期
        let now = 1000 + SLOT_MIN_IDLE_SECS + 1;
        let m2 = "m2|sh:1@1".to_string();
        let out = t.assign(&[m2.clone()], now).0;
        for k in &m1 {
            assert!(t.slots.contains_key(k), "离线设备的锚 {k} 不该被回收");
        }
        let after: Vec<u32> = m1.iter().map(|k| t.slots[k]).collect();
        assert_eq!(before, after, "离线设备的号位必须原样不动");
        assert_eq!(out[&m2], SLOT_MAX as u32 + 1, "收不到候选就照常往上发号");
    }

    /// 锁定的终端一关，「连续对话」就该自动解锁 —— 号位还留着（防 @N 打错），但锁不该陪着留：
    /// 指着一个已经不在的终端时，之后每条不带 @ 的消息都投不出去，用户还看不出原因
    #[test]
    fn sticky_released_when_locked_terminal_closes() {
        let (a, b) = ("m|sh:1@1".to_string(), "m|sh:2@2".to_string());
        let mut t = SlotTable::default();
        let out = t.assign(&[a.clone(), b.clone()], 1000).0;
        t.sticky = Some(out[&b]); // 锁定 b
        // b 的终端关了（不在本批），但 b 所属设备 m 仍在上报（a 还在）
        t.assign(&[a.clone()], 1100);
        assert_eq!(t.sticky, None, "锁定的终端已关，锁必须放开");
        assert!(t.slots.contains_key(&b), "号位本身仍要保留到 SLOT_KEEP_SECS");
    }

    /// 但设备整台离线时不许解锁：那只说明没人汇报，不代表终端关了 —— 关机一次就掉锁，
    /// 等回来还得重新 @N，与号位保留一周的初衷相悖
    #[test]
    fn sticky_kept_when_machine_offline() {
        let (a, b) = ("m1|sh:1@1".to_string(), "m2|sh:2@2".to_string());
        let mut t = SlotTable::default();
        let out = t.assign(&[a.clone(), b.clone()], 1000).0;
        let no = out[&b];
        t.sticky = Some(no);
        // m2 整台离线，只有 m1 在上报
        t.assign(&[a.clone()], 1100);
        assert_eq!(t.sticky, Some(no), "设备离线不该解锁");
    }

    /// 被回收的号若正被「连续对话」锁着，必须一并解锁 —— 否则号转手后消息串进陌生终端
    #[test]
    fn cap_recycle_clears_stale_sticky() {
        let (mut t, live, dead) = packed_table(10, 1000);
        // 锁定一个即将被回收的僵尸的号
        let doomed_no = t.slots[&dead[0]];
        t.sticky = Some(doomed_no);
        let now = 1000 + SLOT_MIN_IDLE_SECS + 1;
        let mut batch = live.clone();
        batch.push("m|sh:999@999".to_string());
        t.assign(&batch, now);
        if !t.slots.contains_key(&dead[0]) {
            assert_eq!(t.sticky, None, "锁定的号被回收了，sticky 必须清掉");
        }
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
