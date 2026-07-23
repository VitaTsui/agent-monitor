//! 把「活进程」精确配到它正在跑的会话：活的 agent 进程一定占着自己的会话 `.jsonl`
//! 文件，已关闭的会话文件没人占。据此得出 pid → session_id 的确定配对，交给
//! build_tasks 优先采用；拿不到时（无 lsof / 平台不支持 / 查询失败）自动退回按
//! mtime 的启发式，行为不变。
//!
//! - Unix（mac/Linux）：`lsof -p <pids>` 列出这些进程打开的文件，取 `.jsonl` 文件名。
//! - Windows：Restart Manager（`RmGetList`）按文件查占用它的进程 —— 文档化 API、
//!   不像句柄枚举那样有挂起风险；只查近期活跃的会话文件并整体节流，控制开销。

use std::collections::HashMap;
use std::path::PathBuf;

/// 返回 pid → session_id（该进程当前打开着的会话文件名，无后缀）。
/// 失败/不支持时返回空表 —— 调用方据此退回 mtime 启发式。
pub fn pin_sessions(pids: &[u32], projects_dirs: &[PathBuf]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    #[cfg(unix)]
    {
        let _ = projects_dirs;
        pin_unix(pids)
    }
    #[cfg(windows)]
    {
        pin_windows(pids, projects_dirs)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pids, projects_dirs);
        HashMap::new()
    }
}

#[cfg(unix)]
fn pin_unix(pids: &[u32]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let csv = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    // -n -P：跳过 DNS/端口反查更快；-F pn：机器可读，按进程输出 p<pid> 与 n<name>
    let Ok(o) = std::process::Command::new("lsof")
        .args(["-n", "-P", "-F", "pn", "-p", &csv])
        .output()
    else {
        return out;
    };
    let text = String::from_utf8_lossy(&o.stdout);
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(r) = line.strip_prefix('p') {
            cur = r.trim().parse::<u32>().ok();
        } else if let Some(n) = line.strip_prefix('n') {
            if n.ends_with(".jsonl") {
                if let (Some(pid), Some(stem)) =
                    (cur, std::path::Path::new(n).file_stem().and_then(|s| s.to_str()))
                {
                    // 一个进程只占一个会话文件；保留第一个即可
                    out.entry(pid).or_insert_with(|| stem.to_string());
                }
            }
        }
    }
    out
}

/// 近期活跃窗口：只对最近改动过的会话文件做 Restart Manager 查询，避免对上百个
/// 历史 .jsonl 全查。已关闭很久的会话本就该判 Finished，无需精确配对。
#[cfg(windows)]
// 窗口要覆盖「进程还活着但会话闲置很久」的情形：闲置数天的会话若其 claude 进程仍在、
// 仍持有文件句柄，就该被 RmGetList 抓到并精确配对。6h 太窄会把它们排除在检查之外，
// 于是 pinned 恒为 0、只能退回易错位的时间戳启发式。8 天对齐配对候选窗口。
const RECENT_MS: u64 = 8 * 24 * 3600 * 1000;

#[cfg(windows)]
fn pin_windows(pids: &[u32], projects_dirs: &[PathBuf]) -> HashMap<u32, String> {
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    // 实测(2026-07，本机 Restart Manager 直接采样)：Claude Code 每次写入都是
    // open→append→close，会话 .jsonl 句柄寿命 < 25ms、不跨空闲持有。对 8 天窗口内 120
    // 个候选 jsonl（含闲置数天的会话）做与本函数等价的 RmGetList 全扫描，holder 恒为 0/120。
    // 结论：RM 精确配对在 Windows 上零收益（pinned 恒空），而每 4 轮最多 200 次
    // RmStartSession/Register/GetList/EndSession 是实打实的周期开销。故默认跳过，直接退回
    // build_tasks 的 btime(created_ms)/--resume/cached 配对层（这些数据在 Windows 上齐备）。
    // 需要重新验证句柄假设时，设环境变量 AM_PIN_RM=1 可临时恢复本扫描做诊断。
    if std::env::var_os("AM_PIN_RM").is_none() {
        return HashMap::new();
    }

    let mut out = HashMap::new();
    let pidset: HashSet<u32> = pids.iter().copied().collect();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // 收集近期活跃的候选会话文件
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in projects_dirs {
        collect_recent_jsonl(root, now_ms, &mut candidates, 0);
        if candidates.len() > 200 {
            break; // 兜底上限，别失控
        }
    }

    for file in &candidates {
        // RmGetList 会列出所有占用该文件的进程（claude 之外，Cursor/索引器/杀软也可能
        // 各持一个句柄）。只取「第一个」会漏掉排在后面的 claude —— 这里遍历全部，挑出
        // 属于本机 claude 会话进程集合的那个，才是这条会话文件的真正主人。
        for pid in holder_pids(file) {
            if pidset.contains(&pid) {
                if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
                    out.entry(pid).or_insert_with(|| stem.to_string());
                }
                break;
            }
        }
    }
    out
}

#[cfg(windows)]
fn collect_recent_jsonl(dir: &std::path::Path, now_ms: u64, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 || out.len() > 200 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            collect_recent_jsonl(&p, now_ms, out, depth + 1);
        } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if now_ms.saturating_sub(mtime_ms) <= RECENT_MS {
                out.push(p);
            }
        }
        if out.len() > 200 {
            return;
        }
    }
}

/// Restart Manager：返回占用该文件的、第一个 agent 候选进程 pid（失败返回 None）。
#[cfg(windows)]
fn holder_pids(file: &std::path::Path) -> Vec<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, RM_PROCESS_INFO,
        CCH_RM_SESSION_KEY,
    };

    let mut session: u32 = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: 传入符合 API 约定的缓冲区；失败即早退
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != 0 {
        return Vec::new();
    }
    let wide: Vec<u16> = file.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let files = [wide.as_ptr()];
    let reg = unsafe {
        RmRegisterResources(
            session,
            1,
            files.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    let result: Vec<u32> = if reg == 0 {
        let mut needed: u32 = 0;
        let mut count: u32 = 0;
        let mut reason: u32 = 0;
        // 先探所需数量
        let _ = unsafe {
            RmGetList(session, &mut needed, &mut count, std::ptr::null_mut(), &mut reason)
        };
        if needed == 0 {
            Vec::new()
        } else {
            let mut infos: Vec<RM_PROCESS_INFO> =
                vec![unsafe { std::mem::zeroed() }; needed as usize];
            count = needed;
            let rc = unsafe {
                RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reason)
            };
            if rc == 0 {
                infos
                    .iter()
                    .take(count as usize)
                    .map(|i| i.Process.dwProcessId)
                    .collect()
            } else {
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    unsafe { RmEndSession(session) };
    result
}
