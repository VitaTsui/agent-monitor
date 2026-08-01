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
    safe_upload_dir_within(dir, &[])
}

/// 同 `safe_upload_dir`，但除了 `upload_root()`，还额外允许写进 `extra_roots` 里的任一目录
/// （传入本机活跃会话的项目 cwd）。原因：文件选目录弹窗是**相对会话 cwd**浏览的，项目常不在
/// 家目录下；只按家目录校验会把「浏览进项目子目录再上传」这种合法操作误拒——表现为网页提示
/// 上传成功、终端里却找不到文件。会话 cwd 是本机已在监控的合法目录，放行是安全的。
pub fn safe_upload_dir_within(
    dir: &str,
    extra_roots: &[std::path::PathBuf],
) -> Result<std::path::PathBuf, String> {
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
    let allowed = out.starts_with(&root)
        || extra_roots
            .iter()
            .any(|r| !r.as_os_str().is_empty() && out.starts_with(r));
    if !allowed {
        return Err(format!(
            "目标目录超出允许范围（仅允许 {} 或活跃会话的项目目录之内）",
            root.display()
        ));
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

/// hook 自报记录的有效期。取 12 小时：hook 只在 claude 起会话/调工具时写一次，之后即使
/// 会话空闲一整天，那条配对仍然成立（进程还在，pid+start 校验会兜住 pid 重用）。
/// 太短反而会让长时间挂着的会话失去最权威的配对信号、退回启发式。
const HOOK_REPORT_TTL_SECS: u64 = 12 * 3600;

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

/// 持久化配对缓存的文件：**终端锚(shell pid) → session_id**。锚在终端 shell 上而非易变的
/// claude pid：claude 经 /clear、--resume、客户端/进程重启会换 pid，但它所在终端的 shell pid
/// 不变。据此在重启后把「该终端现在的 claude」接回上次的会话，避免落到 tier④ mtime 启发式把
/// 并发同项目会话交叉错配（退出客户端再重开时的下发错位）。终端锚取自 ProcessInfo.shell_pid。
fn pairs_file(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("session-pairs.json")
}

/// 进程的「终端锚身份」= (锚 pid, 锚 start)：有 shell 祖先则 (shell_pid, shell_start)，否则
/// 回退 (claude 自身 pid, claude start_time)。start 一并进 key 用于区分「同一个 shell」与
/// 「pid 被重用的新 shell/进程」——Windows 会重用 pid（关掉终端再开一个可能拿到同一 shell pid），
/// 光比 pid 会把新终端错配到旧会话。缓存/恢复都以「pid + start 都对上」为准。
fn anchor_key(p: &am_core::model::ProcessInfo) -> (u32, u64) {
    match (p.shell_pid, p.shell_start) {
        (Some(sp), Some(ss)) => (sp, ss),
        _ => (p.pid, p.start_time),
    }
}

/// 写盘：内存缓存「终端锚 → (session, 锚 start)」，直接落盘（锚 start 一并存，恢复时防 pid 重用）。
fn save_anchor_pairs(
    data_dir: &std::path::Path,
    anchors: &std::collections::HashMap<u32, (String, u64)>,
) {
    let mut obj = serde_json::Map::new();
    for (anchor, (sid, sstart)) in anchors {
        obj.insert(anchor.to_string(), serde_json::json!({ "sid": sid, "sstart": sstart }));
    }
    let _ = std::fs::write(pairs_file(data_dir), serde_json::Value::Object(obj).to_string());
}

/// 把「终端锚 → (session, 锚 start)」翻译成「当前该终端下的**主 claude** pid → session」。
/// 每个活进程取其终端锚身份 (pid, start)，命中锚表且 **start 也对上** 才算——claude 换 pid
/// (/clear、--resume、重启) 后仍在同一 shell 下即接回；死终端/被重用的 pid（start 不同）翻不出
/// 配对，防 pid 重用把新终端错配到旧会话。
///
/// 同一终端锚可能有**多个 claude**：主 claude 与它用 Task 工具派生的子 agent（子 agent 是主
/// claude 的子进程、同一 shell 的孙进程，终端锚相同）。故同一锚**只认最早启动的那个** = 主
/// claude（子 agent 总在会话进行中才派生、启动更晚），免得会话被配到子 agent。
fn translate_anchors(
    anchors: &std::collections::HashMap<u32, (String, u64)>,
    procs: &[am_core::model::ProcessInfo],
) -> std::collections::HashMap<u32, String> {
    let mut main_of: std::collections::HashMap<u32, &am_core::model::ProcessInfo> =
        std::collections::HashMap::new();
    for p in procs {
        let (apid, astart) = anchor_key(p);
        // 锚 pid 命中、且 start 也对上（同一个 shell/进程，非 pid 重用）
        match anchors.get(&apid) {
            Some((_, s)) if *s == astart => {}
            _ => continue,
        }
        match main_of.get(&apid) {
            Some(cur) if cur.start_time <= p.start_time => {} // 已有更早启动的，保留
            _ => {
                main_of.insert(apid, p);
            }
        }
    }
    main_of
        .into_iter()
        .map(|(apid, p)| (p.pid, anchors[&apid].0.clone()))
        .collect()
}

/// 读盘：读回「终端锚 → (session, 锚 start)」。校验推迟到每轮翻译（pid + start 都对上才配），
/// 故这里无需活进程；死终端/被重用的 pid 翻不出配对。缺 sstart 的旧格式按 0 处理（不会误配）。
fn load_anchor_pairs(
    data_dir: &std::path::Path,
) -> std::collections::HashMap<u32, (String, u64)> {
    let mut out = std::collections::HashMap::new();
    let Ok(txt) = std::fs::read_to_string(pairs_file(data_dir)) else {
        return out;
    };
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return out;
    };
    for (anchor_s, ent) in &obj {
        if let (Ok(anchor), Some(sid)) =
            (anchor_s.parse::<u32>(), ent.get("sid").and_then(|x| x.as_str()))
        {
            let sstart = ent.get("sstart").and_then(|x| x.as_u64()).unwrap_or(0);
            out.insert(anchor, (sid.to_string(), sstart));
        }
    }
    out
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
                    let cmd: String = p.command.chars().take(90).collect();
                    client_log(&format!(
                        "[pair] procstart pid={} agent={} start_age={}s cmd={}",
                        p.pid, p.agent, pstart_age, cmd
                    ));
                }
            }
        }
    }
    let paused = state.paused.read().await.clone();
    // 「claude pid → 会话 id」的**权威**配对累积表。
    //
    // pin 是**瞬态信号**：CLAUDE_PID / CLAUDE_CODE_SESSION_ID 只出现在 claude 临时派生的工具
    // 子进程（bash / cargo / conhost…）的 env 里，跑完就没了；常驻的 cmd.exe 子进程并不带这
    // 两个变量。所以空闲等待输入的 claude 一个 candidate 都抓不到（实测同机 5 个 claude，只
    // 有正在执行工具的那个有）。
    //
    // 但「这个 claude 属于哪个会话」是**持久事实** —— 抓到一次就该一直有效。早先这里用本轮
    // 结果直接覆盖缓存：claude 一进入空闲 fresh 就是空的，把已抓到的配对一起清掉 → 退回
    // mtime 启发式 → 会话串到别的终端（钉钉里标题错位、下发打错终端的根因）。
    // 改为累积：只增不删，失效只由「进程是否还是原来那个」决定。
    // 「终端此刻正等你选」：由 PreToolUse hook 在 AskUserQuestion 执行前落下，
    // 下面读 hook 记录时顺带收上来（session_id → AskUserQuestion 的 input JSON），
    // 扫描完再回填到对应会话上报出去。
    let mut pending_selects: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let pinned = {
        use std::sync::atomic::Ordering;
        // 值 = (会话 id, 该 claude 的启动时间)。带 start_time 是为了防 pid 重用 —— 进程退出后
        // pid 被别的进程占用时启动时间对不上，条目立即失效（与终端锚 shell_start 同一思路）。
        static PIN_ACC: std::sync::Mutex<
            Option<std::collections::HashMap<u32, (String, u64)>>,
        > = std::sync::Mutex::new(None);
        let tick = SCAN_TICKS.load(Ordering::Relaxed);
        // 本轮扫描到的活进程身份（pid → 启动时间），用来淘汰失效条目
        let alive: std::collections::HashMap<u32, u64> =
            processes.iter().map(|p| (p.pid, p.start_time)).collect();
        // /clear 会让同一个 claude 换到新会话，但换会话本身**不产生新 pin**（除非它随后又跑了
        // 工具）。累积表若还记着旧 sid，会以「第一优先」(scanner::build_tasks) 把进程粘回清空前
        // 的会话、压过 clear-follow 迁移层 —— 表现为网页/钉钉的标题内容定格在 /clear 之前。
        // 判据同 clear-follow：同项目里出现了 created 更晚的 cleared 会话，就说明这条 pin 过期，
        // 丢弃它、把配对交还给迁移层。查不到该会话时保守保留（信息不足，且 build_tasks 找不到
        // sid 本来也不会用它）。
        let sess_by_id: std::collections::HashMap<&str, &am_core::scanner::SessionSummary> =
            sessions.iter().map(|s| (s.session_id.as_str(), s)).collect();
        let superseded_by_clear = |sid: &str| -> bool {
            let Some(cur) = sess_by_id.get(sid) else { return false };
            sessions.iter().any(|s| {
                s.cleared
                    && s.provider == cur.provider
                    && s.project_key == cur.project_key
                    && s.created_ms > cur.created_ms
            })
        };
        let mut guard = PIN_ACC.lock().unwrap();
        let acc = guard.get_or_insert_with(std::collections::HashMap::new);
        // 每轮淘汰（很便宜）：进程已退出 / pid 被重用 / 会话已被 /clear 取代
        acc.retain(|pid, (sid, start)| {
            alive.get(pid) == Some(&*start) && !superseded_by_clear(sid)
        });
        // 采集节奏分两路，因为两个来源的开销差着数量级：
        //
        // · env 扫描便宜（实测 449 个进程 ~50ms），而 pin 是**瞬态**的 —— 只在 claude 执行工具
        //   的那几秒有子进程可抓。所以只要还有 claude 没配上，就**每轮**扫（约 1.5s 一轮，占用
        //   ~3%），尽量抓住那个窗口；一次抓到就永久钉死，之后自然降频。实测教训：原先每 20 轮
        //   （~30s）才扫一次，撞上工具执行窗口的概率太低，客户端重启后累积表长时间填不上、
        //   `pinned=0` 持续，等于修复没生效。
        // · 文件句柄扫描（Unix lsof；Windows 恒空）很贵，保持每 20 轮。
        //
        // 首轮（tick==1，诊断块已 fetch_add 过所以从 1 起）两路都跑：否则启动后那段时间只能靠
        // mtime 启发式（易错位），正是「刚开客户端就下发」最容易配错的窗口。
        let unpaired = processes.iter().any(|p| !acc.contains_key(&p.pid));
        let maintain = tick == 1 || tick % 20 == 0;
        let mut env_n = 0usize;
        let mut file_n = 0usize;
        let mut added = 0usize;
        // 并入累积表：只收当前存活进程的 pin，顺手记下启动时间当身份。
        // 同一 pid 再次抓到就以最新为准（同一进程换会话 = /clear 后新建了会话）。
        let merge = |acc: &mut std::collections::HashMap<u32, (String, u64)>,
                         pins: std::collections::HashMap<u32, String>,
                         added: &mut usize| {
            for (pid, sid) in pins {
                let Some(&start) = alive.get(&pid) else { continue };
                if acc.insert(pid, (sid, start)).is_none() {
                    *added += 1;
                }
            }
        };
        // **最高优先：hook 自报**。Claude Code 的 hook 在 stdin 里直接给出 session_id，由
        // `am-client hook` 落到 <data_dir>/hooks/<pid>.json（见 hookrec）。这条不用碰运气 ——
        // env pin 只在 claude 正跑工具时存在，而 hook 是 claude 主动报的，空闲会话照样有。
        // 每轮都读（就是列一个小目录，比 env 扫描还便宜），并覆盖其它来源：它最权威。
        let hook_reports = crate::hookrec::read_reports(&state.config.data_dir, HOOK_REPORT_TTL_SECS);
        let hook_n = hook_reports.len();
        for r in hook_reports {
            // 只认当前存活的进程；死 pid 的记录顺手删掉，免得 pid 重用后张冠李戴
            let Some(&start) = alive.get(&r.claude_pid) else {
                crate::hookrec::drop_report(&state.config.data_dir, r.claude_pid);
                continue;
            };
            if let Some(sel) = r.pending_select {
                pending_selects.insert(r.session_id.clone(), sel);
            }
            if acc.insert(r.claude_pid, (r.session_id, start)).is_none() {
                added += 1;
            }
        }
        // 权威来源：claude 派生子进程的 env 里带 CLAUDE_PID + CLAUDE_CODE_SESSION_ID，
        // 直接给出「会话 ↔ claude pid」（见 ProcessScanner::session_pins）。这是 Windows 上
        // 唯一可靠的 pinned 来源 —— claude 写一行开一次就关，句柄扫描（RmGetList/lsof）抓不到。
        if unpaired || maintain {
            let env_pins = {
                let mut procs = state.procs.lock().await;
                tokio::task::block_in_place(|| procs.session_pins())
            };
            env_n = env_pins.len();
            merge(acc, env_pins, &mut added);
        }
        // 句柄扫描留作补充（env 优先级更高，上面先并入、这里不覆盖已有条目）
        if maintain {
            let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
            let dirs = {
                let scanner = state.scanner.lock().await;
                let home = dirs::home_dir().unwrap_or_default();
                vec![scanner.projects_dir().to_path_buf(), home.join(".codex/sessions")]
            };
            let file_pins =
                tokio::task::block_in_place(|| crate::openfiles::pin_sessions(&pids, &dirs));
            file_n = file_pins.len();
            let only_new: std::collections::HashMap<u32, String> =
                file_pins.into_iter().filter(|(pid, _)| !acc.contains_key(pid)).collect();
            merge(acc, only_new, &mut added);
        }
        // 诊断：每 ~30s 记一次。env权威=0 是常态且**不再要紧** —— 只要累积表非空，配对就仍然
        // 可靠；真正该警惕的是 pinned 长期为 0（那才会退回 mtime 启发式）。
        {
            static LAST_LOG: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut l = LAST_LOG.lock().unwrap();
            if now.saturating_sub(*l) > 30 {
                *l = now;
                client_log(&format!(
                    "配对来源 pinned={} 条（累积 · 本轮新增 {} · hook自报={} env权威={} 文件句柄={} · 未配对={}）：{:?}（进程数 {}）",
                    acc.len(),
                    added,
                    hook_n,
                    env_n,
                    file_n,
                    unpaired,
                    acc,
                    processes.len()
                ));
            }
        }
        // 累积表 → build_tasks 要的 pid→sid（丢掉只用于校验身份的启动时间）
        let out: std::collections::HashMap<u32, String> =
            acc.iter().map(|(pid, (sid, _))| (*pid, sid.clone())).collect();
        out
    };
    // 「本轮相比上轮 mtime 有推进」的会话 = 此刻正在被写的活跃会话。用于兜底配对时
    // 区分「正在生成输出的活跃会话」与「刚关闭、mtime 虽新但已冻结的旧会话」。
    let active_ids: std::collections::HashSet<String> = {
        static PREV_MTIMES: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> =
            std::sync::Mutex::new(None);
        let mut guard = PREV_MTIMES.lock().unwrap();
        let prev = guard.get_or_insert_with(std::collections::HashMap::new);
        let mut active = std::collections::HashSet::new();
        let mut cur = std::collections::HashMap::new();
        for s in &sessions {
            cur.insert(s.session_id.clone(), s.mtime_ms);
            if prev.get(&s.session_id).is_some_and(|&pm| s.mtime_ms > pm) {
                active.insert(s.session_id.clone());
            }
        }
        *prev = cur;
        active
    };
    // 稳定配对缓存，锚在**终端(shell pid)**上：终端锚 → session_id。喂给 build_tasks 做兜底，
    // 让长时间闲置的会话保持配对、不掉成「等待输入」占位。锚在终端而非易变的 claude pid：
    // claude 经 /clear、--resume、重启会换 pid，但所在终端 shell 不变——每轮把「终端锚→会话」
    // 翻译成「该终端现在的 claude pid → 会话」，claude 换 pid（含客户端重启）也接得回，消除
    // 空闲/并发会话落到 mtime 启发式而下发错位。这份缓存直接持久化到盘、重启读回。
    // 值 = (session_id, 锚 start)：锚 start 一并存，恢复/翻译时防 shell pid 重用把新终端错配旧会话。
    static ANCHOR_PAIRS: std::sync::Mutex<
        Option<std::collections::HashMap<u32, (String, u64)>>,
    > = std::sync::Mutex::new(None);
    static PAIRS_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // 首轮：从盘恢复终端锚→会话
    if !PAIRS_LOADED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let restored = load_anchor_pairs(&state.config.data_dir);
        if !restored.is_empty() {
            client_log(&format!("恢复配对缓存 {} 条（终端锚，客户端重启）", restored.len()));
            *ANCHOR_PAIRS.lock().unwrap() = Some(restored);
        }
    }
    // 翻译：为每个活 claude 取终端锚，命中锚缓存且锚 start 对上 → 该 claude 配上那条会话
    let cached: std::collections::HashMap<u32, String> = {
        let guard = ANCHOR_PAIRS.lock().unwrap();
        match guard.as_ref() {
            Some(anchors) => translate_anchors(anchors, &processes),
            None => std::collections::HashMap::new(),
        }
    };
    let mut tasks = am_core::scanner::build_tasks(
        &sessions,
        &processes,
        &|pid| paused.contains(&pid),
        &pinned,
        &active_ids,
        &cached,
    );
    // 回填「正等你选」：hook 是按 session_id 报的，这里对上号挂到会话上。
    // 没对上（会话已被 /clear 换掉等）就丢弃 —— 一张挂错会话的选项卡比没有更糟。
    if !pending_selects.is_empty() {
        for t in &mut tasks {
            if let Some(sel) = pending_selects.remove(&t.id) {
                t.pending_select = Some(sel);
            }
        }
    }
    // 用本轮真实配对刷新锚缓存：把「claude_pid → 会话」按终端锚身份 (pid,start) 归账（非 pid- 占位）
    {
        let key_of: std::collections::HashMap<u32, (u32, u64)> =
            processes.iter().map(|p| (p.pid, anchor_key(p))).collect();
        let mut new_anchors = std::collections::HashMap::new();
        for t in &tasks {
            if let Some(pid) = t.pid {
                if !t.id.starts_with("pid-") {
                    if let Some(&(apid, astart)) = key_of.get(&pid) {
                        new_anchors.insert(apid, (t.id.clone(), astart));
                    }
                }
            }
        }
        // 持久化（节流每 8 轮 ~12s）：写盘先于覆盖内存，写的是本轮真实配对（终端锚→会话）
        if SCAN_TICKS.load(std::sync::atomic::Ordering::Relaxed) % 8 == 0 {
            save_anchor_pairs(&state.config.data_dir, &new_anchors);
        }
        *ANCHOR_PAIRS.lock().unwrap() = Some(new_anchors);
    }
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

#[cfg(test)]
mod anchor_tests {
    use super::translate_anchors;
    use am_core::model::{IdeKind, ProcessInfo};
    use std::collections::HashMap;

    const SHELL_START: u64 = 500; // 测试里 shell 的固定启动时间

    /// 造一个带终端锚(shell_pid + 固定 shell_start)的 claude 进程；shell=None 表示找不到 shell。
    fn proc(pid: u32, shell: Option<u32>) -> ProcessInfo {
        proc_full(pid, shell, shell.map(|_| SHELL_START), 0)
    }
    /// 带指定 claude 启动时间（用于区分主 claude / 子 agent）。
    fn proc_at(pid: u32, shell: Option<u32>, start_time: u64) -> ProcessInfo {
        proc_full(pid, shell, shell.map(|_| SHELL_START), start_time)
    }
    /// 完全指定（含 shell_start，用于 pid 重用测试）。
    fn proc_full(
        pid: u32,
        shell: Option<u32>,
        shell_start: Option<u64>,
        start_time: u64,
    ) -> ProcessInfo {
        ProcessInfo {
            pid,
            agent: "claude".into(),
            tty: String::new(),
            cwd: "/proj".into(),
            ide: IdeKind::Cursor,
            ide_name: "Cursor".into(),
            start_time,
            cpu_usage: 0.0,
            memory: 0,
            command: "claude".into(),
            shell_pid: shell,
            shell_start,
        }
    }

    /// 锚表：(锚 pid, sid, 锚 start)。
    fn anchors(pairs: &[(u32, &str, u64)]) -> HashMap<u32, (String, u64)> {
        pairs.iter().map(|(a, s, st)| (*a, (s.to_string(), *st))).collect()
    }

    /// 核心：claude 换了 pid（100→200），但仍在同一终端 shell(2244) 下 —— 锚表存的是
    /// 「2244→sess」，翻译后新 pid 200 照样接回该会话。这就是「claude 换 pid 也接得回」。
    #[test]
    fn reconnects_after_claude_pid_change() {
        let a = anchors(&[(2244, "sess-A", SHELL_START)]);
        let got = translate_anchors(&a, &[proc(200, Some(2244))]);
        assert_eq!(got.get(&200), Some(&"sess-A".to_string()));
    }

    /// pid 重用回归：关掉终端 A(shell 2244,start 500,会话 sess-A)、再开终端 B，Windows 把
    /// 2244 重用给 B 的 shell（start 变成 900）。锚 pid 同为 2244 但 start 不同 → 不该接回
    /// sess-A（否则新终端错配旧会话，用户实测的"关了再开还显示旧会话"）。
    #[test]
    fn reused_shell_pid_with_different_start_is_rejected() {
        let a = anchors(&[(2244, "sess-A", 500)]);
        // B 的 claude 在同 pid 2244 但 start=900 的新 shell 下
        let b = proc_full(200, Some(2244), Some(900), 0);
        let got = translate_anchors(&a, &[b]);
        assert!(got.is_empty(), "shell pid 重用(start 不同)不该接回旧会话");
    }

    /// 锚表里的终端此刻没有活 claude（死终端/换项目）→ 翻不出配对，不会硬配。
    #[test]
    fn stale_anchor_without_live_proc_is_dropped() {
        let a = anchors(&[(9999, "sess-A", SHELL_START)]);
        let got = translate_anchors(&a, &[proc(200, Some(2244))]);
        assert!(got.is_empty());
    }

    /// 找不到 shell 祖先(shell_pid=None) → 回退用 claude 自身 (pid,start) 当锚匹配。
    #[test]
    fn falls_back_to_self_pid_when_no_shell() {
        let a = anchors(&[(200, "sess-A", 0)]); // 回退锚 = (claude pid 200, claude start 0)
        let got = translate_anchors(&a, &[proc(200, None)]);
        assert_eq!(got.get(&200), Some(&"sess-A".to_string()));
    }

    /// 多终端各自接回，互不串。
    #[test]
    fn multiple_terminals_map_independently() {
        let a = anchors(&[(2244, "sess-A", SHELL_START), (3355, "sess-B", SHELL_START)]);
        let got = translate_anchors(&a, &[proc(200, Some(2244)), proc(201, Some(3355))]);
        assert_eq!(got.get(&200), Some(&"sess-A".to_string()));
        assert_eq!(got.get(&201), Some(&"sess-B".to_string()));
    }

    /// 撞车回归：同一终端 shell(2244) 下有主 claude(pid=200,先启动) 与 Task 子 agent
    /// (pid=999,后启动)，两者终端锚相同。会话只能配到**主 claude**，不能配到子 agent。
    #[test]
    fn same_shell_picks_earliest_started_main_claude() {
        let a = anchors(&[(2244, "sess-A", SHELL_START)]);
        let main = proc_at(200, Some(2244), 1000); // 先启动
        let sub = proc_at(999, Some(2244), 2000); // 会话进行中才派生，启动更晚
        // 两种进程顺序都要稳定挑主 claude（不受 Vec/HashMap 顺序影响）
        let got1 = translate_anchors(&a, &[main.clone(), sub.clone()]);
        let got2 = translate_anchors(&a, &[sub, main]);
        for got in [got1, got2] {
            assert_eq!(got.get(&200), Some(&"sess-A".to_string()), "会话应配到主 claude");
            assert_eq!(got.get(&999), None, "子 agent 不该拿到会话");
        }
    }
}
