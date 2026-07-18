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
    /// 会话 ID → 最近一次 git 对比结果（agent 回传后缓存）
    pub git_cache: HashMap<String, am_core::model::GitOverview>,
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
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new(config: Config, registry: Registry) -> SharedState {
        let (tx, _) = broadcast::channel(64);
        Arc::new(Self {
            config,
            machines: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
            registry: RwLock::new(registry),
            pair_codes: RwLock::new(HashMap::new()),
            oauth_states: RwLock::new(HashMap::new()),
            login_throttle: RwLock::new(LoginThrottle::default()),
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

/// tick 循环：驱动 WS 推送节奏 + 定期清扫过期登录态/配对码。
/// （hub 不自监控 —— 服务器不是被监控设备；机器数据全部来自上报。）
pub async fn tick_loop(state: SharedState) {
    let mut tick: u64 = 0;
    loop {
        tick = tick.wrapping_add(1);
        if tick % 400 == 0 {
            state.tokens.write().await.retain(|_, s| !s.expired());
            state.pair_codes.write().await.retain(|_, e| !e.expired());
        }
        let _ = state.tx.send(tick);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}
