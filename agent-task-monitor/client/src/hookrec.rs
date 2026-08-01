//! Claude Code hook 上报：让 agent **自己说出**「我是谁」，取代靠启发式猜配对。
//!
//! 病灶回顾：会话（jsonl）与进程（claude.exe）的对应关系一直靠猜 —— 权威 pin 只存在于
//! claude 临时派生的工具子进程 env 里（跑完就没），空闲会话一个都抓不到；抓不到就退回
//! mtime 启发式，同一时刻开的两个终端极易配错，表现为钉钉里标题串号、下发打到别的终端。
//! 为此堆了终端锚、pin 累积表、clear-follow 迁移等一层层补丁。
//!
//! 而 Claude Code 的 hook 在 stdin 里**直接给出** `session_id` 和 `cwd`，环境变量里还有
//! `CLAUDE_PID`。这是一条权威、及时、无需碰运气的配对信号 —— 有它就不必猜。
//!
//! 通道沿用 bridge 的思路：不引入本地服务，纯文件。hook 进程把一条记录写进
//! `<data_dir>/hooks/<pid>.json`，扫描循环读取即可。
//!
//! 用法（配在 `~/.claude/settings.json`）：
//! ```json
//! { "hooks": { "SessionStart": [ { "hooks": [
//!     { "type": "command", "command": "<客户端可执行文件> hook" } ] } ] } }
//! ```
//! `PreToolUse` 挂同样的命令也可以（更频繁、更保险）。hook 必须**极快且绝不打断 claude**，
//! 所以这里只做「读 stdin → 写一个小文件」，任何异常都静默退出 0。

use std::io::Read;
use std::path::{Path, PathBuf};

/// hook 记录目录
pub fn hooks_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("hooks")
}

/// 一条 hook 上报：某个 claude 进程自报的会话身份
#[derive(Debug, Clone)]
pub struct HookReport {
    pub claude_pid: u32,
    pub session_id: String,
    pub cwd: String,
    /// 写入时刻（epoch 秒），用于判新旧与清理
    pub at: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `am-client hook` 子命令入口：从 stdin 读 hook JSON，落一条记录。
///
/// **绝不失败**：hook 是同步阻塞 claude 的，报错或挂起都会拖累用户的会话。
/// 任何异常都静默返回，宁可这次没记上，也不能影响 claude。
pub fn run_hook_cli(data_dir: &Path) {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return;
    }
    let v: serde_json::Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => return,
    };
    let session_id = v.get("session_id").and_then(|x| x.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let cwd = v.get("cwd").and_then(|x| x.as_str()).unwrap_or("");
    // CLAUDE_PID 由 claude 注入到子进程环境；hook 进程正是它的子进程，所以拿得到。
    // 万一没有就退回父进程 pid（hook 的父进程即 claude 本身）。
    let claude_pid = std::env::var("CLAUDE_PID")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .or_else(parent_pid)
        .unwrap_or(0);
    if claude_pid == 0 {
        return;
    }
    let dir = hooks_dir(data_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let rec = serde_json::json!({
        "claude_pid": claude_pid,
        "session_id": session_id,
        "cwd": cwd,
        "at": now_secs(),
    });
    let Ok(txt) = serde_json::to_string(&rec) else { return };
    // 原子写：扫描循环随时可能在读，半个文件会解析失败
    let tmp = dir.join(format!("{claude_pid}.json.tmp"));
    if std::fs::write(&tmp, txt).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{claude_pid}.json")));
    }
}

/// 取当前进程的父进程 pid（CLAUDE_PID 缺失时的兜底）
#[cfg(windows)]
fn parent_pid() -> Option<u32> {
    // Windows 上没有现成的轻量 API，交给 sysinfo 又太重（hook 要求极快）。
    // 缺 CLAUDE_PID 时直接放弃这条记录 —— 有 PreToolUse 的话下一次工具调用还会再报。
    None
}

#[cfg(not(windows))]
fn parent_pid() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

/// 读取全部 hook 上报（供扫描循环用），顺带清理过期记录。
///
/// `max_age_secs`：超过这个时长的记录视为陈旧 —— 进程多半已退出，留着会让死 pid 一直占着
/// 一条权威配对。取值应显著大于 claude 的空闲时长，否则长时间不动的会话会失去 hook 信号
/// （不过那时还有 pin 累积表和启发式兜底，不至于配不上）。
pub fn read_reports(data_dir: &Path, max_age_secs: u64) -> Vec<HookReport> {
    let dir = hooks_dir(data_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let now = now_secs();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
            let _ = std::fs::remove_file(&path); // 坏文件直接删，免得每轮都解析失败
            continue;
        };
        let at = v.get("at").and_then(|x| x.as_u64()).unwrap_or(0);
        if now.saturating_sub(at) > max_age_secs {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let (Some(pid), Some(sid)) = (
            v.get("claude_pid").and_then(|x| x.as_u64()),
            v.get("session_id").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        out.push(HookReport {
            claude_pid: pid as u32,
            session_id: sid.to_string(),
            cwd: v.get("cwd").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            at,
        });
    }
    out
}

/// 删掉某个 pid 的 hook 记录（进程已确认退出时调用，避免 pid 重用后张冠李戴）
pub fn drop_report(data_dir: &Path, claude_pid: u32) {
    let _ = std::fs::remove_file(hooks_dir(data_dir).join(format!("{claude_pid}.json")));
}
