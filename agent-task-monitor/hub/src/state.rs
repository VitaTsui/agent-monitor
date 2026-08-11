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
    /// 待下发的目录列举请求（上传选目录）
    pub pending_dir: VecDeque<am_core::model::DirQuery>,
    /// 待下发的文件夹操作（新建/删除/重命名）
    pub pending_fsop: VecDeque<am_core::model::FsOp>,
    /// 待下发的「现取文件」请求（网页要看 agent 输出里引用的截图）
    pub pending_file_fetch: VecDeque<am_core::model::FileFetch>,
    /// 现取结果：fetch_id → (结果, 到达时刻)。
    ///
    /// **只在内存里放一会儿**：交给等着的那个网页请求即删，没人来领的也会过期清掉
    /// （见 FETCH_RESULT_TTL_SECS）。会话内容不落我方存储是既定原则，截图同样算会话内容。
    pub file_fetch_results: HashMap<String, (am_core::model::FileFetchResult, Instant)>,
    /// 文件夹操作结果缓存：op_id → 结果（网页轮询后即读走）
    pub fsop_results: HashMap<String, am_core::model::FsOpResult>,
    /// 目录列举结果缓存：(task_id, rel) → 子目录名
    /// (task_id, rel) → (子目录, 文件)。文件用于「选择文件回填相对路径」。
    pub dir_cache: HashMap<(String, String), (Vec<String>, Vec<String>)>,
    /// 上次通知过的在线状态（钉钉推送用，边沿触发上线/离线，避免重复）
    pub notified_online: bool,
    /// 已推过「等待选择」提醒的会话 ID（边沿触发：进入 select 推一次，离开清除）
    pub select_notified: std::collections::HashSet<String>,
    /// 本次「上线」的起点：设备离线→在线边沿时刷新。用于「会话开始」推送的沉降窗口——
    /// 刚上线（含 hub 重启、客户端重启/更新）后客户端会分几次把已有会话陆续扫上来，
    /// 那不是新开会话，不能推。上线后过了沉降期，新冒出来的才算真·新会话。
    pub online_since: Instant,
    /// 会话基线：id → 最近一次快照。用于「会话开始/结束」的稳定判定，避免配对振荡
    /// （同一进程在两个会话文件间来回抖）时同一会话反复推开始/结束。
    pub known_sessions: HashMap<String, Task>,
    /// 会话 id → 最近一次在上报里出现的时刻。会话「消失」超过 FINISH_GRACE_SECS 才判结束，
    /// 抹掉一两个上报周期的抖动。
    pub session_last_seen: HashMap<String, Instant>,
    /// 会话 id → 最近一次处于「等待选择」的时刻。等待用户选择时会话仍活着，但配对可能抖动、
    /// 消息被清，若按「消失=结束/重现=开始」处理会误推。此窗口内不推该会话的开始/结束。
    pub last_select_at: HashMap<String, Instant>,
    /// 待推「会话开始」的新会话：id → (首次见到该终端锚的时刻, 那个锚)。
    /// 见 NEW_SESSION_PAIR_SETTLE_SECS —— 新会话要等终端锚稳定一段时间才推，免得推出去的
    /// 编号指向隔壁终端。锚一变就重新计时。
    pub new_session_pending: HashMap<String, (Instant, String)>,
    /// 该设备最近上报的配置清单。客户端每 30s 才带一次（配置几乎不动，每轮都发是浪费），
    /// 所以这里要缓存住 —— 中间轮次的 pull/push 全靠它算差异，才能每轮推进而不是 30s 一步。
    /// None = 该设备还没报过（旧客户端，或刚上线还没到第一次扫描）。
    pub config_manifest: Option<am_core::model::ConfigManifest>,
}

/// 连续多少次「绑定失效」型发送失败后判定需要重新扫码。
///
/// 不设成 1：偶发的一次不值得让用户去重扫。设成 3：这种失效一旦发生就是持续的
///（实测连挂 18 小时），三次足以与偶发区分，又不会让人白等太久。
pub const WEIXIN_SEND_FAIL_LIMIT: u32 = 3;

/// 一次性图片外链的有效期。钉钉服务器通常几秒内就来拉，2 分钟足够；
/// 留久了等于把会话截图长期挂在一个免鉴权地址上。
pub const PUB_IMAGE_TTL_SECS: u64 = 120;

/// 现取结果在 hub 内存里的最长停留：交给网页即删，没人来领的这么久后清掉。
/// 取 60s —— 网页那边是轮询取件，一两秒就该来领；留久了等于变相「落存储」。
pub const FETCH_RESULT_TTL_SECS: u64 = 60;

/// 机器离线判定阈值
pub const OFFLINE_AFTER_SECS: u64 = 10;

/// 「会话开始」推送沉降期：设备上线后这段时间内出现的会话视为「重连扫回的已有会话」，
/// 不推。客户端重启/更新后分批扫回历史会话可能持续十几秒，取 30s 留足余量。
pub const NEW_SESSION_SETTLE_SECS: u64 = 30;

/// 「会话开始」推送的**配对沉降**：新会话刚被扫到时，客户端的配对往往还没稳定 —— 权威 pin
/// 要等 claude 跑起工具才抓得到（env 挂在临时子进程上），在那之前只能靠 mtime 启发式，同时
/// 开的两个终端极易配错。实测：给 9 号下发后收到的「会话开始」写着 #11，几十秒后配对自行
/// 纠正，但推送已经发出去了。
///
/// 所以要求会话的终端锚连续这么久不变，才认为配对稳定、可以推。取 15s：pin 通常在 claude
/// 开始执行后几秒内就抓到，而「会话开始」不是紧急通知，晚十几秒无妨。
pub const NEW_SESSION_PAIR_SETTLE_SECS: u64 = 15;

/// 「会话已结束」去抖：会话从上报里消失后，要连续消失这么久才判真结束再推。
/// 配对振荡（/clear 后进程在新旧会话文件间来回抖）通常几秒内自愈，取 20s 覆盖。
pub const FINISH_GRACE_SECS: u64 = 20;

/// 「等待选择」保护窗：会话最近这段时间内出现过等待选择态时，其消失/重现不推开始/结束
/// （用户可能在慢慢选，会话仍活着，只是配对抖动）。取 30 分钟，够长时间挂着待选。
pub const SELECT_PROTECT_SECS: u64 = 1800;

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
    /// 钉钉机器人配置有变，Stream 循环该立刻重扫了。
    /// 没有它就要等下一轮 30s 轮询 —— 用户刚保存完就会去钉钉发消息试，
    /// 半分钟没反应只会以为自己配错了。
    pub dingtalk_reload: std::sync::Arc<tokio::sync::Notify>,
    /// 同上，微信机器人扫码绑定/解绑后叫醒长轮询循环
    pub weixin_reload: std::sync::Arc<tokio::sync::Notify>,
    /// 微信推送失败后攒下的通知（账号 → 正文），等用户下次开口时补发。
    ///
    /// 发送失败有两种，别混为一谈（这一条是拿 18 小时的线上故障换来的）：
    /// - **临时性**：网络抖动之类，下次就好；
    /// - **绑定失效**：一律 `-2 prepare failed`，**不会自愈、也与 context_token 新旧无关**
    ///   —— 实测拿刚收到的、几秒钟前的凭据发送照样失败，只有重新扫码才恢复。
    ///   （曾误判成「context_token 约 1.5 小时过期」：那只解释了失败的起点，
    ///   没解释「之后 18 小时、期间反复收到新消息也从未恢复」。取证不足。）
    ///
    /// 两种都不能直接丢消息 —— 丢了就是「任务完成了但你永远不知道」。
    pub weixin_pending_pushes: RwLock<HashMap<String, Vec<String>>>,
    /// 连续「绑定失效」型发送失败的次数（账号 → 次数）。攒够就判定要重扫，
    /// 见 WEIXIN_SEND_FAIL_LIMIT。发送成功即清零。
    pub weixin_send_fails: RwLock<HashMap<String, u32>>,
    /// 一次性图片外链：token → (内容, MIME, 放入时刻)。
    ///
    /// **只为钉钉存在**：它的 `sampleImageMsg` 只认公网 URL —— 图片要由**钉钉的
    /// 服务器**来拉，那台机器带不了我们的登录态，所以这个地址必然免鉴权。
    /// 三重收窄：高熵随机 token（猜不出）、**取走即删**（一次性）、
    /// 到期自动清（见 PUB_IMAGE_TTL_SECS）。全程只在内存，不落盘。
    pub pub_images: RwLock<HashMap<String, (Vec<u8>, String, Instant)>>,
    pub started_at: chrono::DateTime<chrono::Local>,
    /// 会话表有未落盘变更（tick 循环定期 flush 到 sessions.json）
    pub sessions_dirty: std::sync::atomic::AtomicBool,
    /// 会话历史：每个会话结束时留一条最终产出（见 crate::history）。全用户合用一张表，
    /// 查询时按 owner 过滤。
    pub history: RwLock<Vec<crate::history::HistoryEntry>>,
    /// 历史有未落盘变更（tick 循环定期 flush 到 history.json）
    pub history_dirty: std::sync::atomic::AtomicBool,
    /// 机器人会话号位（「@2 / 发 2 / 暂停 2」里的 2）：用户名 → 号位表。
    /// 号绑定终端窗口而非列表位置，跨排序变化与 hub 重启都不变 —— 见 crate::slots。
    pub bot_slots: RwLock<HashMap<String, crate::slots::SlotTable>>,
    /// 号位表有未落盘变更（tick 循环定期 flush 到 bot_slots.json）
    pub bot_slots_dirty: std::sync::atomic::AtomicBool,
    /// 机器人「监控中」的会话：用户名 → 监控态。后台循环据此把新内容推到钉钉会话 webhook。
    /// 钉钉「监控」：user → 其监控中的多个会话（每会话一份）。支持同时监控多个、单独停止。
    pub bot_monitors: RwLock<HashMap<String, Vec<BotMonitor>>>,
    /// 连续对话「待确认」的内容：用户名 → (原文, 暂存时刻秒)。
    /// 锁定的会话冷却后（久未对话），第一条不带 `@` 的消息不直接下发，先回一句确认、把内容
    /// 存在这里；用户回「确认」就发它，省得重打一遍。短期数据，不落盘。
    pub bot_sticky_pending: RwLock<HashMap<String, (String, u64)>>,
    /// 配置同步基线：账号 → 其「配置源」设备的在管配置。见 crate::configsync。
    pub configs: RwLock<crate::configsync::ConfigStore>,
    /// 钉钉「挂起待发」的文件：用户名 → 待随下一条任务一起发的文件。
    /// 用户先发文件（或图文一起发图片）→ 暂存于此 → 下一条发任务的指令把它落到会话 tmp 目录、
    /// 并把相对路径回填到任务文字开头。
    pub bot_pending_files: RwLock<HashMap<String, Vec<BotPendingFile>>>,
    /// 钉钉「合并窗口」缓冲：用户名 → 正在攒的一批内容。
    ///
    /// 逐条转发多条消息时，钉钉给的是几次完全独立的回调 —— payload 里没有转发标记、没有批次
    /// 号、更没有「共 N 条」，hub 无从知道一批到底有几条。唯一能用的判据是时间：转发是连着
    /// 到的。于是内容类消息先不下发，攒在这里，静默满 [`BOT_BATCH_WINDOW_MS`] 才拼成一段
    /// 一次性发出 —— 否则 agent 看到第一条就开跑了，后面几条全变成打断。
    pub bot_pending_batch: RwLock<HashMap<String, BotBatch>>,
    /// 钉钉绑定 —— 两个方向，都是一次性、都会过期：
    ///
    /// · `dingtalk_binds`：**钉钉那头先开口**。陌生 staffId 给全局机器人发消息，
    ///   hub 回一条带 token 的登录链接；用户登录后带 token 来认领，token → 待绑的钉钉号。
    /// · `dingtalk_bind_codes`：**网页这头先开口**。用户在「机器人管理」取一个绑定码
    ///   （扫码或手输），到钉钉里发「绑定 <码>」；码 → 待绑的账号。
    ///
    /// 两条路解决的是同一件事在不同起点：人在电脑前就用码，人在手机上就用链接。
    pub dingtalk_binds: RwLock<HashMap<String, PendingDingtalkBind>>,
    pub dingtalk_bind_codes: RwLock<HashMap<String, PendingBindCode>>,
}

/// 钉钉合并窗口：多久没有新消息就认为这一批发完了（毫秒）。
///
/// 逐条转发时消息通常几百毫秒一条，3s 足够兜住手抖的间隔；再长就开始伤害正常单条对话的
/// 体感了（每条内容都要等满这个窗口才真正下发）。
pub const BOT_BATCH_WINDOW_MS: u64 = 3_000;

/// 钉钉合并窗口里攒着的一批内容
pub struct BotBatch {
    /// 按到达顺序攒下的原文，flush 时用 `\n` 拼成一段
    pub lines: Vec<String>,
    /// 最后一条消息的回执地址：合并后只回一次，用最新的那个（窗口 3s 远短于 webhook 有效期）
    pub webhook: String,
    pub expiry_ms: u64,
    pub staff_id: String,
    pub robot_code: String,
    /// 世代号：每来一条消息 +1。到点的延时任务比对它，认出自己是否已被后续消息取代 ——
    /// 取代了就静默退场，由最后那条消息起的任务负责 flush（这就是「静默窗口重置」）。
    pub gen: u64,
}

/// 钉钉待绑定上下文（一次性 token 指向它）
#[derive(Clone)]
pub struct PendingDingtalkBind {
    /// 待绑定的钉钉 staffId
    pub staff_id: String,
    /// 钉钉昵称（供界面显示）
    pub nick: String,
    /// 生成时刻（秒），用于过期清理
    pub at: u64,
}

/// 绑定码：网页/客户端先取码，再到钉钉里发「绑定 <码>」认领
#[derive(Clone)]
pub struct PendingBindCode {
    /// 取码的账号 —— 谁取的码，钉钉号就绑给谁
    pub user: String,
    /// 生成时刻（秒）
    pub at: u64,
}

/// 生成一次性绑定 token（不可猜）
pub fn new_bind_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 生成绑定码：6 位、避开易混淆字符（0/O、1/I/L），方便手输与扫码识别
pub fn new_bind_code() -> String {
    use rand::Rng;
    const ALPHA: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..6).map(|_| ALPHA[rng.gen_range(0..ALPHA.len())] as char).collect()
}

/// 钉钉挂起的待发文件（downloadCode 换取下载地址，随下一条任务发出时才真正下载+下发）
#[derive(Clone)]
pub struct BotPendingFile {
    pub download_code: String,
    pub file_name: String,
    /// 该文件经由哪个账号的钉钉应用收到（下载要用它的凭据；多租户下可能 != 归属账号）
    pub app_user: String,
    /// 收到时刻（秒），用于过期清理
    pub at: u64,
    /// 微信：收到时就已下载并解密好的内容。
    ///
    /// 不像钉钉那样延后取，是因为微信的下载直链带一次性参数、会过期，且解密要用
    /// 随消息一起来的 aeskey —— 等到下发任务时再去取很可能已经拿不到了，
    /// 而那时报错，人早就忘了自己发过图。收到即取，失败当场就能告诉他。
    /// None = 走 `download_code` 那条（钉钉）。
    pub bytes: Option<Vec<u8>>,
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
        // 号位持久化：hub 重启后「@2」还是同一个终端（重启丢表正是序号错位的成因之一）
        let slots = crate::slots::load(&config.data_dir);
        // 会话历史持久化：hub 重启后仍能回看之前派出去的活的结果
        let history = crate::history::load(&config.data_dir);
        // 配置基线持久化：hub 重启后镜像机不必等源机重传一遍全部配置
        let configs = crate::configsync::ConfigStore::load(&config.data_dir);
        Arc::new(Self {
            machines: RwLock::new(HashMap::new()),
            tokens: RwLock::new(sessions),
            config,
            registry: RwLock::new(registry),
            pair_codes: RwLock::new(HashMap::new()),
            oauth_states: RwLock::new(HashMap::new()),
            login_throttle: RwLock::new(LoginThrottle::default()),
            tx,
            dingtalk_reload: std::sync::Arc::new(tokio::sync::Notify::new()),
            weixin_reload: std::sync::Arc::new(tokio::sync::Notify::new()),
            weixin_pending_pushes: RwLock::new(HashMap::new()),
            weixin_send_fails: RwLock::new(HashMap::new()),
            pub_images: RwLock::new(HashMap::new()),
            dingtalk_binds: RwLock::new(HashMap::new()),
            dingtalk_bind_codes: RwLock::new(HashMap::new()),
            started_at: chrono::Local::now(),
            sessions_dirty: std::sync::atomic::AtomicBool::new(false),
            history: RwLock::new(history),
            history_dirty: std::sync::atomic::AtomicBool::new(false),
            bot_slots: RwLock::new(slots),
            bot_slots_dirty: std::sync::atomic::AtomicBool::new(false),
            bot_monitors: RwLock::new(HashMap::new()),
            bot_sticky_pending: RwLock::new(HashMap::new()),
            bot_pending_files: RwLock::new(HashMap::new()),
            bot_pending_batch: RwLock::new(HashMap::new()),
            configs: RwLock::new(configs),
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
                    // 与侧栏设备计数一致：只数活跃会话（非 Finished），否则 7 天窗口里
                    // 堆积的已结束会话会把「会话数」撑到几十条，跟左侧对不上。
                    session_count: e
                        .tasks
                        .iter()
                        .filter(|t| t.status != TaskStatus::Finished)
                        .count(),
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
                session_count: live
                    .map(|e| e.tasks.iter().filter(|t| t.status != TaskStatus::Finished).count())
                    .unwrap_or(0),
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
        // 固定名安装包对齐（~60s 一次）：发版只上传版本化的包，客户端自更新却固定去下
        // agent-monitor-setup.exe。不对齐的话客户端会把旧包装了又装（见 sync_fixed_installer）。
        // 放 spawn_blocking：拷 11MB 是同步文件 IO，别占着 tick 所在的 worker。
        if tick % 40 == 0 {
            let dir = std::env::var("AM_DOWNLOADS_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| state.config.data_dir.join("downloads"));
            if dir.is_dir() {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::server::sync_fixed_installer(&dir)
                })
                .await;
            }
        }
        // 机器人号位同上：分配/回收只标脏，这里统一写（丢一轮也只是号位重排一次）
        if tick % 40 == 0 && state.bot_slots_dirty.swap(false, Ordering::Relaxed) {
            crate::slots::save(&state).await;
        }
        // 交互历史同上。它是「人不在电脑前时唯一能回看的记录」，重启就丢等于没有，
        // 所以这里跟号位同频落盘（~60s），丢的最多是最后一分钟的几条。
        if tick % 40 == 0 && state.history_dirty.swap(false, Ordering::Relaxed) {
            crate::history::save(&state).await;
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
                                text: format!("**🔴 设备离线**\n\n**设备**：{}", m.hostname),
                                full_content: None,
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
