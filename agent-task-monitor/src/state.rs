use crate::model::{ControlCmd, MachineInfo, MessageBrief, Task, TaskStatus};
use crate::process::ProcessScanner;
use crate::registry::Registry;
use crate::scanner::SessionScanner;
use rsa::RsaPrivateKey;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, Mutex, RwLock};

pub struct Config {
    pub port: u16,
    /// 与前端 .env CRYPTO_KEY 一致的共享 AES 密钥
    pub crypto_key: String,
    pub private_key: RsaPrivateKey,
    /// 本机机器标识
    pub machine_id: String,
    pub hostname: String,
    /// macos / windows / linux
    pub platform: String,
    /// 数据目录（~/.agent-monitor）
    pub data_dir: std::path::PathBuf,
    /// 后管访问令牌（部署时生成，X-Admin-Token 头校验）
    pub admin_token: String,
    /// agent 上报令牌（hub 与 agent 共享，X-Agent-Token 头校验）
    pub agent_token: String,
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

/// hub 侧维护的一台机器（含 hub 本机）
pub struct MachineEntry {
    pub hostname: String,
    pub platform: String,
    pub version: String,
    pub is_hub: bool,
    pub tasks: Vec<Task>,
    pub last_report: Instant,
    /// 待下发给该 agent 的控制命令
    pub pending: VecDeque<ControlCmd>,
    /// 待下发给该 agent 的文件传输
    pub pending_files: VecDeque<crate::model::FileTransfer>,
    /// 会话 ID → 最近消息（agent 上报时缓存，供前端查看远程会话）
    pub messages: HashMap<String, Vec<MessageBrief>>,
    /// 待下发给该 agent 的 git 对比请求
    pub pending_git: VecDeque<crate::model::GitQuery>,
    /// 会话 ID → 最近一次 git 对比结果（agent 回传后缓存）
    pub git_cache: HashMap<String, crate::model::GitOverview>,
}

/// 机器离线判定阈值
pub const OFFLINE_AFTER_SECS: u64 = 10;

/// 文件下发允许写入的根目录：AM_UPLOAD_ROOT，默认用户主目录。
/// 常量时间比较令牌。
/// `==` 会先比长度再逐字节短路返回，把「猜对了多少前缀」和长度以耗时形式泄露出去；
/// 各调用点的失败延迟是在比较**之后**才 sleep，掩盖不了这一点。
pub fn token_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    // 长度不等时与等长输入走同样的比较开销，避免长度成为旁路
    if a.len() != b.len() {
        // 仍做一次等长比较再丢弃结果，防止长度检查本身被计时区分
        let _ = a.as_bytes().ct_eq(a.as_bytes());
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
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

/// 登录态有效期：7 天。到期强制重新登录。
/// （没有有效期的话 token 表只增不减，且泄露的 token 会永久有效。）
pub const SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

/// 口令爆破节流：按账号累计连续失败次数，失败后延迟应答。
/// 不按 IP：服务跑在 Caddy 反代后，对端 IP 恒为反代，而 X-Forwarded-For 可伪造。
#[derive(Default)]
pub struct LoginThrottle {
    /// 用户名 → (连续失败次数, 最近一次失败时刻)
    fails: HashMap<String, (u32, Instant)>,
}

/// 连续失败记录的遗忘时间
const THROTTLE_WINDOW_SECS: u64 = 900;
/// 单次失败最长延迟
const THROTTLE_MAX_DELAY_MS: u64 = 3_000;
/// 节流表容量上限（防被随机用户名刷爆）
const THROTTLE_MAX_ENTRIES: usize = 10_000;

impl LoginThrottle {
    /// 本次尝试前应等待的时长（按已累计的连续失败次数指数退避）
    pub fn delay_for(&self, username: &str) -> std::time::Duration {
        match self.fails.get(username) {
            Some((n, at)) if at.elapsed().as_secs() < THROTTLE_WINDOW_SECS && *n > 0 => {
                let ms = 100u64.saturating_mul(1u64 << (*n).min(6));
                std::time::Duration::from_millis(ms.min(THROTTLE_MAX_DELAY_MS))
            }
            _ => std::time::Duration::ZERO,
        }
    }

    pub fn record_fail(&mut self, username: &str) {
        self.fails.retain(|_, (_, at)| at.elapsed().as_secs() < THROTTLE_WINDOW_SECS);
        if self.fails.len() >= THROTTLE_MAX_ENTRIES && !self.fails.contains_key(username) {
            // 满了就先丢最久没失败过的，保证新条目总能记上
            if let Some(k) = self
                .fails
                .iter()
                .max_by_key(|(_, (_, at))| at.elapsed())
                .map(|(k, _)| k.clone())
            {
                self.fails.remove(&k);
            }
        }
        let e = self.fails.entry(username.to_string()).or_insert((0, Instant::now()));
        e.0 = e.0.saturating_add(1);
        e.1 = Instant::now();
    }

    pub fn record_success(&mut self, username: &str) {
        self.fails.remove(username);
    }
}

/// 一次登录签发的会话
#[derive(Clone)]
pub struct Session {
    pub username: String,
    pub issued_at: Instant,
}

impl Session {
    pub fn new(username: String) -> Self {
        Self { username, issued_at: Instant::now() }
    }

    pub fn expired(&self) -> bool {
        self.issued_at.elapsed().as_secs() >= SESSION_TTL_SECS
    }
}

pub struct AppState {
    pub config: Config,
    pub scanner: Mutex<SessionScanner>,
    pub procs: Mutex<ProcessScanner>,
    /// 全部机器（key = machine_id），hub 本机也在其中
    pub machines: RwLock<HashMap<String, MachineEntry>>,
    /// 手动暂停过的本机 pid（SIGSTOP 后 ps 状态有延迟，本地先行标记）
    pub paused: RwLock<HashSet<u32>>,
    /// 因超额被自动暂停的本机 pid（额度重置后自动恢复）
    pub auto_paused: RwLock<HashSet<u32>>,
    /// 已签发的登录 token → 会话（用户名 + 签发时刻）
    pub tokens: RwLock<HashMap<String, Session>>,
    /// 用户 + 设备信任注册表（持久化）
    pub registry: RwLock<Registry>,
    /// 本机被排除监控的终端集合
    pub excludes: RwLock<Excludes>,
    /// 本机当前探测到的终端（供托盘「监控范围」勾选），(tty, 展示名)
    pub terminals: RwLock<Vec<(String, String)>>,
    /// 任务快照变更信号（每次扫描自增，WS 收到后按各自用户重新拉取）
    pub tx: broadcast::Sender<u64>,
    pub started_at: chrono::DateTime<chrono::Local>,
    /// agent 模式：是否已连上 hub（供托盘显示连接状态）
    pub hub_connected: std::sync::atomic::AtomicBool,
    /// agent 模式：本设备是否已被信任（供托盘显示）
    pub hub_trusted: std::sync::atomic::AtomicBool,
    /// agent 模式：最近一次上报被 hub 拒绝的原因（供托盘显示）。
    /// 只标「未连接」是不够的：令牌不对 / 用户名不存在 / machineId 冲突
    /// 与「网线拔了」在界面上长得一模一样，而桌面版是 GUI，用户看不到日志，
    /// 只会看到永远的「连接中…」。
    pub hub_error: RwLock<Option<String>>,
    /// 已签发的 OAuth state → 签发时刻（防 CSRF：回调必须带回服务端发过的 state，一次性消费）
    pub oauth_states: RwLock<HashMap<String, Instant>>,
    /// 口令登录节流（防爆破）
    pub login_throttle: RwLock<LoginThrottle>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new(config: Config, scanner: SessionScanner, registry: Registry) -> SharedState {
        let (tx, _) = broadcast::channel(64);
        let excludes = Excludes::load(&config.data_dir);
        Arc::new(Self {
            config,
            scanner: Mutex::new(scanner),
            procs: Mutex::new(ProcessScanner::new()),
            machines: RwLock::new(HashMap::new()),
            paused: RwLock::new(HashSet::new()),
            auto_paused: RwLock::new(HashSet::new()),
            tokens: RwLock::new(HashMap::new()),
            registry: RwLock::new(registry),
            excludes: RwLock::new(excludes),
            terminals: RwLock::new(Vec::new()),
            tx,
            started_at: chrono::Local::now(),
            hub_connected: std::sync::atomic::AtomicBool::new(false),
            hub_trusted: std::sync::atomic::AtomicBool::new(false),
            hub_error: RwLock::new(None),
            oauth_states: RwLock::new(HashMap::new()),
            login_throttle: RwLock::new(LoginThrottle::default()),
        })
    }

    /// 聚合指定用户「可见」的任务（信任 + 归属；超级管理员看全部）
    pub async fn tasks_for(&self, username: &str) -> Vec<Task> {
        let machines = self.machines.read().await;
        let registry = self.registry.read().await;
        let mut out = Vec::new();
        for (id, entry) in machines.iter() {
            if !registry.can_view(id, username) {
                continue;
            }
            let online = entry.last_report.elapsed().as_secs() < OFFLINE_AFTER_SECS;
            for t in &entry.tasks {
                let mut t = t.clone();
                if !online {
                    t.status = TaskStatus::Finished;
                    t.status_dsr = "已离线".into();
                    t.process = None;
                    t.pid = None;
                }
                out.push(t);
            }
        }
        out.sort_by(|a, b| {
            let rank = |t: &Task| match t.status {
                TaskStatus::Running => 0,
                TaskStatus::Paused => 1,
                TaskStatus::Idle => 2,
                TaskStatus::Finished => 3,
            };
            rank(a).cmp(&rank(b)).then(b.mtime_ms.cmp(&a.mtime_ms))
        });
        out
    }

    /// 设备管理列表：该用户名下的全部设备（含未信任的 pending）
    pub async fn devices_for(&self, username: &str) -> Vec<MachineInfo> {
        let machines = self.machines.read().await;
        let registry = self.registry.read().await;
        let mut out: Vec<MachineInfo> = machines
            .iter()
            .filter(|(id, _)| registry.owned_by(id, username))
            .map(|(id, e)| {
                let online = e.last_report.elapsed().as_secs() < OFFLINE_AFTER_SECS;
                let meta = registry.device_meta(id);
                MachineInfo {
                    id: id.clone(),
                    hostname: e.hostname.clone(),
                    platform: e.platform.clone(),
                    platform_dsr: crate::model::platform_dsr(&e.platform),
                    version: e.version.clone(),
                    online,
                    is_hub: e.is_hub,
                    last_report_at: None,
                    session_count: e.tasks.len(),
                    running_count: e
                        .tasks
                        .iter()
                        .filter(|t| online && t.status == TaskStatus::Running)
                        .count(),
                    owner: meta.owner,
                    trusted: meta.trusted,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.is_hub
                .cmp(&a.is_hub)
                .then(a.trusted.cmp(&b.trusted))
                .then(a.hostname.cmp(&b.hostname))
        });
        out
    }
}

/// 给任务补上机器归属信息；占位任务 ID 加机器前缀防跨机冲突
pub fn attach_machine(tasks: &mut [Task], machine_id: &str, hostname: &str, platform: &str) {
    for t in tasks.iter_mut() {
        t.machine_id = machine_id.to_string();
        t.hostname = hostname.to_string();
        t.platform = platform.to_string();
        t.platform_dsr = crate::model::platform_dsr(platform);
        if t.id.starts_with("pid-") {
            t.id = format!("{machine_id}-{}", t.id);
        }
    }
}

/// hub 模式后台循环：扫描本机 + 发出变更信号
pub async fn scan_loop(state: SharedState) {
    let mut tick: u64 = 0;
    loop {
        let mut tasks = local_scan(&state).await;
        enforce_quota(&state, &mut tasks).await;
        {
            let mut machines = state.machines.write().await;
            let entry = machines
                .entry(state.config.machine_id.clone())
                .or_insert_with(|| MachineEntry {
                    hostname: state.config.hostname.clone(),
                    platform: state.config.platform.clone(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    is_hub: true,
                    tasks: Vec::new(),
                    last_report: Instant::now(),
                    pending: VecDeque::new(),
                    pending_files: VecDeque::new(),
                    messages: HashMap::new(),
                    pending_git: VecDeque::new(),
                    git_cache: HashMap::new(),
                });
            entry.tasks = tasks;
            entry.last_report = Instant::now();
        }

        tick = tick.wrapping_add(1);

        // 过期会话清扫（约每 10 分钟）。鉴权路径只在「有人拿着过期 token 来访问」
        // 时才顺带清扫，用户关掉页面不再回来的会话会永久滞留，故这里主动回收。
        if tick % 400 == 0 {
            state.tokens.write().await.retain(|_, s| !s.expired());
        }

        let _ = state.tx.send(tick);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}

/// 额度管控：给本机任务标注上限与用量；达到上限自动暂停，额度（5h 窗口）回落后自动恢复。
pub async fn enforce_quota(state: &SharedState, tasks: &mut [Task]) {
    let limit = state.registry.read().await.quota_limit();
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
                if crate::process::control(pid, crate::model::ControlAction::Pause).is_ok() {
                    auto.insert(pid);
                    tracing::info!(
                        "额度超限自动暂停: {} (用量 {} ≥ 上限 {})",
                        t.id, t.used_tokens_5h, limit
                    );
                }
            }
        } else if auto.contains(&pid) {
            // 额度回落 → 自动恢复
            if crate::process::control(pid, crate::model::ControlAction::Resume).is_ok() {
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
        crate::scanner::build_tasks(&sessions, &processes, &|pid| paused.contains(&pid));
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
