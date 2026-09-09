//! 与 Cursor / VSCode 扩展「终端任务监控 · 桥接」的文件级 IPC。
//!
//! 内嵌终端走 ConPTY，外部无法用 WriteConsoleInput 注入；改由编辑器里的扩展调用
//! `terminal.sendText` 送达。通信不引入本地服务，纯靠 `<data_dir>/bridge/` 下的文件：
//! - 扩展每 ~2s 写 `win-<id>.json`（{ts, terminals:[shell_pid,...]}）作心跳+终端清单；
//! - 客户端要给某内嵌终端下发时，写 `outbox/<ts>-<pid>.json`（{pid, text, ts, submit}）；
//! - 扩展轮询 outbox，pid 命中自己的某个终端就 sendText 并删文件，超时文件自清。
//!
//! 客户端只做两件事：判断「该 shell_pid 是否有活着的扩展在管」+ 写 outbox。

use std::path::{Path, PathBuf};

fn bridge_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bridge")
}

/// 是否有「活着的扩展」正在管理某终端 shell_pid（读扩展心跳 win-*.json，5s 内算活）。
pub fn has_live_terminal(data_dir: &Path, shell_pid: u32) -> bool {
    let dir = bridge_dir(data_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    let now = now_ms();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !(name.starts_with("win-") && name.ends_with(".json")) {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
            continue;
        };
        let ts = v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0);
        if now.saturating_sub(ts) > 5000 {
            continue; // 心跳过期，视为该编辑器窗口已关
        }
        let hit = v
            .get("terminals")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_u64)
                    .any(|p| p as u32 == shell_pid)
            })
            .unwrap_or(false);
        if hit {
            return true;
        }
    }
    false
}

/// 给某内嵌终端投递一条输入（写 outbox 文件，等扩展取走）。成功写盘返回 true。
///
/// `submit` = 送完文本后是否再补一个回车提交。发布任务当然要（true）；
/// **选择卡的选项作答不要**：那串数字的最后一个已经是「提交/下一题」键，再补回车就落到
/// 翻页后的下一题上、把它按默认高亮项答掉（见 agent 里 from_select 的判断）。
pub fn send_via_extension(data_dir: &Path, shell_pid: u32, text: &str, submit: bool) -> bool {
    let dir = bridge_dir(data_dir).join("outbox");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let ts = now_ms();
    // 毫秒不足以区分背靠背的两次投递：选择卡作答是「序号 → Tab → 回车」连着写的，
    // 全落在同一毫秒里，只用「毫秒-pid」做文件名后一条会**同名覆盖**前一条，
    // 单选「2」于是退化成一个裸回车、被选择卡当成选中默认高亮项。
    // 补一个进程内自增序号，两个字段都定宽，字典序 = 投递顺序（扩展就是 sort 后顺序处理的）。
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = dir.join(format!("{ts:013}-{seq:09}-{shell_pid}.json"));
    let body = serde_json::json!({ "pid": shell_pid, "text": text, "ts": ts, "submit": submit });
    std::fs::write(&file, body.to_string()).is_ok()
}

/// 同一毫秒内的投递序号，只保证本进程内单调递增；跨进程由前面的毫秒时间戳兜住。
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连着投递两条必须产生**两个**文件。
    ///
    /// 选择卡的作答会被拆成多步下发（序号 + Tab/回车），客户端 `for cmd in commands`
    /// 背靠背执行、中间没有任何等待。文件名若只有「毫秒-pid」，两次写入落在同一毫秒
    /// 就会同名覆盖 —— 只剩后一条。单选「2」于是退化成一个裸回车，选择卡收到回车
    /// 就选中默认高亮项，表现为「明明选的 2，终端选成了 1」。
    #[test]
    fn back_to_back_sends_never_collide() {
        let dir = std::env::temp_dir().join(format!("am-bridge-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pid = 4321;
        assert!(send_via_extension(&dir, pid, "2", false));
        assert!(send_via_extension(&dir, pid, "\r", false));
        let out = bridge_dir(&dir).join("outbox");
        let n = std::fs::read_dir(&out).unwrap().count();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(n, 2, "两条投递被覆盖成了 {n} 个文件");
    }

    /// 文件名要能按字典序还原出投递顺序 —— 扩展是 readdir 之后顺序处理的，
    /// 顺序错了就等于先回车后选号。
    #[test]
    fn filenames_sort_in_send_order() {
        let dir = std::env::temp_dir().join(format!("am-bridge-ord-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pid = 4321;
        for t in ["a", "b", "c", "d", "e"] {
            assert!(send_via_extension(&dir, pid, t, false));
        }
        let out = bridge_dir(&dir).join("outbox");
        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        let texts: Vec<String> = names
            .iter()
            .map(|n| {
                let txt = std::fs::read_to_string(out.join(n)).unwrap();
                let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                v.get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            texts,
            vec!["a", "b", "c", "d", "e"],
            "字典序没能还原投递顺序"
        );
    }
}
