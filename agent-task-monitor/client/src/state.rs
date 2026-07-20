//! 桌面客户端状态：本机扫描、排除、上报链路状态。
//! 不含任何服务端（HTTP 路由/注册表/登录态）。
use am_core::model::Task;
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
    /// hub 下发的强制更新下限：本机低于它必须更新才能继续使用
    pub hub_min_version: RwLock<Option<String>>,
    /// 每设备上报令牌（配对后持久化于 data_dir/device-token）
    pub device_token: RwLock<Option<String>>,
    /// 进行中的配对 (code, pair_token)
    pub pair_info: RwLock<Option<(String, String)>>,
    /// 最近一次上报被拒原因（托盘显示，与断网区分）
    pub hub_error: RwLock<Option<String>>,
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
            hub_min_version: RwLock::new(None),
            device_token: RwLock::new(None),
            pair_info: RwLock::new(None),
            hub_error: RwLock::new(None),
        })
    }
}


/// 客户端落盘日志（与 desktop::ulog 同一文件；服务/扫描层也能写）
pub fn client_log(msg: &str) {
    tracing::info!("{msg}");
    let path = std::env::var("AM_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| dirs::data_dir().unwrap_or_default().join("AgentMonitor"))
        .join("client.log");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = chrono::Local::now().format("%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

static SCAN_TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 终端的排除键：unix 用 tty（一个终端标签一个 tty）；
/// Windows 拿不到 tty，退而用工作目录 —— 语义变成「排除该项目目录的终端」，
/// 且跨进程重启稳定（此前 Windows 上终端列表永远为空，监控范围形同虚设）。
pub fn terminal_key(p: &am_core::model::ProcessInfo) -> String {
    if p.tty.is_empty() {
        format!("cwd:{}", p.cwd)
    } else {
        p.tty.clone()
    }
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
    // 记录本机全部探测到的终端（含被排除的），供托盘/设置里的「监控范围」勾选
    {
        let mut seen: Vec<(String, String)> = Vec::new();
        for p in &processes {
            let key = terminal_key(p);
            if !seen.iter().any(|(t, _)| t == &key) {
                let name = p
                    .cwd
                    .split(['/', '\\'])
                    .rev()
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string();
                seen.push((key, format!("{} ({})", name, p.ide_name)));
            }
        }
        *state.terminals.write().await = seen;
    }
    // 终端级排除：被排除终端的进程直接不参与后续（连有哪些终端都不外泄）
    {
        let excludes = state.excludes.read().await;
        processes.retain(|p| !excludes.is_excluded(&terminal_key(p)));
    }
    let sessions = {
        let mut scanner = state.scanner.lock().await;
        tokio::task::block_in_place(|| scanner.scan())
    };
    // 扫描诊断（前 3 轮 + 之后每 ~60s 一次）：会话扫不到时能从日志直接定位
    // 是目录不存在、没有 jsonl、还是解析失败
    {
        use std::sync::atomic::Ordering;
        let n = SCAN_TICKS.fetch_add(1, Ordering::Relaxed);
        if n < 3 || n % 40 == 0 {
            let scanner = state.scanner.lock().await;
            let dir = scanner.projects_dir().to_path_buf();
            let (mut pdirs, mut jsonl) = (0u32, 0u32);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        pdirs += 1;
                        if let Ok(fs2) = std::fs::read_dir(e.path()) {
                            jsonl += fs2
                                .flatten()
                                .filter(|f| {
                                    f.path().extension().and_then(|x| x.to_str()) == Some("jsonl")
                                })
                                .count() as u32;
                        }
                    }
                }
            }
            client_log(&format!(
                "[scan] projects_dir={} exists={} 项目目录={} jsonl文件={} 解析出会话={} 代理进程={}",
                dir.display(),
                dir.is_dir(),
                pdirs,
                jsonl,
                sessions.len(),
                processes.len()
            ));
            // 配对诊断：会话与进程都在却配不上（Windows「一直等待输入、不同步」）时，
            // 打出两侧的配对键 —— 进程侧 key = encode_path(cwd)，会话侧要等于 project_key，
            // 一眼看出是尾随分隔符 / 大小写 / 编码规则哪里对不上。只在启动前 3 轮打印，
            // 避免长期刷屏。
            if n < 3 && !sessions.is_empty() && !processes.is_empty() {
                for p in &processes {
                    client_log(&format!(
                        "[pair] proc agent={} key={} cwd={:?}",
                        p.agent,
                        am_core::scanner::encode_path(&p.cwd),
                        p.cwd
                    ));
                }
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                for s in &sessions {
                    let id6: String = s.session_id.chars().rev().take(6).collect::<Vec<_>>()
                        .into_iter().rev().collect();
                    let age_s = now_ms.saturating_sub(s.mtime_ms) / 1000;
                    // created=0 说明该系统取不到文件创建时间（btime），created_ms 配对会失效
                    let created_age = if s.created_ms == 0 {
                        "n/a".to_string()
                    } else {
                        format!("{}s", now_ms.saturating_sub(s.created_ms) / 1000)
                    };
                    client_log(&format!(
                        "[pair] sess id=..{} key={} mtime_age={}s created_age={} ended={}",
                        id6, s.project_key, age_s, created_age, s.turn_ended
                    ));
                }
                for p in &processes {
                    let pstart_age = now_ms / 1000 - p.start_time;
                    client_log(&format!(
                        "[pair] procstart pid={} start_age={}s",
                        p.pid, pstart_age
                    ));
                }
            }
        }
    }
    let paused = state.paused.read().await.clone();
    // 按「进程打开着哪个会话文件」得出确定配对；lsof/RmGetList 有开销，节流每 4 轮算
    // 一次、其余复用缓存（会话与文件的对应关系很稳定）。失败则空表，build_tasks 退回
    // mtime 启发式，行为不变。
    let pinned = {
        use std::sync::atomic::Ordering;
        static PIN_CACHE: std::sync::Mutex<Option<std::collections::HashMap<u32, String>>> =
            std::sync::Mutex::new(None);
        let tick = SCAN_TICKS.load(Ordering::Relaxed);
        if tick % 4 == 0 {
            let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
            let dirs = {
                let scanner = state.scanner.lock().await;
                let home = dirs::home_dir().unwrap_or_default();
                vec![
                    scanner.projects_dir().to_path_buf(),
                    home.join(".codex/sessions"),
                ]
            };
            let fresh =
                tokio::task::block_in_place(|| crate::openfiles::pin_sessions(&pids, &dirs));
            *PIN_CACHE.lock().unwrap() = Some(fresh.clone());
            fresh
        } else {
            PIN_CACHE.lock().unwrap().clone().unwrap_or_default()
        }
    };
    let mut tasks = am_core::scanner::build_tasks(
        &sessions,
        &processes,
        &|pid| paused.contains(&pid),
        &pinned,
    );
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
