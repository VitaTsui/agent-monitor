//! hub 服务端状态：机器聚合、登录态、注册表、配对码。
//! 不含任何本机扫描/托盘/上报（那些在 am-client）。
use am_core::model::{ControlCmd, MachineInfo, MessageBrief, Task, TaskStatus};
use crate::registry::Registry;
use rsa::RsaPrivateKey;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};

pub struct Config {
    pub port: u16,
    /// 与前端 .env CRYPTO_KEY 一致的共享 AES 密钥
    pub crypto_key: String,
    pub private_key: RsaPrivateKey,
    /// 数据目录（~/.agent-monitor）
    pub data_dir: std::path::PathBuf,
    /// 后管访问令牌（部署时生成，X-Admin-Token 头校验）
    pub admin_token: String,
    /// 全局 agent 上报令牌（内部部署/兼容通道；普通用户走每设备令牌）
    pub agent_token: String,
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
    pub pending_files: VecDeque<am_core::model::FileTransfer>,
    /// 会话 ID → 最近消息（agent 上报时缓存，供前端查看远程会话）
    pub messages: HashMap<String, Vec<MessageBrief>>,
    /// 待下发给该 agent 的 git 对比请求
    pub pending_git: VecDeque<am_core::model::GitQuery>,
    /// 待下发的目录列举请求（上传选目录）
    pub pending_dir: VecDeque<am_core::model::DirQuery>,
    /// 待下发的文件夹操作（新建/删除/重命名）
    pub pending_fsop: VecDeque<am_core::model::FsOp>,
    /// 文件夹操作结果缓存：op_id → 结果（网页轮询后即读走）
    pub fsop_results: HashMap<String, am_core::model::FsOpResult>,
    /// 目录列举结果缓存：(task_id, rel) → 子目录名
    /// (task_id, rel) → (子目录, 文件)。文件用于「选择文件回填相对路径」。
    pub dir_cache: HashMap<(String, String), (Vec<String>, Vec<String>)>,
    /// 会话 ID → 最近一次 git 对比结果（agent 回传后缓存）
    pub git_cache: HashMap<String, am_core::model::GitOverview>,
    /// 上次通知过的在线状态（钉钉推送用，边沿触发上线/离线，避免重复）
    pub notified_online: bool,
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

/// 登录态有效期：30 天**滑动**窗口 —— 只要期间有任何活动就自动顺延，
/// 体验对齐 Claude / ChatGPT：常用用户永不掉线，闲置一个月才需重登。
/// （仍要有期限：泄露的 token 不能永久有效，token 表也不能只增不减。）
pub const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;

/// 活动续期节流：距上次续期超过 1 小时才写一次 last_seen，
/// 避免每个请求都抢 tokens 写锁 / 反复触发落盘。
pub const SESSION_TOUCH_SECS: u64 = 3600;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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

/// 设备配对条目：客户端 pair/start 创建，网页 pair/claim 绑定，客户端 pair/status 领走
#[derive(Clone)]
pub struct PairEntry {
    pub machine_id: String,
    pub hostname: String,
    pub platform: String,
    /// 客户端持有的轮询凭证（防他人凭 code 轮询窃取设备令牌）
    pub pair_token: String,
    pub created: Instant,
    /// 认领后写入：待客户端领取的每设备上报令牌
    pub device_token: Option<String>,
}

impl PairEntry {
    /// 配对码有效期：10 分钟（够用户完成登录/注册）
    pub fn expired(&self) -> bool {
        self.created.elapsed().as_secs() > 600
    }
}

/// 一次登录签发的会话。
/// 用墙钟（unix 秒）而非 Instant：会话要持久化到磁盘，服务重启后
/// 网页/移动端/客户端全都不掉线。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub username: String,
    /// 最近活动时间（unix 秒），滑动过期的基准
    pub last_seen: u64,
}

impl Session {
    pub fn new(username: String) -> Self {
        Self { username, last_seen: now_secs() }
    }

    pub fn expired(&self) -> bool {
        now_secs().saturating_sub(self.last_seen) >= SESSION_TTL_SECS
    }

    /// 活动续期（滑动窗口顺延）
    pub fn touch(&mut self) {
        self.last_seen = now_secs();
    }
}

/// 从数据目录加载持久化会话（过期的直接丢弃）
pub fn load_sessions(data_dir: &std::path::Path) -> HashMap<String, Session> {
    let Ok(txt) = std::fs::read_to_string(data_dir.join("sessions.json")) else {
        return HashMap::new();
    };
    let map: HashMap<String, Session> = serde_json::from_str(&txt).unwrap_or_default();
    map.into_iter().filter(|(_, s)| !s.expired()).collect()
}

/// 会话落盘（原子写 + 仅属主可读：token 等同登录凭证）
pub async fn save_sessions(state: &SharedState) {
    let snapshot = state.tokens.read().await.clone();
    let Ok(txt) = serde_json::to_string(&snapshot) else {
        return;
    };
    let path = state.config.data_dir.join("sessions.json");
    let tmp = state.config.data_dir.join("sessions.json.tmp");
    if std::fs::write(&tmp, &txt).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    let _ = std::fs::rename(&tmp, &path);
}

pub struct AppState {
    pub config: Config,
    /// 全部上报机器（key = machine_id）
    pub machines: RwLock<HashMap<String, MachineEntry>>,
    /// 已签发的登录 token → 会话
    pub tokens: RwLock<HashMap<String, Session>>,
    /// 用户 + 设备信任注册表（持久化）
    pub registry: RwLock<Registry>,
    /// 设备配对：code → 配对条目
    pub pair_codes: RwLock<HashMap<String, PairEntry>>,
    /// OAuth state 防 CSRF
    pub oauth_states: RwLock<HashMap<String, Instant>>,
    /// 口令登录节流
    pub login_throttle: RwLock<LoginThrottle>,
    /// 快照变更信号（tick 循环自增，WS 据此推送）
    pub tx: broadcast::Sender<u64>,
    pub started_at: chrono::DateTime<chrono::Local>,
    /// 会话表有未落盘变更（tick 循环定期 flush 到 sessions.json）
    pub sessions_dirty: std::sync::atomic::AtomicBool,
    /// 机器人「最近一次列出的会话」：用户名 → 有序 task_id，
    /// 让「暂停 3」这类按序号操作能对上会话。
    pub bot_last_list: RwLock<HashMap<String, Vec<String>>>,
    /// 机器人「监控中」的会话：用户名 → 监控态。后台循环据此把新内容推到钉钉会话 webhook。
    pub bot_monitors: RwLock<HashMap<String, BotMonitor>>,
}

/// 机器人持续监控一个会话的状态（通过钉钉会话级 webhook 推送新内容）
#[derive(Clone)]
pub struct BotMonitor {
    pub task_id: String,
    /// 钉钉会话 webhook（回消息用的临时地址，可主动 POST 推送）
    pub webhook: String,
    /// webhook 失效时间(ms)；超过则停止监控（0=未知，不因此停）
    pub expiry_ms: u64,
    /// 已推送到的最后一条消息时间戳（算增量用）
    pub last_ts: String,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new(config: Config, registry: Registry) -> SharedState {
        let (tx, _) = broadcast::channel(64);
        // 会话持久化：重启不掉线（网页 / 移动端 / 客户端一体生效）
        let sessions = load_sessions(&config.data_dir);
        Arc::new(Self {
            machines: RwLock::new(HashMap::new()),
            tokens: RwLock::new(sessions),
            config,
            registry: RwLock::new(registry),
            pair_codes: RwLock::new(HashMap::new()),
            oauth_states: RwLock::new(HashMap::new()),
            login_throttle: RwLock::new(LoginThrottle::default()),
            tx,
            started_at: chrono::Local::now(),
            sessions_dirty: std::sync::atomic::AtomicBool::new(false),
            bot_last_list: RwLock::new(HashMap::new()),
            bot_monitors: RwLock::new(HashMap::new()),
        })
    }

    /// 取某会话最近缓存的消息（在各机器的 messages 缓存里找第一个命中的）
    pub async fn bot_task_messages(&self, task_id: &str) -> Vec<am_core::model::MessageBrief> {
        let machines = self.machines.read().await;
        for entry in machines.values() {
            if let Some(msgs) = entry.messages.get(task_id) {
                return msgs.clone();
            }
        }
        Vec::new()
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
                    platform_dsr: am_core::model::platform_dsr(&e.platform),
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
                    shared: false,
                }
            })
            .collect();
        // 注册表兜底：hub 重启后实时表是空的，未在上报的设备（关机/客户端未开）
        // 也必须留在列表里 —— 否则设备会随每次发版「凭空消失」，
        // 用户既看不到它、也无法对它撤销信任或删除。
        for (id, meta) in registry.devices_of(username) {
            if machines.contains_key(&id) {
                continue;
            }
            let last = (meta.last_seen > 0).then(|| {
                chrono::DateTime::from_timestamp(meta.last_seen as i64, 0)
                    .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default()
            });
            out.push(MachineInfo {
                hostname: if meta.hostname.is_empty() { id.clone() } else { meta.hostname.clone() },
                platform_dsr: am_core::model::platform_dsr(&meta.platform),
                platform: meta.platform.clone(),
                version: meta.version.clone(),
                online: false,
                is_hub: false,
                last_report_at: last,
                session_count: 0,
                running_count: 0,
                owner: meta.owner.clone(),
                trusted: meta.trusted,
                shared: false,
                id,
            });
        }
        // 协助码共享给我的（他人）设备：作为只读+可控条目并入列表
        let mine: std::collections::HashSet<String> = out.iter().map(|m| m.id.clone()).collect();
        for (id, meta) in registry.shared_to(username) {
            if mine.contains(&id) {
                continue;
            }
            let live = machines.get(&id);
            let online = live
                .map(|e| e.last_report.elapsed().as_secs() < OFFLINE_AFTER_SECS)
                .unwrap_or(false);
            out.push(MachineInfo {
                hostname: if meta.hostname.is_empty() { id.clone() } else { meta.hostname.clone() },
                platform_dsr: am_core::model::platform_dsr(&meta.platform),
                platform: meta.platform.clone(),
                version: meta.version.clone(),
                online,
                is_hub: false,
                last_report_at: None,
                session_count: live.map(|e| e.tasks.len()).unwrap_or(0),
                running_count: live
                    .map(|e| e.tasks.iter().filter(|t| online && t.status == TaskStatus::Running).count())
                    .unwrap_or(0),
                owner: meta.owner.clone(),
                trusted: true,
                shared: true,
                id,
            });
        }
        out.sort_by(|a, b| {
            b.is_hub
                .cmp(&a.is_hub)
                .then(a.trusted.cmp(&b.trusted))
                .then(a.hostname.cmp(&b.hostname))
        });
        out
    }
}

/// tick 循环：驱动 WS 推送节奏 + 定期清扫过期登录态/配对码。
/// （hub 不自监控 —— 服务器不是被监控设备；机器数据全部来自上报。）
pub async fn tick_loop(state: SharedState) {
    use std::sync::atomic::Ordering;
    let mut tick: u64 = 0;
    loop {
        tick = tick.wrapping_add(1);
        if tick % 400 == 0 {
            {
                let mut map = state.tokens.write().await;
                let before = map.len();
                map.retain(|_, s| !s.expired());
                if map.len() != before {
                    state.sessions_dirty.store(true, Ordering::Relaxed);
                }
            }
            state.pair_codes.write().await.retain(|_, e| !e.expired());
        }
        // 会话变更定期落盘（~60s 一次）：登录/登出/活动续期都只标脏，这里统一写
        if tick % 40 == 0 && state.sessions_dirty.swap(false, Ordering::Relaxed) {
            save_sessions(&state).await;
        }
        // 设备离线边沿检测（每 ~3s）：曾在线、现超阈值未上报 → 推「离线」
        if tick % 2 == 0 {
            let mut offline_events = Vec::new();
            {
                let reg = state.registry.read().await;
                let mut machines = state.machines.write().await;
                for (id, m) in machines.iter_mut() {
                    if m.notified_online && m.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
                        m.notified_online = false;
                        if let Some(owner) = reg.device_meta(id).owner {
                            offline_events.push(crate::dingtalk::NotifyEvent {
                                owner,
                                kind: crate::dingtalk::EventKind::Device,
                                task_id: None,
                                text: format!("### 🔴 设备离线\n- **设备**：{}", m.hostname),
                            });
                        }
                    }
                }
            }
            if !offline_events.is_empty() {
                let st = state.clone();
                let now_ms = now_secs() * 1000;
                tokio::spawn(async move { crate::dingtalk::deliver(&st, offline_events, now_ms).await });
            }
        }
        let _ = state.tx.send(tick);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}
