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
pub fn send_via_extension(data_dir: &Path, shell_pid: u32, text: &str) -> bool {
    let dir = bridge_dir(data_dir).join("outbox");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let ts = now_ms();
    let file = dir.join(format!("{ts}-{shell_pid}.json"));
    let body = serde_json::json!({ "pid": shell_pid, "text": text, "ts": ts, "submit": true });
    std::fs::write(&file, body.to_string()).is_ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
