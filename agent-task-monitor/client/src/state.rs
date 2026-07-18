//! 桌面客户端状态：本机扫描、排除、上报链路状态。
//! 不含任何服务端（HTTP 路由/注册表/登录态）。
use am_core::model::{Task, TaskStatus};
use am_core::process::ProcessScanner;
use am_core::scanner::SessionScanner;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub struct Config {
    /// 本机机器标识
    pub machine_id: String,
    pub hostname: String,
    /// macos / windows / linux
    pub platform: String,
    /// 数据目录（~/.agent-monitor）
    pub data_dir: std::path::PathBuf,
}

/// 本机「不允许被监控」的终端集合（按 tty 排除），持久化到 excluded.json。
/// 被排除的终端：其会话不会被扫描/上报（连有哪些终端都不外泄）。
#[derive(Default)]
pub struct Excludes {
    ttys: HashSet<String>,
    path: std::path::PathBuf,
}

impl Excludes {
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("excluded.json");
        let ttys = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Self { ttys, path }
    }
    fn save(&self) {
        let list: Vec<&String> = self.ttys.iter().collect();
        if let Ok(txt) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(&self.path, txt);
        }
    }
    pub fn is_excluded(&self, tty: &str) -> bool {
        !tty.is_empty() && self.ttys.contains(tty)
    }
    pub fn set(&mut self, tty: &str, excluded: bool) {
        if excluded {
            self.ttys.insert(tty.to_string());
        } else {
            self.ttys.remove(tty);
        }
        self.save();
    }
    pub fn list(&self) -> Vec<String> {
        self.ttys.iter().cloned().collect()
    }
}


pub fn upload_root() -> std::path::PathBuf {
    std::env::var("AM_UPLOAD_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

/// 校验文件下发的目标目录，返回可安全写入的绝对路径。
///
/// 不做限制的话，任何能向设备传文件的人都可以挑 `/root/.ssh`、`/etc/cron.d`
/// 之类的目录写文件（文件名虽已过滤穿越，但目录本身就足够拿下机器）。
/// 因此目标目录必须落在 `upload_root()` 之内。
pub fn safe_upload_dir(dir: &str) -> Result<std::path::PathBuf, String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("缺少目标目录".into());
    }
    let root = upload_root();
    let candidate = std::path::Path::new(dir);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    // 逐段归一化：不用 canonicalize（目标目录可能尚未创建），
    // 手工消掉 `.` 与 `..`，避免 root/../../etc 这类绕过。
    let mut out = std::path::PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err("目标目录非法".into());
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if !out.starts_with(&root) {
        return Err(format!("目标目录超出允许范围（仅允许 {} 之内）", root.display()));
    }
    Ok(out)
}

pub struct AppState {
    pub config: Config,
    pub scanner: Mutex<SessionScanner>,
    pub procs: Mutex<ProcessScanner>,
    /// 手动暂停过的本机 pid
    pub paused: RwLock<HashSet<u32>>,
    /// 因超额被自动暂停的本机 pid
    pub auto_paused: RwLock<HashSet<u32>>,
    /// 本机被排除监控的终端集合
    pub excludes: RwLock<Excludes>,
    /// 本机当前探测到的终端（托盘「监控范围」），(tty, 展示名)
    pub terminals: RwLock<Vec<(String, String)>>,
    /// 是否已连上 hub（托盘显示）
    pub hub_connected: std::sync::atomic::AtomicBool,
    /// 本设备是否已被信任（托盘显示）
    pub hub_trusted: std::sync::atomic::AtomicBool,
    /// hub 端新版本号（更新提示）
    pub hub_latest_version: RwLock<Option<String>>,
    /// 每设备上报令牌（配对后持久化于 data_dir/device-token）
    pub device_token: RwLock<Option<String>>,
    /// 进行中的配对 (code, pair_token)
    pub pair_info: RwLock<Option<(String, String)>>,
    /// 最近一次上报被拒原因（托盘显示，与断网区分）
    pub hub_error: RwLock<Option<String>>,
    /// hub 下发的 5h token 额度上限（0=不限）；本机据此自动暂停/恢复
    pub quota_limit: std::sync::atomic::AtomicU64,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new(config: Config) -> SharedState {
        let excludes = Excludes::load(&config.data_dir);
        let scanner = SessionScanner::new(
            dirs::home_dir().unwrap_or_default().join(".claude/projects"),
        );
        Arc::new(Self {
            config,
            scanner: Mutex::new(scanner),
            procs: Mutex::new(ProcessScanner::new()),
            paused: RwLock::new(HashSet::new()),
            auto_paused: RwLock::new(HashSet::new()),
            excludes: RwLock::new(excludes),
            terminals: RwLock::new(Vec::new()),
            hub_connected: std::sync::atomic::AtomicBool::new(false),
            hub_trusted: std::sync::atomic::AtomicBool::new(false),
            hub_latest_version: RwLock::new(None),
            device_token: RwLock::new(None),
            pair_info: RwLock::new(None),
            hub_error: RwLock::new(None),
            quota_limit: std::sync::atomic::AtomicU64::new(0),
        })
    }
}

/// 额度管控：给本机任务标注上限与用量；达到上限自动暂停，额度（5h 窗口）回落后自动恢复。
pub async fn enforce_quota(state: &SharedState, tasks: &mut [Task]) {
    // 额度上限由 hub 随上报响应下发（旧实现读本地注册表，远程设备永远拿不到 hub 配置）
    let limit = state.quota_limit.load(std::sync::atomic::Ordering::Relaxed);
    let mut auto = state.auto_paused.write().await;
    let mut paused = state.paused.write().await;

    for t in tasks.iter_mut() {
        t.token_limit = limit;
        let Some(pid) = t.pid else { continue };
        // 手动暂停的不受额度逻辑干预
        if paused.contains(&pid) {
            continue;
        }
        if limit > 0 && t.used_tokens_5h >= limit {
            // 超额 → 自动暂停
            if !auto.contains(&pid) {
                if am_core::process::control(pid, am_core::model::ControlAction::Pause).is_ok() {
                    auto.insert(pid);
                    tracing::info!(
                        "额度超限自动暂停: {} (用量 {} ≥ 上限 {})",
                        t.id, t.used_tokens_5h, limit
                    );
                }
            }
        } else if auto.contains(&pid) {
            // 额度回落 → 自动恢复
            if am_core::process::control(pid, am_core::model::ControlAction::Resume).is_ok() {
                auto.remove(&pid);
                tracing::info!("额度恢复自动继续: {} (用量 {} < 上限 {})", t.id, t.used_tokens_5h, limit);
            }
        }
        if auto.contains(&pid) {
            t.auto_paused = true;
            t.status = TaskStatus::Paused;
            t.status_dsr = "已暂停(超额)".into();
        }
    }
    // 清理已消失进程的暂停标记。
    // 手动暂停集合同样必须清理：pid 会被系统复用，残留的死 pid 会让复用到该
    // pid 的新会话被误判为「已手动暂停」，从而跳过额度管控并在界面上显示错误状态。
    // （被 SIGSTOP 的进程在 ps 中仍可见，故存活判定不会误删真正暂停中的条目。）
    let alive: HashSet<u32> = tasks.iter().filter_map(|t| t.pid).collect();
    auto.retain(|pid| alive.contains(pid));
    paused.retain(|pid| alive.contains(pid));
}

/// 扫描本机，产出带机器信息的任务快照。被排除的终端在此彻底剔除。
pub async fn local_scan(state: &SharedState) -> Vec<Task> {
    // procs.scan()/scanner.scan() 会起 `ps` 子进程、读会话文件，都是同步阻塞调用。
    // 直接在 async 上下文里跑会占住一个 worker 线程（每 1.5s 一次），
    // 期间该线程上的其它任务（HTTP 请求、WS 推送）全部排队。
    // block_in_place 会把同线程的其它任务挪走，代价最小。（多线程 runtime 才可用，
    // 见 main.rs：服务跑在 Runtime::new() 建的多线程 runtime 上。）
    let mut processes = {
        let mut procs = state.procs.lock().await;
        tokio::task::block_in_place(|| procs.scan())
    };
    // 记录本机全部探测到的终端（含被排除的），供托盘「监控范围」勾选
    {
        let mut seen: Vec<(String, String)> = Vec::new();
        for p in &processes {
            if !p.tty.is_empty() && !seen.iter().any(|(t, _)| t == &p.tty) {
                let name = p.cwd.rsplit(['/', '\\']).next().unwrap_or("").to_string();
                seen.push((p.tty.clone(), format!("{} ({})", name, p.ide_name)));
            }
        }
        *state.terminals.write().await = seen;
    }
    // 终端级排除：被排除 tty 的进程直接不参与后续（连有哪些终端都不外泄）
    {
        let excludes = state.excludes.read().await;
        processes.retain(|p| !excludes.is_excluded(&p.tty));
    }
    let sessions = {
        let mut scanner = state.scanner.lock().await;
        tokio::task::block_in_place(|| scanner.scan())
    };
    let paused = state.paused.read().await.clone();
    let mut tasks =
        am_core::scanner::build_tasks(&sessions, &processes, &|pid| paused.contains(&pid));
    // 会话文件层：无存活进程的会话若其历史 tty 被排除也一并剔除（尽力而为）
    // 这里主要保证「有进程」的会话已被上面的 retain 过滤。

    // 清理已消失进程的暂停标记
    {
        let alive: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        let mut p = state.paused.write().await;
        p.retain(|pid| alive.contains(pid));
    }

    attach_machine(
        &mut tasks,
        &state.config.machine_id,
        &state.config.hostname,
        &state.config.platform,
    );
    enforce_quota(state, &mut tasks).await;
    tasks
}

/// 给任务补上机器归属信息；占位任务 ID 加机器前缀防跨机冲突
pub fn attach_machine(tasks: &mut [Task], machine_id: &str, hostname: &str, platform: &str) {
    for t in tasks.iter_mut() {
        t.machine_id = machine_id.to_string();
        t.hostname = hostname.to_string();
        t.platform = platform.to_string();
        t.platform_dsr = am_core::model::platform_dsr(platform);
        if t.id.starts_with("pid-") {
            t.id = format!("{machine_id}-{}", t.id);
        }
    }
}

#[cfg(test)]
mod upload_dir_tests {
    use super::safe_upload_dir;

    /// AM_UPLOAD_ROOT 是进程级全局状态，而 cargo test 默认多线程并行跑：
    /// 不串行化的话，几个用例会互相踩对方的 set/remove —— 谁先 remove，
    /// 别人的 safe_upload_dir 就读到 fallback 的家目录，随机挂。
    static ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_root<T>(root: &str, f: impl FnOnce() -> T) -> T {
        // 用例断言失败会 panic，锁可能被投毒，这里不关心锁内数据，直接取回
        let _guard = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("AM_UPLOAD_ROOT", root);
        let r = f();
        std::env::remove_var("AM_UPLOAD_ROOT");
        r
    }

    #[test]
    fn accepts_paths_inside_root() {
        with_root("/tmp/amroot", || {
            assert_eq!(
                safe_upload_dir("/tmp/amroot/a/b").unwrap(),
                std::path::PathBuf::from("/tmp/amroot/a/b")
            );
            // 相对路径按 root 解析
            assert_eq!(
                safe_upload_dir("a/b").unwrap(),
                std::path::PathBuf::from("/tmp/amroot/a/b")
            );
        });
    }

    #[test]
    fn rejects_paths_outside_root() {
        with_root("/tmp/amroot", || {
            assert!(safe_upload_dir("/root/.ssh").is_err());
            assert!(safe_upload_dir("/etc/cron.d").is_err());
            // 穿越回上层
            assert!(safe_upload_dir("/tmp/amroot/../../etc").is_err());
            assert!(safe_upload_dir("../../etc").is_err());
            // 前缀相同但不是子目录
            assert!(safe_upload_dir("/tmp/amroot-evil").is_err());
            assert!(safe_upload_dir("").is_err());
        });
    }

    #[test]
    fn normalizes_dot_segments_inside_root() {
        with_root("/tmp/amroot", || {
            assert_eq!(
                safe_upload_dir("/tmp/amroot/a/../b").unwrap(),
                std::path::PathBuf::from("/tmp/amroot/b")
            );
        });
    }
}
