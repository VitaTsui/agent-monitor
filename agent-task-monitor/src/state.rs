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
    pub username: String,
    pub password: String,
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
}

/// 机器离线判定阈值
pub const OFFLINE_AFTER_SECS: u64 = 10;

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
    /// 已签发的登录 token → 用户名
    pub tokens: RwLock<HashMap<String, String>>,
    /// 用户 + 设备信任注册表（持久化）
    pub registry: RwLock<Registry>,
    /// 本机被排除监控的终端集合
    pub excludes: RwLock<Excludes>,
    /// 本机当前探测到的终端（供托盘「监控范围」勾选），(tty, 展示名)
    pub terminals: RwLock<Vec<(String, String)>>,
    /// 任务快照变更信号（每次扫描自增，WS 收到后按各自用户重新拉取）
    pub tx: broadcast::Sender<u64>,
    pub started_at: chrono::DateTime<chrono::Local>,
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

    /// 该用户能否监控某任务所属机器（信任 + 归属）
    pub async fn can_view_machine(&self, machine_id: &str, username: &str) -> bool {
        self.registry.read().await.can_view(machine_id, username)
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
                });
            entry.tasks = tasks;
            entry.last_report = Instant::now();
        }

        tick = tick.wrapping_add(1);
        let _ = state.tx.send(tick);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}

/// 额度管控：给本机任务标注上限与用量；达到上限自动暂停，额度（5h 窗口）回落后自动恢复。
pub async fn enforce_quota(state: &SharedState, tasks: &mut [Task]) {
    let limit = state.registry.read().await.quota_limit();
    let mut auto = state.auto_paused.write().await;
    let paused = state.paused.read().await;

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
    // 清理已消失进程的自动暂停标记
    let alive: HashSet<u32> = tasks.iter().filter_map(|t| t.pid).collect();
    auto.retain(|pid| alive.contains(pid));
}

/// 扫描本机，产出带机器信息的任务快照。被排除的终端在此彻底剔除。
pub async fn local_scan(state: &SharedState) -> Vec<Task> {
    let mut processes = {
        let mut procs = state.procs.lock().await;
        procs.scan()
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
        scanner.scan()
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
