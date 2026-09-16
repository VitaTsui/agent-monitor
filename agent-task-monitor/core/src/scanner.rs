use crate::model::{MessageBrief, ProcessInfo, SubTask, SubTaskOutcome, Task, TaskStatus};
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个会话 jsonl 解析出的摘要（缓存单元）
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// 会话来源代理：claude / codex …（决定消息解析器与进程配对）
    pub provider: String,
    /// 这条会话来自**桌面客户端**而非终端 CLI（ChatGPT 桌面版的 Codex、
    /// Claude 桌面版的本地代理）。两处判据都取自上游自己写下的事实，不是猜的：
    /// codex 看 `session_meta.payload.originator`，claude 看会话文件在不在桌面客户端
    /// 的本地代理目录树里。只影响展示名与配对方式，解析器是同一套。
    pub desktop: bool,
    pub session_id: String,
    /// 项目目录编码名（~/.claude/projects 下的目录名），配对进程用
    pub project_key: String,
    /// 归一化后的项目根（`encode_path` 等于 `project_key` 的那个 cwd）。配对/分组用，
    /// 不随会话内 `cd` 漂移。
    pub cwd: String,
    /// 会话的**锚定目录**：`shell_cwd` 所在的 git 仓库根（不在仓库里就是 `shell_cwd`
    /// 本身，尾窗没读到就退回 `cwd`）。上传落点、目录浏览根都用它。
    ///
    /// 之所以要往上收到仓库根：`shell_cwd` 随会话 `cd` 每一轮都在跳（实测同一会话几分钟内
    /// 走过 `desktop`、`desktop/src-tauri`、`desktop/web/dist`），拿它当落点等于每次上传都
    /// 不知道文件会落到哪，连构建产物目录都能落进去。仓库根则怎么 cd 都不变。
    pub live_cwd: String,
    /// 尾窗里**最后一条**记录的 cwd 原样（空 = 尾窗没读到）。
    ///
    /// 只有一个用途：判断会话是否已经漂到锚定目录之下。漂了就说明「终端此刻在哪」与
    /// 「文件落在哪」不是同一个目录，相对路径是否解析得对取决于终端拿哪个当根 —— 那件事
    /// 我们无从确证，于是这种时候前端改回填绝对路径，把不确定性绕开（见 web 的 doUpload）。
    pub shell_cwd: String,
    /// 会话标题（首个用户提示词）
    pub title: String,
    pub prompt: String,
    pub last_action: String,
    /// 最后一条有效条目是否表示「回合结束」（助手纯文本收尾）
    pub turn_ended: bool,
    /// 本会话**此刻**是刚 `/clear` 出来、还没输入的空会话：内容只有 /clear 命令块、
    /// 无真实 prompt。用户一敲字就翻回 false。
    ///
    /// **这个公开字段目前没有任何生产读取点**，留着只为对外把「此刻空着」这件事说清楚
    /// （上报出去的快照里也带着它）。`turn_ended` 那处收口读的是 `parse_tail` 里的同名
    /// 局部变量，不走这里。
    ///
    /// 进程跟着 `/clear` 迁移那件事**一概不看它** —— 它只活几秒，扫描周期（1.5 秒）错过
    /// 一拍就永远观测不到，跟随会 100% 失效。那件事看的是 [`Self::clear_born`] 与
    /// [`supersession_map`]。别再把任何判断挂回这个字段上。
    pub cleared: bool,
    /// **这条会话是某次 `/clear` 生出来的**：它的记录里出现过 `/clear` 命令块。
    ///
    /// 与 [`Self::cleared`] 的区别只有一处，却是关键的一处：`cleared` 还要求「还没有真实
    /// prompt」，用户一输入就翻回 false —— 等于给「我接替了谁」按了个几秒钟的保质期，
    /// 用户手快一点，前端就永远错过那一帧。继任关系（`Task::supersedes`）因此建在这一条上：
    /// 它只跟「/clear 命令块还在不在尾窗里」有关，与用户输入无关。
    ///
    /// 边界，别当成永久事实：判据取自 `parse_tail` 的尾窗（`TAIL_BYTES`，4 MiB），命令块
    /// 在文件开头，会话正文涨过尾窗后它会翻回 false。无害 —— 那是几小时后的事，而跟随在
    /// 换 id 后几秒内就完成了。
    pub clear_born: bool,
    pub started_at: Option<String>,
    pub last_active_at: Option<String>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub mtime_ms: u64,
    /// 会话文件创建时间（epoch 毫秒，取不到为 0）。用于把进程配到它真正在跑的会话：
    /// 新起/空白的会话在终端打开（=进程启动）那刻创建，created_ms≈进程 start_time；
    /// 旧会话创建时间差很远。比 mtime/started_at 都可靠（空白会话没 started_at、
    /// 长跑会话 mtime 不等于启动时刻）。
    pub created_ms: u64,
    pub line_count: u64,
    /// 近 5 小时滚动窗口内的 token 用量（input+output+cache_creation 估算）
    pub used_tokens_5h: u64,
    /// 终端里 claude 原生排队、尚未被会话接受执行的输入（按入队顺序）。
    /// 来自会话 jsonl 的 queue-operation 记录：enqueue 入列、remove 出列（被接受或取消），
    /// 末态仍在列的即当前排队项。前端把它们挂在内容区底部显示。
    pub queued_inputs: Vec<String>,
    /// 尾窗里**最后一次 AskUserQuestion 被了结**的时刻（epoch 毫秒）：作答落下 tool_result，
    /// 或这一轮被 Esc 中断。没见过就是 None。
    ///
    /// 「终端正等你选」平时由 PostToolUse hook 清除，但那条 hook 缺席的情形不少
    /// （客户端旧版没写这条配置、hook 拿不到 CLAUDE_PID、用户按 Esc 直接打断），
    /// 一缺席卡片就在远端永远挂着。jsonl 里的 tool_result 是**精确**信号：它带着
    /// 对应 tool_use 的 id，不是「文件又写过 ⇒ 大概答完了」那种会误伤的启发式
    /// （见 client::state 回填处的说明）。
    pub select_answered_ms: Option<u64>,
    /// **这条会话里有没有任何实质内容**：真实用户输入，或助手的回复。
    ///
    /// 只在斜杠命令里打过转的会话（`/clear`、`/model`…）两样都没有 —— 那些命令被记成
    /// `type=user`，正文是 `<command-name>…</command-name>` 信封，[`user_text`] 剥完
    /// 就什么都不剩。实测本机 73 份会话记录里有 **11 份**是这种空壳，且**全部**
    /// assistant 记录数为 0、图片附件数为 0。
    ///
    /// 判据取「用户输入 **或** 助手回复」而不是只看标题：只发了图片、或只有工具调用的
    /// 会话标题也可能是空的，那种有内容、不该丢。（本机没有这类样本，所以这道保险是
    /// 按可能性留的，不是按现象留的。）
    pub has_content: bool,
    /// **这条会话派过几个子代理**（只数 `subagents/agent-*.jsonl` 的文件数）。
    ///
    /// 给列表用的一个便宜的「有没有、有几个」：侧栏要据此决定这一行画不画展开箭头，
    /// 而真正的子代理清单只能现读磁盘（`/monitor/tasks/:id/subtasks`，大会话 0.8~2 秒），
    /// 一行拉一次是不可能的。所以这里**只 readdir 数文件名，不打开任何文件**。
    ///
    /// 与 [`crate::model::Task::sub_tasks`] 不是一回事：那份是带状态的清单、还套着
    /// 24 小时 / 50 条的保留窗口（只服务「当前状态面板」），而这个是**总数、不设窗口**，
    /// 历史会话照样是真实值 —— 两个数字对不上是正常的。
    pub sub_agent_count: usize,
}

impl SessionSummary {
    /// **空壳**：整份记录剥完只剩斜杠命令信封（`/clear`、`/model`…），一句人话都没有。
    ///
    /// 判据是「用户输入 **或** 助手回复 **或** 任何一处兜出来的提示词」三者全空，而不是
    /// 「标题为空」——只发了图片、只有工具调用的会话标题也可能是空的，那种有内容。
    /// `prompt` 在 `summarize` 里已经过 上一轮缓存 → 文件头部 两级兜底，所以长会话的尾窗
    /// 里恰好没提示词也不会被误判。
    ///
    /// **空壳 ≠ 该丢**：`/clear` 刚生出来的新会话就是这个形状，而它是此刻活着的那一条。
    /// 死壳子与活会话的差别在「有没有进程占着」，由 [`build_tasks`] 判（那里才有进程）。
    pub fn is_empty_shell(&self) -> bool {
        !self.has_content && self.prompt.is_empty()
    }
}

/// 一个会话的「当前全貌」：对话消息 + 后台子任务。
///
/// 两者来源不同（前者读尾部窗口，后者从会话开头增量重放），但调用方每次都要一起拿 ——
/// 分成两个方法会让同一份文件被解析两遍。
#[derive(Debug, Clone, Default)]
pub struct SessionView {
    /// 最近若干条消息，末尾可能附一条 `role:"todos"` 的任务清单快照
    pub messages: Vec<MessageBrief>,
    /// 该会话名下的后台子任务（已按磁盘纠正状态、已淘汰过期条目）
    pub sub_tasks: Vec<SubTask>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    size: u64,
    mtime_ms: u64,
    line_count: u64,
    summary: SessionSummary,
    /// 文件头部解析出的稳定信息（append-only，只读一次）
    head: HeadInfo,
}

/// 会话文件头部的稳定信息：初始 cwd / 初始提示词 / 会话开始时间
#[derive(Debug, Clone, Default)]
struct HeadInfo {
    cwd: Option<String>,
    prompt: Option<String>,
    started_at: Option<String>,
}

/// Claude Code 会话来源：扫描 ~/.claude/projects 下的 jsonl，带增量缓存。\n/// 未来的 Codex 会话来源可平行实现一个 Scanner 并在聚合处合并。
pub struct SessionScanner {
    projects_dir: PathBuf,
    /// Codex CLI 会话根目录（~/.codex/sessions），不存在则跳过
    codex_dir: PathBuf,
    /// Claude 桌面版本地代理的会话根目录，不存在则跳过（= 没装/没用过桌面版，
    /// 行为与本次改动前完全一致）
    claude_desktop_dir: PathBuf,
    cache: HashMap<PathBuf, CacheEntry>,
    /// 每个会话的「当前状态」重放进度（任务清单 / 后台任务）
    state_cache: HashMap<PathBuf, SessionState>,
}

/// 任务清单与后台任务的重放状态。
///
/// 这两者必须从会话开头重放才能成形（TaskCreate 决定任务号、run_in_background
/// 决定哪些是后台任务），只读尾部窗口是拼不出来的 —— 长会话动辄几十 MB，
/// 早期的 TaskCreate 全在窗口之外，清单会永远是空的。
/// 会话文件是 append-only，故这里记住已消费的偏移，每次只解析新增字节。
#[derive(Default)]
struct SessionState {
    /// 已消费到的字节偏移（总是停在某个换行之后）
    offset: u64,
    todos: TodoTracker,
    bg: BgTracker,
}

/// 会话列表回溯天数的默认值。
///
/// 原本写死 7 天，代价是「上周那条会话」在界面上根本不存在 —— 不是列出来标成已结束，
/// 是连扫都不扫。放宽到 30 天：本机实测 7 天窗口只有 7 份 jsonl，30 天窗口 61 份
/// （Codex 另有 33 份），一个月足够覆盖「上次那个需求是怎么改的」这类回看。
pub const HISTORY_DAYS_DEFAULT: u64 = 30;

/// 环境变量名：会话列表回溯天数。
///
/// 沿用本项目既有的配置方式 —— `AM_*` 环境变量，客户端启动时还会把 `config.txt`
/// 里的同名键补进环境（见 client/src/main.rs 的配置加载），所以桌面版用户改配置文件
/// 即可，不必设系统环境变量。不另起第四套配置。
pub const HISTORY_DAYS_ENV: &str = "AM_HISTORY_DAYS";

/// 会话列表最多回溯的时长（毫秒）。
///
/// 只读一次环境变量并记住：这个值每轮扫描的每个文件都要用一次，本机 30 天窗口下
/// 一轮就是近百次；而进程生命周期内它不会变（改配置要重启客户端）。
/// 取值非法（非数字 / 0）时回落到默认值，不 panic —— 配置写错不该让客户端起不来。
fn history_window_ms() -> u64 {
    static CACHED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        let days = std::env::var(HISTORY_DAYS_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|d| *d > 0)
            .unwrap_or(HISTORY_DAYS_DEFAULT);
        days.saturating_mul(24 * 3600 * 1000)
    })
}

/// 同一个会话号只留一条。
///
/// [`SessionScanner::scan`] 会走**三个来源**（CLI 根、桌面版本地代理的每个隔离家目录、
/// Codex），而且每个 `projects` 根下还可以有多个项目目录 —— 没有任何东西保证同一个
/// 会话号不会在两处同时出现（`~/.claude/projects/<A>/<id>.jsonl` 与
/// `<B>/<id>.jsonl` 就能撞上）。此前产出侧不去重，重复与否全靠运气，
/// 而网页那头用 Map 压着才没发作 —— **靠消费方兜着的不算修好**。
///
/// 冲突时**留 mtime 大的那份**：同一个会话号在两处，新的那份才是还在写的；
/// 并列时留先扫到的（扫描顺序 CLI → 桌面版 → Codex，确定）。
/// 入参已按 mtime 倒序排好，所以只要保留首次出现即可。
///
/// 实测本机 73 份会话记录里没有撞号的 —— 这是一道结构性保险，不是在修一个正在发作的
/// 现象；但它的代价只是一个 HashSet。
fn dedup_by_session_id(sorted_by_mtime_desc: Vec<SessionSummary>) -> Vec<SessionSummary> {
    let mut seen = HashSet::new();
    sorted_by_mtime_desc
        .into_iter()
        .filter(|s| seen.insert(s.session_id.clone()))
        .collect()
}

/// 环境变量名：把 cwd 落在**系统临时目录**里的会话也一并列出来。
///
/// 默认不列（见 [`is_throwaway_cwd`]）。万一真有人把活干在 /tmp 里，设
/// `AM_KEEP_TEMP_SESSIONS=1` 即可恢复原样。沿用本项目既有的 `AM_*` 方式，
/// 客户端启动时还会把 `config.txt` 里的同名键补进环境。
pub const KEEP_TEMP_SESSIONS_ENV: &str = "AM_KEEP_TEMP_SESSIONS";

/// 系统临时目录的根。
///
/// 两个来源，都不是我发明的路径：
/// - [`std::env::temp_dir()`]：各平台由系统/环境给出（Windows 的 `%TEMP%`、
///   Unix 的 `$TMPDIR`）。macOS 上它是每用户的 `/var/folders/…`。
/// - Unix 上再加一个 `/tmp`：POSIX 规定的共享临时目录，Rust 标准库自己在
///   `$TMPDIR` 缺失时也回落到它。macOS 给每个用户单独的 `$TMPDIR`，但很多工具
///   （包括 Claude Code 自己的 scratchpad）照样写 `/tmp` —— 实测本机 17 条一次性
///   会话的 cwd 全在 `/private/tmp/claude-501/…` 下，一条都不在 `$TMPDIR` 里。
///
/// **不枚举具体的一次性目录名**（`claude-501`、`am-verify-wt`、`worktrees`…）：
/// 那是拿字面量当判据，换个工具、换台机器就失效。这里判的是「操作系统说这块地方是
/// 临时的」，与谁在里面建了什么无关。
///
/// 每个根同时给出原样与 canonical 两种形态：macOS 上 `/tmp` 是 `/private/tmp` 的
/// 符号链接，而会话记录里写的是解析后的 `/private/tmp/…`，只比原样会一条都匹配不上。
fn temp_roots() -> &'static [PathBuf] {
    static ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut roots = vec![std::env::temp_dir()];
        if cfg!(unix) {
            roots.push(PathBuf::from("/tmp"));
        }
        let mut out = Vec::new();
        for r in roots {
            if let Ok(c) = r.canonicalize() {
                if c != r {
                    out.push(c);
                }
            }
            out.push(r);
        }
        out
    })
}

/// 这条会话的工作目录是不是**一次性的**（落在系统临时目录里）。
///
/// 这类会话本来就不该当项目列出来：实测本机 73 份会话记录里有 **17 份**的 cwd 在
/// `/private/tmp/claude-501/<项目>/<会话号>/scratchpad` 这类目录下 —— 都是工具自己
/// 开的草稿地，用完即弃，路径大多已经不存在了。scanner 此前从不校验 cwd，于是它们
/// 全都以「项目」的身份出现在侧栏里。
///
/// 判据只认「系统临时目录前缀」，不认「目录是否还存在」：后者会把插着的外置盘没挂上、
/// 或仓库临时挪过位置的**真项目**一起丢掉，那比多列几行糟得多。（本机实测两者抓到的
/// 是同一批 17 条，但失效方式完全不同。）
fn is_throwaway_cwd(cwd: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let p = Path::new(cwd);
    let canon = p.canonicalize().ok();
    temp_roots()
        .iter()
        .any(|root| p.starts_with(root) || canon.as_deref().is_some_and(|c| c.starts_with(root)))
}

/// 「这条会话还可能活着」的窗口（毫秒）。两处在用，是同一个判断：
///
/// 1. **会话 ↔ 进程配对**：比这更老的会话不参与配对 —— 它不可能还连着一个活着的终端，
///    放进候选只会去抢别人的进程（配对按最近活动排序 + zip 截断，多出来的纯属干扰）。
/// 2. **每轮上报的热列表**：客户端 1.5s 一轮全量重报一次会话快照，比这更老的会话
///    改走低频的历史列表（见 client 的 `HISTORY_REPORT_INTERVAL_SECS`）。
///    实测：7 天窗口一轮 8 条、11 KB；30 天窗口 94 条、92 KB —— 后者按 1.5s 一轮
///    算是每天 5 GB 的上行，不能每轮都发。
///
/// **与 [`history_window_ms`] 是两回事，故不跟着一起放宽**：那个宽是为了「看得见历史」，
/// 这个宽只会让判断变差。7 天的理由见配对处的注释（挂一夜/过周末的会话仍要配得上进程）。
pub const LIVE_WINDOW_MS: u64 = 7 * 24 * 3600 * 1000;
/// 摘要解析时读取的文件尾部大小
const TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// 头部读取大小（拿初始 cwd / 提示词 / 开始时间）
const HEAD_BYTES: usize = 256 * 1024;
/// codex 写在 `session_meta.payload.originator` 里的「ChatGPT 桌面版」标记。
/// 本机实测另两种取值 `codex_exec` / `codex-tui` 都是 CLI。
const CODEX_DESKTOP_ORIGINATOR: &str = "Codex Desktop";
/// Claude 桌面版本地代理（Cowork）给每条会话开一个隔离的家目录：
/// `<根>/<组织 id>/<用户 id>/local_<会话 uuid>/.claude/projects/<项目名>/<uuid>.jsonl`。
/// 里面那份 jsonl 就是标准 Claude Code 格式，用同一套解析器。
/// 两层 id 是上游私有实现、随时可能变，所以这里不写死层数，见 [`claude_desktop_roots`]。
const CLAUDE_DESKTOP_SESSION_PREFIX: &str = "local_";
/// 本地代理根目录名。既用来拼绝对路径（见 [`claude_desktop_dir`]），也当作
/// [`claude_local_agent_session_id`] 的锚点 —— 少了它，任何叫 `local_xxx` 的
/// 普通目录都会被当成会话 id。
const CLAUDE_DESKTOP_ROOT_DIR: &str = "local-agent-mode-sessions";
/// 从本地代理根往下找 `local_*` 的最大深度（实测在第 2 层；留一层余量）。
const CLAUDE_DESKTOP_MAX_DEPTH: usize = 3;

// 关于会话目录旁边那份 `local_<uuid>.json`（含 `title`/`cwd`/`lastActivityAt`/
// `isAgentCompleted`）：**故意不读**。
//
// 它能给的 title/cwd/起止时间，jsonl 里本来就有，同一套解析器已经拿到了；剩下唯一
// 有诱惑力的是 `isAgentCompleted` —— 名字看着像「这条会话跑完了没有」，实测**不是**。
// 本机 11 条真实会话里只有 2 条带这个字段，两条都是 `false`，而它们的 `audit.jsonl`
// 末行都是 `{"type":"result","subtype":"success","stop_reason":"end_turn"}`：回合明明
// 已经正常收尾，字段却仍是 `false`。照它判活性，等于把早就结束的会话永远显示成
// 「执行中」—— 正是要避免的那类假状态。
//
// 所以活性仍只有一个来源：**有没有配到活着的进程**。本地代理跑在宿主机上时
// （`hostLoopMode`）就是一个普通的 claude 进程，按 cwd 正常配对；跑在 VM 里时
// 宿主机上根本没有对应进程，如实显示「已结束」，不拿一个语义没验证的字段去凑。

/// 从本地代理根 `root` 下找出全部 `.claude/projects`（实现见
/// [`SessionScanner::claude_desktop_roots`] 的说明）。
fn claude_desktop_roots(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > CLAUDE_DESKTOP_MAX_DEPTH {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let is_session = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(CLAUDE_DESKTOP_SESSION_PREFIX));
            if is_session {
                let projects = p.join(".claude").join("projects");
                if projects.is_dir() {
                    out.push(projects);
                }
                // 会话家目录里不会再嵌一个会话家目录，不必往下走
                continue;
            }
            walk(&p, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, 0, &mut out);
    }
    out
}

/// Claude 桌面版本地代理的会话根目录。
///
/// Electron 的 userData 根：macOS `~/Library/Application Support/Claude`、
/// Windows `%APPDATA%\\Claude`、Linux `~/.config/Claude` —— 正是 `dirs::config_dir()`
/// 各平台的取值。取不到家目录就给一个必然不存在的路径，调用方按「目录不存在」处理。
fn claude_desktop_dir() -> PathBuf {
    dirs::config_dir()
        .map(|c| c.join("Claude").join(CLAUDE_DESKTOP_ROOT_DIR))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

/// 从一个路径里取出 Claude 桌面版本地代理的**会话 id**（形如 `local_<uuid>`），
/// 认不出来就返回 None。
///
/// 这个 id 不是我们发明的编号，是上游自己给会话的主键：会话隔离家目录就叫这个名字
/// （`<根>/<组织>/<用户>/local_<uuid>/…`），桌面客户端内部也用它当路由参数
/// （app.asar 里 `dispatchNavigate(\`/cowork/${sessionId}\`)`）。正因为两处是同一个值，
/// 客户端才能拿磁盘上的路径去比对「窗口里此刻开着的是哪条会话」
/// （见 `client::appinject::SessionRef`）。
///
/// 判据是**两段路径都要在**：先出现根目录名 `local-agent-mode-sessions`，其后才认
/// `local_` 打头的那一段。只认前缀会把任何用户目录里叫 `local_xxx` 的文件夹误当成会话。
pub fn claude_local_agent_session_id(path: &str) -> Option<String> {
    let mut seen_root = false;
    for seg in path.split(['/', '\\']) {
        if seg == CLAUDE_DESKTOP_ROOT_DIR {
            seen_root = true;
            continue;
        }
        if seen_root && seg.starts_with(CLAUDE_DESKTOP_SESSION_PREFIX) {
            return Some(seg.to_string());
        }
    }
    None
}

impl SessionScanner {
    pub fn new(projects_dir: PathBuf) -> Self {
        let codex_dir = dirs::home_dir()
            .map(|h| h.join(".codex/sessions"))
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        Self {
            projects_dir,
            codex_dir,
            claude_desktop_dir: claude_desktop_dir(),
            cache: HashMap::new(),
            state_cache: HashMap::new(),
        }
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    /// 扫描全部项目目录，返回回溯窗口内（见 [`history_window_ms`]）有活动的会话摘要
    pub fn scan(&mut self) -> Vec<SessionSummary> {
        let now_ms = now_ms();
        let mut out = Vec::new();
        // Claude Code CLI 会话（~/.claude/projects）
        self.scan_projects_root(&self.projects_dir.clone(), false, &mut out, now_ms);
        // Claude 桌面版本地代理会话：每条会话一个隔离家目录，里面是同样的
        // `.claude/projects/<项目>/<uuid>.jsonl` —— 同一套解析器，只是换个根。
        // 目录不存在（没装桌面版/没用过本地代理）时下面这行返回空表，等于没这段。
        for root in self.claude_desktop_roots() {
            self.scan_projects_root(&root, true, &mut out, now_ms);
        }
        // Codex CLI 会话（~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl）
        self.scan_codex_into(&mut out, now_ms);
        // 工作目录是一次性草稿地的会话不算项目，默认不列（见 is_throwaway_cwd）
        if std::env::var(KEEP_TEMP_SESSIONS_ENV).is_err() {
            out.retain(|s| !is_throwaway_cwd(&s.cwd));
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.mtime_ms));
        dedup_by_session_id(out)
    }

    /// 扫一个 `projects` 根：`<root>/<项目目录>/<会话 uuid>.jsonl`。
    ///
    /// `desktop` 决定这批会话算不算桌面客户端来源 —— 文件内容一模一样，区别只在它躺在
    /// 哪个根下面，解析器认不出来，只有调用方知道。
    fn scan_projects_root(
        &mut self,
        root: &Path,
        desktop: bool,
        out: &mut Vec<SessionSummary>,
        now_ms: u64,
    ) {
        let Ok(projects) = fs::read_dir(root) else {
            return;
        };
        for project in projects.flatten() {
            let pdir = project.path();
            if !pdir.is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(&pdir) else {
                continue;
            };
            for f in files.flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if now_ms.saturating_sub(mtime_ms) > history_window_ms() {
                    continue;
                }
                let created_ms = meta
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Some(mut summary) = self.summarize(&path, meta.len(), mtime_ms) {
                    summary.created_ms = created_ms;
                    summary.desktop = desktop;
                    // 每轮现数：父会话 jsonl 一个字节没动、子代理目录照样会变
                    // （子代理跑完时只有它自己那份记录在长），放进 summarize 的
                    // size+mtime 缓存里会一直是旧值。
                    summary.sub_agent_count = count_sub_agents(&path);
                    out.push(summary);
                }
            }
        }
    }

    /// Claude 桌面版本地代理下的全部 `projects` 根。
    ///
    /// 目录树是上游私有实现（`<根>/<组织 id>/<用户 id>/local_<会话 uuid>/.claude/projects`，
    /// 两层 id 随时可能加减），所以这里不写死层数：从根往下最多 [`CLAUDE_DESKTOP_MAX_DEPTH`]
    /// 层找名字以 `local_` 开头、且底下确实有 `.claude/projects` 的目录。
    /// 层数或命名一旦变了就一个都找不到 → 返回空表 → 与没有这段代码时表现一致，
    /// 不会报错、也不会把别处的会话误收进来。
    fn claude_desktop_roots(&self) -> Vec<PathBuf> {
        claude_desktop_roots(&self.claude_desktop_dir)
    }

    /// 递归收集 Codex 会话摘要（同一个回溯窗口，带同一套 mtime/size 缓存）
    fn scan_codex_into(&mut self, out: &mut Vec<SessionSummary>, now_ms: u64) {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
            if depth > 4 {
                return;
            }
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, files, depth + 1);
                } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                    files.push(p);
                }
            }
        }
        if !self.codex_dir.is_dir() {
            return;
        }
        let mut files = Vec::new();
        walk(&self.codex_dir.clone(), &mut files, 0);
        for path in files {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if now_ms.saturating_sub(mtime_ms) > history_window_ms() {
                continue;
            }
            let created_ms = meta
                .created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if let Some(mut sum) = self.summarize_codex(&path, meta.len(), mtime_ms) {
                sum.created_ms = created_ms;
                out.push(sum);
            }
        }
    }

    /// Codex 会话摘要：head 取 session_meta（cwd/开始时间/会话号）与首条真实用户消息，
    /// tail 取最近动作与回合状态。缓存策略与 Claude 相同（size+mtime 命中即复用）。
    fn summarize_codex(&mut self, path: &Path, size: u64, mtime_ms: u64) -> Option<SessionSummary> {
        if let Some(hit) = self.cache.get(path) {
            if hit.size == size && hit.mtime_ms == mtime_ms {
                return Some(hit.summary.clone());
            }
        }
        let head_txt = {
            let mut f = fs::File::open(path).ok()?;
            let mut buf = vec![0u8; HEAD_BYTES];
            let n = f.read(&mut buf).ok()?;
            buf.truncate(n);
            String::from_utf8_lossy(&buf).to_string()
        };
        let mut session_id = None;
        let mut cwd = String::new();
        let mut started_at = None;
        let mut prompt = String::new();
        let mut originator = String::new();
        for line in head_txt.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str) {
                Some("session_meta") => {
                    let p = v.get("payload");
                    session_id = p
                        .and_then(|p| p.get("session_id").or_else(|| p.get("id")))
                        .and_then(Value::as_str)
                        .map(String::from);
                    if let Some(c) = p.and_then(|p| p.get("cwd")).and_then(Value::as_str) {
                        cwd = c.to_string();
                    }
                    if let Some(o) = p.and_then(|p| p.get("originator")).and_then(Value::as_str) {
                        originator = o.to_string();
                    }
                    started_at = p
                        .and_then(|p| p.get("timestamp"))
                        .and_then(Value::as_str)
                        .map(String::from)
                        .or_else(|| v.get("timestamp").and_then(Value::as_str).map(String::from));
                }
                Some("response_item") if prompt.is_empty() => {
                    if let Some(t) = codex_user_text(&v) {
                        prompt = t;
                    }
                }
                _ => {}
            }
            if session_id.is_some() && !prompt.is_empty() {
                break;
            }
        }
        // 文件名兜底取会话号：rollout-…-<uuid>.jsonl
        let session_id = session_id.or_else(|| {
            path.file_stem()?
                .to_str()?
                .rsplit('-')
                .next()
                .map(String::from)
        })?;

        // 尾部：最近动作 + 回合是否结束（最后一条有效项是否助手文本）
        let tail = read_tail(path, TAIL_BYTES).ok()?;
        let mut last_action = String::new();
        let mut turn_ended = false;
        let mut last_active = None;
        for line in tail.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
                last_active = Some(ts.to_string());
            }
            if v.get("type").and_then(Value::as_str) != Some("response_item") {
                continue;
            }
            let p = v.get("payload");
            match p.and_then(|p| p.get("type")).and_then(Value::as_str) {
                Some("message") => {
                    let role = p.and_then(|p| p.get("role")).and_then(Value::as_str);
                    if role == Some("assistant") {
                        turn_ended = true;
                    } else if role == Some("user") && codex_user_text(&v).is_some() {
                        turn_ended = false;
                    }
                }
                Some("function_call") | Some("custom_tool_call") => {
                    if let Some(n) = p.and_then(|p| p.get("name")).and_then(Value::as_str) {
                        last_action = n.to_string();
                    }
                    turn_ended = false;
                }
                _ => {}
            }
        }

        let summary = SessionSummary {
            provider: "codex".into(),
            // `originator` 是 codex 自己写进 session_meta 的来源标记，本机实测三种取值：
            // `Codex Desktop`（ChatGPT 桌面版）、`codex_exec`、`codex-tui`（都是 CLI）。
            // 上游哪天改了名字，这里认不出来 → 当成 CLI 会话，即本次改动前的行为。
            desktop: originator == CODEX_DESKTOP_ORIGINATOR,
            session_id,
            project_key: encode_path(&cwd),
            // codex 的 cwd 取自 session_meta，一条会话只有一个值、不存在漂移，
            // 于是「项目根」「锚定目录」「此刻在哪」本就是同一个 —— 也因此不收到 git 根：
            // codex 就在这个目录里跑，往上挪反而会让相对路径失准。
            live_cwd: cwd.clone(),
            shell_cwd: cwd.clone(),
            cwd,
            title: prompt.clone(),
            prompt,
            last_action,
            turn_ended,
            cleared: false,
            clear_born: false,
            // Codex 会话没有斜杠命令信封那套，能解析出来就是有内容的
            has_content: true,
            // Codex 没有子代理这套机制
            sub_agent_count: 0,
            started_at,
            last_active_at: last_active,
            version: None,
            git_branch: None,
            mtime_ms,
            created_ms: 0,
            line_count: 0,
            used_tokens_5h: 0,
            queued_inputs: Vec::new(),
            // codex 没有 AskUserQuestion 这套选择卡，也就无所谓了结
            select_answered_ms: None,
        };
        self.cache.insert(
            path.to_path_buf(),
            CacheEntry {
                size,
                mtime_ms,
                line_count: 0,
                summary: summary.clone(),
                head: HeadInfo::default(),
            },
        );
        Some(summary)
    }

    /// 带缓存的摘要解析：文件未变直接复用；变了只增量数行数 + 重新解析尾部
    fn summarize(&mut self, path: &Path, size: u64, mtime_ms: u64) -> Option<SessionSummary> {
        if let Some(hit) = self.cache.get(path) {
            if hit.size == size && hit.mtime_ms == mtime_ms {
                return Some(hit.summary.clone());
            }
        }
        let prev = self.cache.get(path).cloned();
        // 增量统计行数：只读上次大小之后的新增字节
        let line_count = match &prev {
            Some(p) if size >= p.size => p.line_count + count_lines_from(path, p.size).unwrap_or(0),
            _ => count_lines_from(path, 0).unwrap_or(0),
        };
        // 头部只解析一次（append-only 文件头部不变）
        let head = match &prev {
            Some(p) => p.head.clone(),
            None => parse_head(path),
        };

        let session_id = path.file_stem()?.to_string_lossy().to_string();
        let tail = read_tail(path, TAIL_BYTES).ok()?;
        let mut summary = parse_tail(&session_id, path, &tail)?;
        summary.mtime_ms = mtime_ms;
        summary.line_count = line_count;
        // 尾部窗口里可能没有真实用户提示词（长回合）：上次结果 → 头部初始提示词
        if summary.prompt.is_empty() {
            if let Some(p) = &prev {
                summary.prompt = p.summary.prompt.clone();
            }
        }
        if summary.prompt.is_empty() {
            if let Some(hp) = &head.prompt {
                summary.prompt = hp.clone();
            }
        }
        // 会话标题 = 头部首个用户提示词（原始任务）；缺失时回退当前提示词。
        // 开头的路径只留文件名（见 shorten_leading_paths）——标题在列表里只显示头 20 来字，
        // 目录前缀会把额度吃光。
        summary.title = shorten_leading_paths(
            &head
                .prompt
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| summary.prompt.clone()),
        );
        // 空壳会话（只有斜杠命令信封，见 [`SessionSummary::is_empty_shell`]）**此处不再拦**。
        //
        // 原先这里是一句 `return None`。它把「这条记录现在什么都没有」当成了「这条记录
        // 永远什么都不会有」—— 而 `/clear` 刚生出来的那份 jsonl 正好就是这个形状：只有一个
        // `/clear` 信封，几秒后用户一输入就有正文了。于是那条**活着的**新会话在解析层就被
        // 抹掉，后面所有依赖「看得见它」的环节（继任关系、进程迁移、状态判定）全部落空，
        // 没装 hook 的机器上 `/clear` 跟随 100% 失效。
        //
        // 「死壳子 vs 活会话」的差别不在内容，在**有没有进程占着它**，而解析器看不到进程。
        // 所以这道判断整体搬去 `build_tasks`（那里才同时握着会话与进程），此处一条不漏地
        // 产出，由产出任务的那一层决定谁该进列表。
        // 会话开始时间以头部第一条为准
        if head.started_at.is_some() {
            summary.started_at = head.started_at.clone();
        }
        // 尾部窗口没扫到规范 cwd（shell 漂移）时：上次结果 → 头部初始 cwd
        if encode_path(&summary.cwd) != summary.project_key {
            if let Some(p) = &prev {
                if encode_path(&p.summary.cwd) == summary.project_key {
                    summary.cwd = p.summary.cwd.clone();
                }
            }
        }
        if encode_path(&summary.cwd) != summary.project_key {
            if let Some(hc) = &head.cwd {
                summary.cwd = hc.clone();
            }
        }
        // 尾窗一条 cwd 都没读到（增量扫描时新行里没有、或极短会话）：沿用上一轮的结果，
        // 再退回项目根。**不能就这么留空** —— 调用方一见空就退回 `project`，等于每隔
        // 几轮上传落点就在「当前目录」和「项目根」之间跳一次，比一直用错更难查。
        if summary.shell_cwd.is_empty() {
            summary.shell_cwd = prev
                .as_ref()
                .map(|p| p.summary.shell_cwd.clone())
                .filter(|c| !c.is_empty())
                .unwrap_or_default();
        }
        // 锚定目录 = shell_cwd 所在的 git 仓库根。收到仓库根是为了稳定：shell_cwd 每轮都在
        // 跳，而仓库根怎么 cd 都不变。不在任何仓库里就用 shell_cwd 本身，再退项目根。
        //
        // 只在这里算（`summarize` 只有会话文件真变了才走到，缓存命中直接返回），
        // 所以 stat 父链的开销只落在活跃会话上，不是每轮每会话。
        summary.live_cwd = if summary.shell_cwd.is_empty() {
            summary.cwd.clone()
        } else {
            git_root_of(&summary.shell_cwd).unwrap_or_else(|| summary.shell_cwd.clone())
        };
        self.cache.insert(
            path.to_path_buf(),
            CacheEntry {
                size,
                mtime_ms,
                line_count,
                summary: summary.clone(),
                head,
            },
        );
        Some(summary)
    }

    /// 解析一个会话的对话消息 + 该会话名下的后台子任务。
    ///
    /// 消息取最后 `limit` 条；任务清单（todos）仍作为一条 `role:"todos"` 的状态快照
    /// 追加在末尾（它是一段 Markdown，本就按消息渲染）。
    ///
    /// **后台子任务不再混进消息流**：此前它被序列化成一条 `role:"bgtasks"` 的伪消息
    /// 塞在末尾，消费方得先把它从对话里摘出来、`JSON.parse`、再记得别把它渲染成聊天
    /// 气泡 —— 一份结构化数据伪装成一条消息，每个消费方都要复述一遍同样的绕法。
    /// 现在它单独返回，挂在 `Task::sub_tasks` 上。
    ///
    /// `parent_ended` = 这条会话已经没有进程在跑了。据此给它名下仍挂在「执行中」的
    /// 后台命令收尾（见 [`ORPHANED_STATUS`]）—— 调用方知道进程状态，解析器不知道。
    pub fn session_view(
        &mut self,
        session_id: &str,
        limit: usize,
        parent_ended: bool,
    ) -> Result<SessionView> {
        let path = self.find_session_file(session_id)?;
        let is_codex = path.starts_with(&self.codex_dir);

        // 对话流：只读尾部 8MB，足够渲染最近对话
        let tail = read_tail(&path, 8 * 1024 * 1024)?;
        let mut msgs = Vec::new();
        for line in tail.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let brief = if is_codex {
                codex_entry_to_brief(&v)
            } else {
                entry_to_brief(&v)
            };
            if let Some(m) = brief {
                msgs.push(m);
            }
        }
        let skip = msgs.len().saturating_sub(limit);
        let mut messages: Vec<MessageBrief> = msgs.into_iter().skip(skip).collect();
        if is_codex {
            // Codex 没有任务清单/后台任务语义，直接返回
            return Ok(SessionView {
                messages,
                sub_tasks: Vec::new(),
            });
        }

        // 状态快照：从会话开头增量重放得来，不受上面 limit 窗口影响。
        let ts = messages
            .last()
            .map(|m| m.timestamp.clone())
            .unwrap_or_default();
        let (todos, sub_tasks) = self.replay_state(&path, parent_ended, false)?;
        if let Some(m) = todos {
            messages.push(MessageBrief {
                role: "todos".into(),
                content: m,
                timestamp: ts,
                is_error: false,
                tools: Vec::new(),
                tool_use_id: String::new(),
            });
        }
        Ok(SessionView {
            messages,
            sub_tasks,
        })
    }

    /// 一条会话名下的**全部**子任务（不套保留窗口）。
    ///
    /// 与 `Task::sub_tasks` 的区别只在口径，不是另一套数据：
    /// - `Task::sub_tasks` 是**当前状态面板**——随快照每轮下发，只留近 24 小时、
    ///   最多 50 条终态，好让面板不被上周的东西淹掉；
    /// - 这个是**全量视角**——按需读盘，给「从历史列表点开一条五天前的会话，
    ///   把它的子会话展开来看」用。活跃会话调它同样成立，结果是前者的超集。
    ///
    /// 还会补上父记录漏掉的那些：阻塞式派活不写 `agentId`，只重放父记录会少一批，
    /// 而 `subagents/` 目录是全的（展示名从旁边的 `.meta.json` 取）。
    ///
    /// `parent_ended` 语义同 [`Self::session_view`]。
    pub fn sub_tasks_all(&mut self, session_id: &str, parent_ended: bool) -> Result<Vec<SubTask>> {
        let path = self.find_session_file(session_id)?;
        if path.starts_with(&self.codex_dir) {
            // Codex 没有子代理/后台任务语义
            return Ok(Vec::new());
        }
        Ok(self.replay_state(&path, parent_ended, true)?.1)
    }

    /// 解析**一个子会话**（异步子代理）的对话消息，返回最后 `limit` 条。
    ///
    /// 路径是 `<父会话 jsonl 同级>/<父会话号>/subagents/agent-<agentId>.jsonl`，格式与
    /// 父会话记录完全一致，所以复用同一个 [`entry_to_brief`] —— 消费方拿到的结构与
    /// `/messages` 一模一样，渲染逻辑不必分叉。
    ///
    /// 这是**按需**读盘：本机实测 422 份子会话记录，全量推上去是不可能的。
    pub fn subagent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<MessageBrief>> {
        // 防路径穿越：子会话号与会话号同一套字符集约束
        if agent_id.is_empty()
            || !agent_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            anyhow::bail!("非法子会话 ID");
        }
        let path = self.find_session_file(session_id)?;
        let dir = SubAgentDir::scan(&path);
        let file = dir.file_of(agent_id);
        if !file.is_file() {
            anyhow::bail!("子会话记录不存在");
        }
        let tail = read_tail(&file, 8 * 1024 * 1024)?;
        let mut msgs = Vec::new();
        for line in tail.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            // 子会话记录里每一条都是 sidechain，这里不能跳
            if let Some(m) = parse_entry(&v, false) {
                msgs.push(m);
            }
        }
        let skip = msgs.len().saturating_sub(limit);
        Ok(msgs.into_iter().skip(skip).collect())
    }

    /// 该会话的子会话记录最近一次被写入的时刻（epoch 毫秒，没有子会话就 0）。
    ///
    /// 给上层做缓存键用：[`Self::messages`] 末尾那条 `bgtasks` 快照里「子会话还在不在跑」
    /// 是由 `subagents/` 目录里的文件决定的，父会话 jsonl 一个字节没动，答案照样会变
    /// （子会话跑完时只有它自己那份记录在长）。只按父会话 mtime 做缓存会把这类变化
    /// 整个挡在门外 —— 现象就是父会话闲着时，跑完的子会话胶囊迟迟不消失。
    ///
    /// 只做一次 `read_dir` + `stat`，不读内容。
    pub fn subagents_mtime(&self, session_id: &str) -> u64 {
        self.find_session_file(session_id)
            .map(|p| SubAgentDir::scan(&p).newest_ms())
            .unwrap_or(0)
    }

    /// 增量重放任务清单与后台子任务：前者是一段 Markdown 快照，后者是结构化清单。
    /// 只解析上次之后新增的字节；文件被截断/轮转时从头重来。
    ///
    /// `parent_ended` = 父会话已经结束（没有进程在跑它）。为真时会给它名下仍挂在
    /// 「执行中」的后台命令收尾 —— 父会话都没了，它派生的后台命令不可能还在跑。
    /// `full` = 全量视角（不套保留窗口，且补上父记录漏掉的子会话）。
    fn replay_state(
        &mut self,
        path: &Path,
        parent_ended: bool,
        full: bool,
    ) -> Result<(Option<String>, Vec<SubTask>)> {
        let size = fs::metadata(path)?.len();
        let st = self.state_cache.entry(path.to_path_buf()).or_default();
        // 文件变小 = 被截断或换了内容，之前的重放结果作废
        if size < st.offset {
            *st = SessionState::default();
        }
        if size > st.offset {
            let mut f = fs::File::open(path)?;
            f.seek(SeekFrom::Start(st.offset))?;
            let mut buf = Vec::with_capacity((size - st.offset) as usize);
            f.read_to_end(&mut buf)?;
            // 只消费到最后一个换行为止：末尾那行可能正被写入，只有半截
            let end = match buf.iter().rposition(|b| *b == b'\n') {
                Some(p) => p + 1,
                None => 0,
            };
            let text = String::from_utf8_lossy(&buf[..end]);
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                st.todos.observe(&v);
                st.bg.observe(&v);
            }
            st.offset += end as u64;
        }
        // 强制产出当前态（dirty 只用于增量期间的去重，这里要的是全量快照）
        st.todos.dirty = true;
        // 子会话状态与父会话记录对齐后再出快照：父记录既漏派活（阻塞式子会话没有
        // agentId）也漏收尾（完成通知不保证写得下来），只有 subagents/ 目录是全的。
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let subs = SubAgentDir::scan(path);
        let parent_ended_ms = parent_ended.then(|| file_mtime_ms(path)).filter(|m| *m > 0);
        // 全量视角才把目录里那些父记录漏掉的子会话补进来：阻塞式派活不写 agentId，
        // 只看父记录会少一批。热路径那份维持原样，不多传一个字节。
        let extra = if full {
            subs.last_write
                .keys()
                .map(|id| {
                    new_sub_task(
                        id.clone(),
                        "agent",
                        subs.meta_label(id).unwrap_or_else(|| "子会话".to_string()),
                        "running".to_string(),
                        String::new(),
                        None,
                        0,
                        // 上游把起跑那次调用的 id 写在 sidecar 里，直接取，不必猜
                        subs.meta_str(id, "toolUseId").unwrap_or_default(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let bg = st.bg.reconciled(
            &subs.last_write,
            now_ms,
            &|id| subs.tail(id),
            ReconcileOpts {
                parent_ended_ms,
                full,
                extra,
            },
        );
        Ok((st.todos.take_snapshot("").map(|m| m.content), bg))
    }

    fn find_session_file(&self, session_id: &str) -> Result<PathBuf> {
        // 防路径穿越：session_id 只允许 uuid 字符
        if !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            anyhow::bail!("非法会话 ID");
        }
        if let Ok(projects) = fs::read_dir(&self.projects_dir) {
            for project in projects.flatten() {
                let candidate = project.path().join(format!("{session_id}.jsonl"));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        // Codex：rollout-<时间>-<会话号>.jsonl，按文件名后缀匹配
        fn find_codex(dir: &Path, session_id: &str, depth: usize) -> Option<PathBuf> {
            if depth > 4 {
                return None;
            }
            for e in fs::read_dir(dir).ok()?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if let Some(hit) = find_codex(&p, session_id, depth + 1) {
                        return Some(hit);
                    }
                } else if p
                    .file_stem()
                    .and_then(|x| x.to_str())
                    .map(|n| n.ends_with(session_id))
                    .unwrap_or(false)
                {
                    return Some(p);
                }
            }
            None
        }
        if self.codex_dir.is_dir() {
            if let Some(p) = find_codex(&self.codex_dir, session_id, 0) {
                return Ok(p);
            }
        }
        anyhow::bail!("会话 {session_id} 不存在")
    }
}

/// 「谁接替了谁」：新会话 id → **被它取代的**旧会话 id。
///
/// 只读会话元数据，**不看进程最终配给了谁** —— hook 自报（`pinned`）会直接把 pid 重指到
/// 新会话、从而跳过下面 clear-follow 那层配对，若靠配对结果回推，同一件事就会时有时无。
///
/// 判据：一条「生于 /clear」的会话（[`SessionSummary::clear_born`]），它的前任必然同时满足
/// 三件事 —— 同一个 `(provider, 项目)`、创建更早，且**最后一次写入正好发生在新会话诞生那一刻**。
/// 最后一条是 `/clear` 的副作用：它不往旧文件写正文，只顶一下 mtime（实测旧 jsonl 只多了一条
/// 无 timestamp 的 `cost-state`），此后再没人碰那个文件，时间戳就永久冻在那个瞬间。所以取
/// `|旧.mtime − 新.created|` 最小者即可，而且这个差值**不随时间漂移**：还在跑的旁路会话
/// mtime 一直往前走，只会越离越远。
fn supersession_map(sessions: &[SessionSummary]) -> HashMap<&str, &str> {
    // 容差只用来「压根没有前任时给出 None」—— 新终端起手第一句就 `/clear`、或旧会话已被删/
    // 落在扫描范围之外。**这道窗口必须窄**：认错前任的代价是把一条正跑着的兄弟会话判成
    // superseded，随即被全部配对层封锁，用户眼看着自己那条会话变 Finished。
    //
    // 5 秒这个数出自实测，不是拍的：本机 `~/.claude/projects` 71 份记录里 10 份生于 `/clear`，
    // 真前任的 `|旧.mtime − 新.created|` 分别是 4 / 4 / 7 / 27 / 33 / 34 / 40 / 42 / 60 毫秒，
    // 极端一例 4023 毫秒。5 秒盖得住那个极端值，同时把「同项目里另一条正在写的会话恰好落进
    // 窗口」的误判面收窄了 12 倍（原先是 60 秒 —— 一条活跃会话几乎必然在里面）。
    //
    // 真正区分多个候选的是下面那个「差值最小」，不是这道窗口。
    const CLEAR_MTIME_TOL_MS: i64 = 5_000;
    let mut out: HashMap<&str, &str> = HashMap::new();
    for s_new in sessions
        .iter()
        .filter(|s| s.clear_born && s.created_ms != 0)
    {
        let born = s_new.created_ms as i64;
        let mut best: Option<(i64, &SessionSummary)> = None;
        for old in sessions {
            if old.session_id == s_new.session_id
                || old.provider != s_new.provider
                || old.project_key != s_new.project_key
                || old.created_ms == 0
                || old.created_ms >= s_new.created_ms
            {
                continue;
            }
            let d = (old.mtime_ms as i64 - born).abs();
            if d > CLEAR_MTIME_TOL_MS {
                continue;
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, old));
            }
        }
        if let Some((_, old)) = best {
            out.insert(s_new.session_id.as_str(), old.session_id.as_str());
        }
    }
    out
}

/// **已经被接替、因而已经死了的会话**（[`supersession_map`] 的值集）。
///
/// 一条会话被接替，意思就是那个终端已经换到新会话上去了 —— 它不该再被配上任何进程，
/// 无论那个配对来自缓存、mtime 兜底，还是一条早已过期的 pin。
///
/// 对外暴露是因为客户端那边也要这个判断（累积 pin 表要据此淘汰陈旧条目）。**判断只有
/// 这一份**：此前客户端自己手写过一遍「同项目里出现了 created 更晚的 cleared 会话」，
/// 两套判据各自漂移，且都建在 `cleared` 上 —— 那个标志用户一输入就翻回 false，等于给
/// 淘汰按了个几秒的保质期，错过就永久把进程粘在清空前的旧会话上。
pub fn superseded_sessions(sessions: &[SessionSummary]) -> HashSet<&str> {
    supersession_map(sessions).into_values().collect()
}

/// 把会话摘要与进程信息聚合成任务
pub fn build_tasks(
    sessions: &[SessionSummary],
    processes: &[ProcessInfo],
    manual_paused: &dyn Fn(u32) -> bool,
    // pid -> session_id：由「进程打开着哪个会话文件」得出的确定配对（客户端注入）。
    // 有它就优先按它配。绝大多数情况为空（claude 不持续占文件）。
    pinned: &HashMap<u32, String>,
    // 「本轮相比上轮 mtime 有推进」的会话号集合 = 此刻正在被写的活跃会话。兜底配对只
    // 认这些 —— 刚关闭的会话 mtime 已冻结、不在其中，不会被重启的空白进程抢去显示旧内容。
    // active_ids 曾用于「按 mtime 推进兜底」，改为「只配自己创建的会话」后不再需要，
    // 保留形参以免动全部调用点。
    active_ids: &HashSet<String>,
    // 上一轮的稳定配对（pid → session_id）：让长时间闲置的会话保持配对、不掉成
    // 「等待输入」占位。仅在本轮没有更强信号（①②③④）配上该进程时兜底沿用。
    cached: &HashMap<u32, String>,
) -> Vec<Task> {
    let _ = active_ids;
    // 按 cwd 分组进程（已按启动时间升序）
    // 进程按「编码后的 cwd」分组，与会话的项目目录名对齐（会话内 cwd 会漂移，目录名不会）
    // 按 (provider, cwd-key) 分组：各 provider 的会话只与同类进程配对
    let mut proc_by_key: HashMap<(String, String), Vec<&ProcessInfo>> = HashMap::new();
    for p in processes {
        // 共享宿主进程（桌面客户端的 app-server）不按 cwd 配：它一个进程托着多条会话，
        // 自己的 cwd 恒为 `/`，跟哪条会话都对不上。它走下面的桌面配对层。
        if p.shared_host {
            continue;
        }
        proc_by_key
            .entry((p.agent.clone(), encode_path(&p.cwd)))
            .or_default()
            .push(p);
    }

    // 同一 (provider, 项目) 下的活跃会话，按最后活动时间与进程配对（排序见下方）
    let mut sess_by_key: HashMap<(String, String), Vec<&SessionSummary>> = HashMap::new();
    // 只有「近期活跃」的会话才参与进程配对（太老的会话大概率已结束）。
    // 窗口须足够宽：IDE 里挂着的会话闲置大半天很常见（午休/过夜），
    // 6 小时窗口会让活着的会话配不到进程 → 判 Finished，同时进程沦为
    // 「会话尚未产生记录」占位 —— 一个会话变两条。7 天足以覆盖长挂会话，
    // 配对时按最近活动排序 + zip 截断，也不会把真正的死会话捞回来。
    let now = now_ms();
    for s in sessions {
        if now.saturating_sub(s.mtime_ms) < LIVE_WINDOW_MS {
            sess_by_key
                .entry((s.provider.clone(), s.project_key.clone()))
                .or_default()
                .push(s);
        }
    }
    // 按「最后活动时间」倒序：真正还在跑的排前面。
    //
    // 这里绝不能按 started_at 排：会话的起始时间是文件里第一条记录的时间，
    // 而 `claude --resume` 起来的会话可能是几周前开的 —— 它此刻活得好好的，
    // 起始时间却比一个刚开没多久、但早就没人管的会话还老。按起始时间挑，
    // 活着的长会话会被死会话挤掉：配不到进程 → 判为 Finished → 从列表里消失，
    // 而那个死会话反倒顶着 pid 一直显示「等待输入」，内容永远不更新。
    for list in sess_by_key.values_mut() {
        list.sort_by_key(|b| std::cmp::Reverse(b.mtime_ms));
    }

    let mut pid_of_session: HashMap<&str, &ProcessInfo> = HashMap::new();
    let mut paired_pids: HashSet<u32> = HashSet::new();
    // pid→进程、session_id→会话：全函数各建一份，供各配对层共用，避免每层重建映射或线性 find
    let proc_by_pid: HashMap<u32, &ProcessInfo> = processes.iter().map(|p| (p.pid, p)).collect();
    let sid_index: HashMap<&str, &SessionSummary> = sessions
        .iter()
        .map(|s| (s.session_id.as_str(), s))
        .collect();
    // 继任关系：本函数里**唯一**的一份（下面的 clear-follow 迁移与产出的
    // `Task::supersedes` 都读它），不许再各算一套。
    let succ = supersession_map(sessions);
    // 已经被接替、因而已经死了的会话。**贯穿全函数的一条硬规则：它们不配进程**。
    //
    // 这条规则取代了原先那套「谁迁走了就把谁记进 released」的写法。原写法只在迁移真的
    // 发生时才封住旧会话，于是缓存一空（客户端刚重启）就没人封它，tier② 立刻按「created
    // 最早」把进程粘回旧会话 —— 正是「网页停在清空前」的那一手。既然「被接替」这件事
    // 只看会话元数据就能算出来（与配没配上进程无关），封锁就该无条件生效。
    let superseded: HashSet<&str> = succ.values().copied().collect();

    // 第一优先：按「进程打开着哪个会话文件」得出的确定配对（pinned）。
    // 这能解决「关闭的会话 mtime 反而更新、抢走了活进程」——因为已关闭会话的文件
    // 没有活进程占着，压根不会出现在 pinned 里；闲置但仍开着的会话则会被正确配上。
    //
    // 被接替的会话连 pin 也不认：pin 表是累积的，`/clear` 换会话本身不产生新 pin
    //（除非随后又跑了工具），表里那条旧 sid 会一直以最高优先级把进程拽回清空前的会话。
    if !pinned.is_empty() {
        for (pid, sid) in pinned {
            if superseded.contains(sid.as_str()) {
                continue;
            }
            if let (Some(p), Some(s)) = (proc_by_pid.get(pid), sid_index.get(sid.as_str()).copied())
            {
                pid_of_session.insert(s.session_id.as_str(), *p);
                paired_pids.insert(*pid);
            }
        }
    }

    // clear-follow：claude 执行 /clear 会另起一份全新的 jsonl（新 sessionId），同一进程从旧
    // 会话转到它。但下面 tier② 会按「created 最早」把进程粘回它启动时创建的旧会话 → 网页
    // 内容/标题定格在清空前。这两层负责把进程搬到继任会话上。
    //
    // **判据一律取 `succ`（继任关系），不再取 `cleared`。** `cleared` 的含义是「刚清空、
    // 还没输入」，用户一敲字就翻回 false —— 等于给「跟着 /clear 走」按了个几秒钟的保质期，
    // 扫描周期（1.5 秒）稍微错过一拍，这件事就再也不会发生了。继任关系没有保质期。
    // clear-follow (a) 保持：上一轮已迁到继任会话的进程，本轮继续粘住它，抢在 tier② 之前。
    // 否则——缓存本轮已指向新会话、(b) 不会再迁，空闲的进程会被 tier② 按「created 最早」
    // 又拽回旧会话；下一轮缓存又变回旧会话、(b) 再迁到新…… 于是进程在 旧↔新 间每轮抖动，
    // 表现为卡片标题/内容闪烁（旧会话有标题 ↔ 新空会话只剩项目名）。粘住即止住抖动。
    //
    // 它自己也被接替了（连着 /clear 两次）就不许再粘 —— 否则 (b) 见 pid 已配对直接跳过，
    // 第二次 /clear 就跟不过去了。
    for (pid, sid) in cached {
        if paired_pids.contains(pid) || pid_of_session.contains_key(sid.as_str()) {
            continue;
        }
        if !succ.contains_key(sid.as_str()) || superseded.contains(sid.as_str()) {
            continue;
        }
        let Some(s) = sid_index.get(sid.as_str()).copied() else {
            continue;
        };
        if let Some(p) = proc_by_pid.get(pid) {
            pid_of_session.insert(s.session_id.as_str(), *p);
            paired_pids.insert(*pid);
        }
    }
    // clear-follow (b) 迁移：收集本轮有前任、却还没配上进程的会话；没有就整层跳过，
    // 额外开销只落在真发生过 /clear 的轮次。
    //
    // 这一层在「同项目开着多个终端」时不可替代：只有它知道「上一轮是**这台** pid 配在
    // 前任身上」，而 tier② 只会按创建时间挑，两个终端一交叉就串台。
    let mut fresh: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| {
            succ.contains_key(s.session_id.as_str())
                && !pid_of_session.contains_key(s.session_id.as_str())
        })
        .collect();
    if !fresh.is_empty() {
        fresh.sort_by_key(|s| s.created_ms); // 按 created 升序稳定处理
        for s_new in fresh {
            // 前任是谁由 `succ` 说了算（上面独立算过一次，与 pid 配对无关）；
            // 这一层只负责把**前任那台进程**搬过来。
            let Some(old) = succ
                .get(s_new.session_id.as_str())
                .and_then(|sid| sid_index.get(*sid).copied())
            else {
                continue;
            };
            // 上一轮配在前任身上、此刻还活着且尚未被更强信号配走的那台进程
            let mut pick: Option<u32> = None;
            for (pid, sid) in cached {
                if sid.as_str() != old.session_id || paired_pids.contains(pid) {
                    continue;
                }
                let Some(p) = proc_by_pid.get(pid) else {
                    continue;
                };
                if p.agent != s_new.provider || encode_path(&p.cwd) != s_new.project_key {
                    continue;
                }
                pick = Some(*pid);
                break;
            }
            if let Some(pid) = pick {
                pid_of_session.insert(s_new.session_id.as_str(), proc_by_pid[&pid]);
                paired_pids.insert(pid);
            }
        }
    }

    // pinned 没覆盖到的（绝大多数——claude 并不持续占着会话文件，写一行开一次就关，
    // lsof/RmGetList 抓不到），按下面几级信号在同项目内配对：
    for (key, procs) in &proc_by_key {
        if let Some(sess) = sess_by_key.get(key) {
            let mut free_procs: Vec<&ProcessInfo> = procs
                .iter()
                .rev()
                .filter(|p| !paired_pids.contains(&p.pid))
                .copied()
                .collect();
            let mut free_sess: Vec<&SessionSummary> = sess
                .iter()
                .filter(|s| !pid_of_session.contains_key(s.session_id.as_str()))
                .copied()
                .collect();

            // ① 命令行 --resume <id>：恢复指定会话（创建于很久前，靠命令行认出）。
            //
            // **这一层走在「被接替的会话不配进程」前面**：命令行是用户亲口说的，
            // 「我要接着跑这条」压过任何由文件时间戳推出来的判断。被 /clear 甩掉的老会话
            // 过几天照样能被 `--resume` 捞回来接着用，那一刻它就不再是死的了。
            free_procs.retain(|p| {
                if let Some(rid) = resume_session_id(&p.command) {
                    if let Some(pos) = free_sess.iter().position(|s| s.session_id == rid) {
                        let s = free_sess.remove(pos);
                        pid_of_session.insert(s.session_id.as_str(), p);
                        paired_pids.insert(p.pid);
                        return false;
                    }
                }
                true
            });
            // ②③④ 之前统一封锁：已被接替的会话不再参与任何启发式配对（见 `superseded`）
            free_sess.retain(|s| !superseded.contains(s.session_id.as_str()));

            // ② 进程只配「自己创建的会话」：进程一定先于它创建的会话，且 start_time 向下取整
            // ≤ 真实启动，故「会话 created_ms ≥ 进程 start」恒成立。回看窗口必须≈0，只留 0.5s
            // 兜文件系统时间戳粒度。曾用 2min→仍错配，2s→仍不够：若会话在进程启动后几秒才落盘
            // （首条消息略慢），另一个晚 2s 内启动的进程会把它抢走（diff 落在 -2s 内），贪心取
            // |diff| 最小就张冠李戴（实测 Cursor 多终端 /clear 发到「上一个会话」的终端）。收到
            // 0.5s 后，早于本进程启动创建的会话被彻底排除，各进程只配自己启动后创建的那条。
            const CREATE_BACK_MS: i64 = 500;
            const CREATE_FWD_MS: i64 = 4 * 3600 * 1000;
            // 进程按启动升序，逐个认领「创建时间 ≥ 自身启动(容 0.5s)、且尚未被认领的最早
            // 会话」。为什么不用「|diff| 最小贪心」：那会让晚启动的进程把早启动进程的会话抢走
            // ——只要那条会话的创建时间恰好离晚进程更近（会话首条消息略慢落盘时常发生），就
            // 张冠李戴，表现为「当前会话下发到上一个会话的终端」。按启动序 + 认领最早后继会话，
            // 早开的进程先挑走它自己那条（最早创建的后继），晚开的进程只能拿更晚的，天然不串。
            let mut proc_order: Vec<usize> = (0..free_procs.len())
                .filter(|&i| free_procs[i].start_time != 0)
                .collect();
            proc_order.sort_by_key(|&i| free_procs[i].start_time);
            let mut used_sess = vec![false; free_sess.len()];
            let mut used_proc = vec![false; free_procs.len()];
            for &pi in &proc_order {
                let p_ms = (free_procs[pi].start_time as i64) * 1000;
                let mut best: Option<usize> = None;
                let mut best_created = i64::MAX;
                for (si, s) in free_sess.iter().enumerate() {
                    if used_sess[si] || s.created_ms == 0 {
                        continue;
                    }
                    let diff = s.created_ms as i64 - p_ms;
                    if (-CREATE_BACK_MS..=CREATE_FWD_MS).contains(&diff)
                        && (s.created_ms as i64) < best_created
                    {
                        best_created = s.created_ms as i64;
                        best = Some(si);
                    }
                }
                if let Some(si) = best {
                    used_sess[si] = true;
                    used_proc[pi] = true;
                    pid_of_session.insert(free_sess[si].session_id.as_str(), free_procs[pi]);
                    paired_pids.insert(free_procs[pi].pid);
                }
            }
            free_procs = free_procs
                .iter()
                .enumerate()
                .filter(|(i, _)| !used_proc[*i])
                .map(|(_, p)| *p)
                .collect();
            free_sess = free_sess
                .iter()
                .enumerate()
                .filter(|(i, _)| !used_sess[*i])
                .map(|(_, s)| *s)
                .collect();

            // ③ 命令行 --continue（恢复最近改动的会话，无显式 id）：配给剩余里 mtime 最近
            // 的会话（free_sess 是 mtime 降序）。没有 --resume/--continue、也没有自己新建会话
            // （created≈start）的进程 —— 如 Cursor 里刚开、还没发消息的空白终端 —— 就留作
            // 空白占位（会话尚未产生记录），绝不无差别按 mtime 硬配去抢旧会话。
            //
            // 空壳排除见下面 ④ 处的说明：③④ 是纯启发式，够不到「这台进程刚 /clear 过」这个
            // 依据，挑中空壳只会给另一个终端一个空标题的格子。
            free_procs.retain(|p| {
                if wants_continue(&p.command) {
                    if let Some(pos) = free_sess.iter().position(|s| !s.is_empty_shell()) {
                        let s = free_sess.remove(pos);
                        pid_of_session.insert(s.session_id.as_str(), p);
                        paired_pids.insert(p.pid);
                        return false;
                    }
                }
                true
            });

            // ④ 剩余进程配「最后写入不早于本进程启动」的会话：覆盖续跑/压缩恢复的长会话，
            // 以及**闲置很久但进程仍活着**的会话 —— 会话文件建于很久前、进程比它晚启动、命令行
            // 也无 --continue（Claude Code 自动续跑正是如此）。
            //
            // 关键约束 s.mtime >= 进程启动：一个会话若最后一次写入发生在进程启动【之前】，
            // 那这段内容必然是上一个进程留下的（典型：某进程 --resume 了老会话、写了几句后
            // 退出；用户又在同目录开一个全新空白终端）。此时新空白进程绝不能凭 mtime 新就把
            // 那条老会话抢过来一直显示旧内容 —— 它没写过那个文件。放进占位（会话尚未产生记录）
            // 才对。进程 start_time 只精确到秒、向下取整（≤ 真实启动），对「进程启动后才写入」
            // 的活跃会话恒成立，不会误伤；只挡住启动前就停笔的旧会话。free_sess 是 mtime 降序。
            //
            // 不再额外卡「最近 30min 活跃」窗口：闲置的会话 mtime 会冻结，30min 一到它就掉出
            // ④、进程沦为空白占位、会话被判 Finished（表现为「闲太久被当关闭、又冒出空白终端」）。
            // 安全性完全由上面的 mtime>=启动 约束保证，与活跃间隔无关；会话集合本身已卡回溯窗口。
            //
            // **空壳会话不参与 ③④**（见 [`SessionSummary::is_empty_shell`]）。空壳留在
            // `sessions` 里是为了让「刚 /clear 出来的那条活会话」有落点，而它该被谁认领是有
            // 明确依据的：pin 自报、clear-follow 按缓存迁移、tier② 的「进程只配自己创建的
            // 会话」、或命令行 `--resume` 点名。③④ 是纯 mtime 启发式，拿不到那个依据 ——
            // 同项目另开一个终端，就可能凭「mtime 最新」把这条空壳挑走，用户得到一个没有
            // 标题、也不属于他那个终端的格子。这个口子是本次「不再在解析层丢空壳」新开的，
            // 从前空壳压根不在 `sessions` 里，③④ 够不着它。
            let mut free_sess: Vec<&SessionSummary> = free_sess
                .into_iter()
                .filter(|s| !s.is_empty_shell())
                .collect();
            // 新进程优先认领新会话：按启动时间降序，避免老进程抢走更晚的会话文件
            free_procs.sort_by_key(|b| std::cmp::Reverse(b.start_time));
            for p in free_procs {
                let p_start_ms = p.start_time.saturating_mul(1000);
                // free_sess 已按 mtime 降序：第一条满足「mtime≥启动」的即该进程可认领的最新会话
                if let Some(pos) = free_sess.iter().position(|s| s.mtime_ms >= p_start_ms) {
                    let s = free_sess.remove(pos);
                    pid_of_session.insert(s.session_id.as_str(), p);
                    paired_pids.insert(p.pid);
                }
            }
        }
    }

    // ⑤ 缓存兜底：仍没配上会话的存活进程，若上一轮它配过某会话、该会话还在（7 天内）、
    // 且本轮没被更强信号（①-④）配给别的进程 —— 就沿用上一轮的配对。专治「长时间闲置的
    // 会话（>30min 够不着 phase④，命令行又无 --resume）掉成『等待输入』占位、唤醒后又冒出
    // 一条新会话」：配对一旦建立就粘住，只要进程活着、会话还在，就不再翻来覆去。
    if !cached.is_empty() {
        for (pid, sid) in cached {
            if paired_pids.contains(pid) {
                continue;
            }
            if pid_of_session.contains_key(sid.as_str()) {
                continue; // 该会话本轮已被别的进程配走（如 /clear 后进程改配新会话）
            }
            if superseded.contains(sid.as_str()) {
                continue; // 已被接替 → 死的，别靠缓存把进程拽回去
            }
            if let (Some(p), Some(s)) = (
                proc_by_pid.get(pid),
                sid_index
                    .get(sid.as_str())
                    .copied()
                    .filter(|s| now.saturating_sub(s.mtime_ms) < LIVE_WINDOW_MS),
            ) {
                pid_of_session.insert(s.session_id.as_str(), *p);
                paired_pids.insert(*pid);
            }
        }
    }

    // ⑥ 桌面客户端：共享宿主进程 ↔ 它这一轮托着的会话。
    //
    // 前面几层全是「一进程一会话、cwd 即项目」的终端模型，桌面客户端不是这样：
    // ChatGPT 桌面版只有一个 `codex … app-server`，同时托着界面上的每一条对话，
    // 而且它的 cwd 恒为 `/`。所以这里既不比 cwd、也不做一一对应，改问两件事：
    // 会话自己说了它来自桌面客户端（`originator`），以及**这条会话在本次 App 运行期间
    // 被写过**（mtime ≥ 宿主进程启动）。
    //
    // 后一条就是 tier④ 那个「会话最后写入不能早于进程启动」的约束，用意也一样：
    // 上次开 App 时留下的旧对话，这次没碰过，不该顶着 pid 显示成「等待输入」。
    for h in processes.iter().filter(|p| p.shared_host) {
        let host_start_ms = h.start_time.saturating_mul(1000);
        for s in sessions {
            if !s.desktop || s.provider != h.agent {
                continue;
            }
            if s.mtime_ms < host_start_ms {
                continue;
            }
            if pid_of_session.contains_key(s.session_id.as_str())
                || superseded.contains(s.session_id.as_str())
            {
                continue;
            }
            pid_of_session.insert(s.session_id.as_str(), h);
            paired_pids.insert(h.pid);
        }
    }

    let mut tasks = Vec::new();
    for s in sessions {
        let proc_info = pid_of_session
            .get(s.session_id.as_str())
            .map(|p| (*p).clone());
        // **空壳会话只在没有进程占着时才是垃圾**（见 [`SessionSummary::is_empty_shell`]）。
        //
        // 死壳子：用户 `/clear` 或 `/model` 之后直接关了终端，留下一份只有命令信封的 jsonl，
        // 列在历史里就是一串一模一样的空白行（实测本机 `~/.claude/projects` 71 份记录里
        // 有 10 份是这种壳子）。这种丢掉。
        //
        // 活会话：`/clear` 刚生出来的那一份长得一模一样，可它此刻正被一台 claude 占着，
        // 用户下一句话就写进去了。这种必须留 —— 丢了它，继任关系、进程迁移、状态判定
        // 全都没有落点，没装 hook 的机器上 `/clear` 跟随就整条断掉。
        //
        // 这道判断原先在 `summarize` 里（一句 `return None`），那一层看不见进程，只能按
        // 「此刻有没有内容」一刀切，于是把活的和死的一起砍了。搬到这里才分得开。
        if proc_info.is_none() && s.is_empty_shell() {
            continue;
        }
        let status = match &proc_info {
            None => TaskStatus::Finished,
            Some(p) => {
                if manual_paused(p.pid) {
                    // 我方主动暂停 → 已暂停
                    TaskStatus::Paused
                } else if crate::process::is_stopped(p.pid) {
                    // 被系统挂起但非我方暂停（后台进程读终端被 SIGTTIN 停住）→ 孤儿，视为已结束
                    TaskStatus::Finished
                } else if s.turn_ended {
                    TaskStatus::Idle
                } else {
                    TaskStatus::Running
                }
            }
        };
        let status_dsr = status.dsr().to_string();
        tasks.push(Task {
            id: s.session_id.clone(),
            machine_id: String::new(),
            hostname: String::new(),
            platform: String::new(),
            platform_dsr: String::new(),
            provider: s.provider.clone(),
            provider_dsr: if s.desktop {
                crate::model::provider_dsr_desktop(&s.provider)
            } else {
                crate::model::provider_dsr(&s.provider)
            },
            // 「哪个客户端」在这一层是个布尔量，带上去 —— 上层才不必去抠展示字符串
            desktop: s.desktop,
            title: if s.title.is_empty() {
                s.prompt.clone()
            } else {
                s.title.clone()
            },
            used_tokens_5h: s.used_tokens_5h,
            token_limit: 0,
            auto_paused: false,
            status_dsr,
            ide_dsr: proc_info
                .as_ref()
                .map(|p| p.ide_name.clone())
                .unwrap_or_else(|| "—".into()),
            pid: proc_info.as_ref().map(|p| p.pid),
            // 这条会话接替了谁：① `/clear` 的前任会话；② 没有前任、但配上了进程 —— 那它
            // 顶掉的就是这个 pid 的占位任务（`pid-<pid>`，会话文件落盘前列表里的那条）。
            // 两者都是「同一个终端换了任务 id」，消费方跟随的判据是同一个，不必分开处理。
            // 共享宿主（桌面客户端的 app-server）排除在②之外：它一个进程托着多条会话，
            // 本来就不会有占位任务，指过去只会得到一个列表里不存在的 id。
            supersedes: succ
                .get(s.session_id.as_str())
                .map(|sid| (*sid).to_string())
                .or_else(|| {
                    proc_info
                        .as_ref()
                        .filter(|p| !p.shared_host)
                        .map(|p| format!("pid-{}", p.pid))
                }),
            project: s.cwd.clone(),
            project_name: short_name(&s.cwd),
            // 与 project 不同时才有意义（会话 cd 进了子目录）；相同就当没有，
            // 让调用方走 project 那条老路，少一个可能对不上的来源。
            live_cwd: (!s.live_cwd.is_empty() && s.live_cwd != s.cwd).then(|| s.live_cwd.clone()),
            prompt: s.prompt.clone(),
            last_action: s.last_action.clone(),
            status,
            started_at: s.started_at.clone(),
            last_active_at: s.last_active_at.clone(),
            mtime_ms: s.mtime_ms,
            line_count: s.line_count,
            version: s.version.clone(),
            git_branch: s.git_branch.clone(),
            process: proc_info,
            recent_messages: Vec::new(),
            sub_tasks: Vec::new(),
            sub_task_count: s.sub_agent_count,
            queued_inputs: s.queued_inputs.clone(),
            // hook 侧的实时信号，扫描器看不到；由客户端在配对后回填（见 client/state.rs）
            pending_select: None,
        });
    }

    // 没配到会话文件的代理进程（刚启动尚未落盘，或 Gemini/Aider 等暂无
    // 解析器）→ 每个进程恰好一条「进程任务」：标题给 provider + 项目目录，
    // 控制（暂停/恢复/中断/终止走信号）完全可用，只是没有对话流。
    // 注意：必须只有这一个循环 —— 历史上这里有 pid-/proc- 两个循环、
    // 判断条件等价，每个未配对进程会重复出现两次。
    for p in processes {
        if paired_pids.contains(&p.pid) {
            continue;
        }
        // 共享宿主进程本身不是一条会话：它 cwd 恒为 `/`，给它发一张占位卡就是
        // 「同一个终端号下多出一条空会话」那个老毛病的翻版。桌面客户端此刻没有活动会话
        // （或会话都是上次运行留下的）时，它就该一张卡都不出。
        if p.shared_host {
            continue;
        }
        // 被系统挂起（非我方暂停）的占位进程判为孤儿，前台会过滤掉
        if crate::process::is_stopped(p.pid) && !manual_paused(p.pid) {
            continue;
        }
        // 标题：会话标题拿不到（占位任务本来就没有）就只显示模型名；
        // 项目目录名由分组/副行展示，不塞进标题
        let title = crate::model::provider_dsr(&p.agent);
        let status = if manual_paused(p.pid) {
            TaskStatus::Paused
        } else {
            TaskStatus::Idle
        };
        tasks.push(Task {
            // id 用 pid- 前缀：attach_machine 会给它加机器前缀防跨机冲突
            id: format!("pid-{}", p.pid),
            machine_id: String::new(),
            hostname: String::new(),
            platform: String::new(),
            platform_dsr: String::new(),
            provider: p.agent.clone(),
            provider_dsr: crate::model::provider_dsr(&p.agent),
            // 进程占位任务只从进程表来，没有会话文件可判来源，一律按终端 CLI 算
            desktop: false,
            title,
            used_tokens_5h: 0,
            token_limit: 0,
            auto_paused: false,
            status_dsr: status.dsr().to_string(),
            ide_dsr: p.ide_name.clone(),
            pid: Some(p.pid),
            // 占位任务是这条链的**起点**，它没有前任
            supersedes: None,
            project: p.cwd.clone(),
            project_name: short_name(&p.cwd),
            // 占位任务只有进程、没有会话记录，谈不上「会话此刻在哪」——
            // 进程 cwd 就是全部信息，已经在 project 里了。
            live_cwd: None,
            prompt: "（会话尚未产生记录）".into(),
            last_action: "等待输入".into(),
            status,
            started_at: None,
            last_active_at: None,
            mtime_ms: p.start_time * 1000,
            line_count: 0,
            version: None,
            git_branch: None,
            process: Some(p.clone()),
            recent_messages: Vec::new(),
            sub_tasks: Vec::new(),
            // 进程占位任务还没有会话文件，自然也没有子代理目录
            sub_task_count: 0,
            queued_inputs: Vec::new(),
            // 这是「只有进程、没配上会话」的占位任务，压根谈不上等你选
            pending_select: None,
        });
    }

    tasks.sort_by(|a, b| {
        // 活跃在前，其余按最近活动倒序
        let rank = |t: &Task| match t.status {
            TaskStatus::Running => 0,
            TaskStatus::Paused => 1,
            TaskStatus::Idle => 2,
            TaskStatus::Finished => 3,
        };
        rank(a).cmp(&rank(b)).then(b.mtime_ms.cmp(&a.mtime_ms))
    });
    tasks
}

// ---------- jsonl 解析 ----------

fn parse_tail(session_id: &str, path: &Path, tail: &str) -> Option<SessionSummary> {
    // 目录名即项目 key；Windows 下统一小写，与 encode_path 的同一化对齐（大小写不敏感）
    let project_key = normalize_key_case(
        path.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    let mut cwd = String::new();
    // 会话记录里的 cwd 会跟随 shell 漂移；以「编码后等于项目目录名」的 cwd 为准
    let mut canonical_cwd = String::new();
    // 漂移后的**最新** cwd：终端解析 `./x` 用的是它。归一化到 canonical 是为了配对稳定，
    // 但拿归一化结果当上传落点就会写到别的目录去（见 model 的 `Task::live_cwd`）。
    let mut live_cwd = String::new();
    let mut prompt = String::new();
    let mut last_action = String::new();
    let mut turn_ended = false;
    // 尾窗里见过 /clear 命令块 —— 即「这条会话生于某次 /clear」，见 SessionSummary::clear_born。
    // **只置位、不复位**：它是既成事实。「刚清空、还没输入」那个更窄的判断是
    // `cleared = saw_clear && prompt.is_empty()`，由 prompt 那一半负责随输入翻掉。
    let mut saw_clear = false;
    // 这条会话里有没有任何实质内容（真实用户输入 / 助手回复），见 SessionSummary::has_content
    let mut has_content = false;
    // 忠实回放 claude 原生输入队列：(匹配键=原始 content, 展示文本=Some 时才是真实用户
    // 输入)。通知类（task-notification 等）也占位（展示文本 None），这样按位置的「空
    // content 出列」能对上正确的项，最终只把「真实用户输入」拿去展示。
    let mut queue: Vec<(String, Option<String>)> = Vec::new();
    let mut started_at: Option<String> = None;
    let mut last_active_at: Option<String> = None;
    let mut version: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut used_tokens_5h: u64 = 0;
    let window_start_ms = now_ms().saturating_sub(5 * 3600 * 1000);
    // 「正等你选」的了结信号：尚无结果的 AskUserQuestion 的 tool_use id + 它被了结的时刻
    let mut open_ask: Option<String> = None;
    let mut select_answered_ms: Option<u64> = None;

    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(c) = v.get("cwd").and_then(Value::as_str) {
            if cwd.is_empty() {
                cwd = c.to_string();
            }
            if canonical_cwd.is_empty() && encode_path(c) == project_key {
                canonical_cwd = c.to_string();
            }
            // 每条都覆盖 —— 要的就是尾窗里**最后**那条。空串不算（有的记录带个空 cwd，
            // 认了它等于把已知的工作目录抹成未知）。
            if !c.is_empty() {
                live_cwd = c.to_string();
            }
        }
        if let Some(ver) = v.get("version").and_then(Value::as_str) {
            version = Some(ver.to_string());
        }
        if let Some(gb) = v.get("gitBranch").and_then(Value::as_str) {
            if !gb.is_empty() {
                git_branch = Some(gb.to_string());
            }
        }
        if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
            if started_at.is_none() {
                started_at = Some(ts.to_string());
            }
            last_active_at = Some(ts.to_string());
        }
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "user" => {
                if v.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
                    || v.get("isSidechain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    continue;
                }
                let content = v.pointer("/message/content");
                // /clear 命令：内容形如 "<command-name>/clear</command-name>..."。清空后
                // claude 另起的新会话开头就是它。标记见过，配合「无真实 prompt」判 cleared。
                if content
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.contains("<command-name>/clear</command-name>"))
                {
                    saw_clear = true;
                }
                // 选择卡的了结：作答会落下带 tool_use_id 的 tool_result；按 Esc 打断则
                // 只有中断标记、永远等不到结果 —— 两者都意味着这张卡不该再挂在远端。
                if let Some(id) = &open_ask {
                    if content_answers(content, id) || is_interrupt_marker(content) {
                        select_answered_ms = v
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .and_then(iso_to_ms);
                        open_ask = None;
                    }
                }
                if is_interrupt_marker(content) {
                    // 在终端里按 Esc 中断 → 这一轮就此打住，回到等待输入。
                    // 必须显式判定：中断记录被 user_text 当系统内容滤掉（对的，它不是
                    // 用户发言），若不在这里收口，turn_ended 会保持中断前的值 —— 那时
                    // 最后一条是 assistant 带 tool_use，即 false，于是会话永远停在
                    // 「正在调用工具」，界面一直显示执行中，而终端早已在等你。
                    turn_ended = true;
                    last_action = "已中断".into();
                } else if let Some(text) = user_text(content) {
                    has_content = true;
                    prompt = text;
                    last_action = "等待助手响应".into();
                    turn_ended = false;
                } else if content_has_tool_result(content) {
                    turn_ended = false;
                }
            }
            "queue-operation" => match v.get("operation").and_then(Value::as_str) {
                Some("enqueue") => {
                    let key = v
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    let disp = queued_user_text(&v);
                    if let Some(text) = &disp {
                        prompt = text.clone();
                        last_action = "等待助手响应".into();
                        turn_ended = false;
                    }
                    queue.push((truncate(key, 500), disp));
                }
                // 出列（被会话接受执行 或 取消）：content 有值→按内容精确移除（匹配不到
                // 再退移队首）；content 为空→移除队首（FIFO，最旧的先被接受）。空 content
                // 的 remove 之前被 queued_user_text 滤成 None、什么都不做，已接受/撤回的项
                // 因此卡在队列里一直显示「排队中」不消失。
                Some("remove") => {
                    let key = v
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim();
                    if !key.is_empty() {
                        let k = truncate(key, 500);
                        if let Some(pos) = queue.iter().position(|(c, _)| c == &k) {
                            queue.remove(pos);
                        } else if !queue.is_empty() {
                            queue.remove(0);
                        }
                    } else if !queue.is_empty() {
                        queue.remove(0);
                    }
                }
                // 会话接受队首执行（content 恒空）：移除队首
                Some("dequeue") => {
                    if !queue.is_empty() {
                        queue.remove(0);
                    }
                }
                // 全部弹出（终端按 Esc 把排队全部插入会话）：清空
                Some("popAll") => {
                    queue.clear();
                }
                _ => {}
            },
            "assistant" => {
                has_content = true;
                // 统计 5h 窗口内 token 用量（input+output+cache_creation）
                if let Some(u) = v.pointer("/message/usage") {
                    let in_window = v
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(iso_to_ms)
                        .map(|ms| ms >= window_start_ms)
                        .unwrap_or(true);
                    if in_window {
                        let t = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                        used_tokens_5h += t("input_tokens")
                            + t("output_tokens")
                            + t("cache_creation_input_tokens");
                    }
                }
                if let Some(items) = v.pointer("/message/content").and_then(Value::as_array) {
                    let mut tool_names = Vec::new();
                    let mut has_text = false;
                    for item in items {
                        match item.get("type").and_then(Value::as_str) {
                            Some("tool_use") => {
                                if let Some(n) = item.get("name").and_then(Value::as_str) {
                                    // 新一张选择卡：在见到它的结果之前，之前那次的了结时刻作废
                                    if n == "AskUserQuestion" {
                                        open_ask = item
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .map(|s| s.to_string());
                                        select_answered_ms = None;
                                    }
                                    tool_names.push(n.to_string());
                                }
                            }
                            Some("text") => has_text = true,
                            _ => {}
                        }
                    }
                    if !tool_names.is_empty() {
                        last_action = format!("正在调用工具: {}", tool_names.join(", "));
                        turn_ended = false;
                    } else if has_text {
                        last_action = "助手已回复".into();
                        turn_ended = true;
                    }
                }
            }
            _ => {}
        }
    }

    // 只把真实用户排队输入（展示文本 Some）拿去上报；通知类占位项丢弃
    let queued_inputs: Vec<String> = queue.into_iter().filter_map(|(_, d)| d).collect();
    // 刚清空、未输入的空会话：见过 /clear 且没有真实 prompt
    let cleared = saw_clear && prompt.is_empty();

    Some(SessionSummary {
        provider: "claude".into(),
        // 解析器只认文件内容，认不出这份 jsonl 躺在哪个根下 —— 由 scan 的调用方按扫描根
        // 覆写（桌面客户端本地代理的会话文件格式与 CLI 完全一样，见 scan_projects_root）。
        desktop: false,
        session_id: session_id.to_string(),
        project_key,
        cwd: if canonical_cwd.is_empty() {
            cwd
        } else {
            canonical_cwd
        },
        // 此处先原样放最后那个 cwd；收到仓库根是在 summarize 里做的（那儿才有 prev 兜底，
        // 且只在会话文件真的变了时才走一次，不会每轮都去 stat 一遍父链）。
        live_cwd: live_cwd.clone(),
        shell_cwd: live_cwd,
        title: String::new(),
        prompt,
        last_action,
        // cleared 会话没有进行中的回合 → 视为回合结束（显示 Idle 而非 Running）
        turn_ended: turn_ended || cleared,
        cleared,
        // 「生于 /clear」是永久事实，与「此刻还空着」分开记（见字段说明）
        clear_born: saw_clear,
        has_content,
        // 由 scan 在产出处现数（解析器只看文件内容，不知道这份 jsonl 躺在哪个根下）
        sub_agent_count: 0,
        started_at,
        last_active_at,
        version,
        git_branch,
        mtime_ms: 0,
        created_ms: 0,
        line_count: 0,
        used_tokens_5h,
        queued_inputs,
        select_answered_ms,
    })
}

/// 这条 user 记录里是否含**指定** tool_use 的结果（即那次工具调用已了结）
fn content_answers(content: Option<&Value>, tool_use_id: &str) -> bool {
    let Some(Value::Array(items)) = content else {
        return false;
    };
    items.iter().any(|it| {
        it.get("type").and_then(Value::as_str) == Some("tool_result")
            && it.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id)
    })
}

/// 从 claude 进程命令行里取被 `--resume <id>` / `--resume=<id>` / `-r <id>` 指定的
/// 会话号。恢复的会话 started_at 很旧，靠时间配不上，只能从命令行认出来。
/// `--continue`（无显式 id）返回 None，交给 started_at/mtime 兜底。
fn resume_session_id(command: &str) -> Option<&str> {
    let looks_id =
        |s: &str| s.len() >= 8 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let toks: Vec<&str> = command.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        if let Some(rest) = t.strip_prefix("--resume=") {
            if looks_id(rest) {
                return Some(rest);
            }
        }
        if (*t == "--resume" || *t == "-r") && i + 1 < toks.len() && looks_id(toks[i + 1]) {
            return Some(toks[i + 1]);
        }
    }
    None
}

/// 命令行里是否带 `--continue` / `-c`（恢复最近一个会话，无显式会话号）。
fn wants_continue(command: &str) -> bool {
    command
        .split_whitespace()
        .any(|t| t == "--continue" || t == "-c")
}

/// ISO8601 → epoch 毫秒
fn iso_to_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

/// 标题开头那串路径只留文件名：`./tmp/图片.jpg 根据图片改` → `图片.jpg 根据图片改`。
///
/// 从网页/钉钉发任务时习惯「先甩路径、再说需求」，而列表里的标题只显示头 20 来字，额度
/// 全被 `./tmp/…` 这样的目录前缀吃掉 —— 几条并排全是同一个开头，看不出谁是谁，甚至一个
/// 需求字都露不出来。
///
/// 只去目录、留文件名，不整段丢弃：「说的是哪个文件」本身也是信息，剥光了标题反而更难认。
/// 也只处理**开头连续**的路径 token；正文中间提到的路径原样保留，那儿多半是有意引用。
fn shorten_leading_paths(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s.trim_start();
    while let Some(tok) = rest.split_whitespace().next() {
        // 判据从严：必须带路径分隔符。裸文件名（a.md）本来就短，不动它
        if !tok.contains('/') && !tok.contains('\\') {
            break;
        }
        let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
        // 以分隔符结尾（`./tmp/`）时 basename 为空：留着原样，免得整段消失
        if base.is_empty() {
            break;
        }
        out.push_str(base);
        out.push(' ');
        rest = rest[tok.len()..].trim_start();
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 从 user 条目 content 中提取真实提示词（过滤命令包装与 tool_result）
fn user_text(content: Option<&Value>) -> Option<String> {
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut buf = String::new();
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(t);
                    }
                }
            }
            buf
        }
        _ => return None,
    };
    let trimmed = text.trim();
    // 这些都是 Claude Code 注入的系统内容，只是恰好被记成 type=user。
    // 不滤掉的话会在对话流里冒充「用户发的话」——后台任务跑完的回执
    // <task-notification> 尤其常见，用户会看到自己「发」了一段 XML。
    if trimmed.is_empty()
        || trimmed.starts_with("<local-command")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("<task-notification>")
        || trimmed.starts_with("Caveat:")
        || trimmed.starts_with("[Request interrupted")
        // 上下文压缩后注入的续接摘要（compact）：整段几百行的 "Summary: 1. Primary
        // Request and Intent..."，也是记成 type=user。不滤掉的话，对话流里会突然
        // 冒出一大段英文，看着像自己发的。
        || trimmed.starts_with("This session is being continued from a previous conversation")
    {
        return None;
    }
    Some(truncate(trimmed, FLOW_TEXT_MAX))
}

/// 这条 user 记录是不是「用户在终端按 Esc 中断」的标记。
///
/// Claude Code 把中断记成 `type=user`、正文 `[Request interrupted by user...]`
/// （另有 `...for tool use` 变体）。它不是用户输入，而是「这一轮到此为止」的信号。
fn is_interrupt_marker(content: Option<&Value>) -> bool {
    let text = match content {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => return false,
    };
    text.starts_with("[Request interrupted")
}

/// queue-operation enqueue 的 content：用户排队输入（过滤系统通知包装）
fn queued_user_text(v: &Value) -> Option<String> {
    let text = v.get("content").and_then(Value::as_str)?.trim().to_string();
    if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
        return None;
    }
    // 与对话流里的 user 正文同上限：否则同一条下发在「排队条」被截到 500、进流后是全文，
    // 两边按内容去重就对不上。
    Some(truncate(&text, FLOW_TEXT_MAX))
}

fn content_has_tool_result(content: Option<&Value>) -> bool {
    matches!(content, Some(Value::Array(items)) if items
        .iter()
        .any(|i| i.get("type").and_then(Value::as_str) == Some("tool_result")))
}

/// 把一条 jsonl 记录转成对话消息（不可展示的返回 None）
/// Codex response_item 里的真实用户输入（滤掉 <permissions>/<recommended_plugins> 等注入块）
fn codex_user_text(v: &Value) -> Option<String> {
    let p = v.get("payload")?;
    if p.get("type").and_then(Value::as_str) != Some("message")
        || p.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    let mut buf = String::new();
    for item in p.get("content")?.as_array()? {
        if item.get("type").and_then(Value::as_str) == Some("input_text") {
            if let Some(t) = item.get("text").and_then(Value::as_str) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(t);
            }
        }
    }
    let t = buf.trim();
    if t.is_empty() || t.starts_with('<') {
        return None;
    }
    Some(truncate(t, FLOW_TEXT_MAX))
}

/// 把一行 Codex 会话记录转为简要消息（与 Claude 的 entry_to_brief 对应）。
/// 覆盖：用户/助手消息、function_call / custom_tool_call（工具行）及其输出（结果行）。
fn codex_entry_to_brief(v: &Value) -> Option<MessageBrief> {
    if v.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let p = v.get("payload")?;
    match p.get("type").and_then(Value::as_str)? {
        "message" => match p.get("role").and_then(Value::as_str)? {
            "user" => codex_user_text(v).map(|t| MessageBrief {
                role: "user".into(),
                content: t,
                timestamp: ts,
                is_error: false,
                tools: Vec::new(),
                tool_use_id: String::new(),
            }),
            "assistant" => {
                let mut buf = String::new();
                for item in p.get("content")?.as_array()? {
                    if item.get("type").and_then(Value::as_str) == Some("output_text") {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            buf.push_str(t);
                        }
                    }
                }
                let t = buf.trim();
                (!t.is_empty()).then(|| MessageBrief {
                    role: "assistant".into(),
                    content: truncate(t, FLOW_TEXT_MAX),
                    timestamp: ts,
                    is_error: false,
                    tools: Vec::new(),
                    tool_use_id: String::new(),
                })
            }
            _ => None, // developer 等注入角色不进对话流
        },
        "function_call" | "custom_tool_call" => {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("?");
            // arguments 是 JSON 字符串，截一段作为提示即可
            let args = p
                .get("arguments")
                .or_else(|| p.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(MessageBrief {
                role: "tool".into(),
                content: String::new(),
                timestamp: ts,
                is_error: false,
                // Codex 一条记录就是一次调用，但形状要与 Claude 那边一致 ——
                // 前端只认一套结构，不为来源分叉。它的 id 叫 call_id。
                tools: vec![crate::model::ToolCall {
                    id: p
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: name.to_string(),
                    hint: truncate(args, 120),
                }],
                tool_use_id: String::new(),
            })
        }
        "function_call_output" | "custom_tool_call_output" => {
            let out = p.get("output").and_then(Value::as_str).unwrap_or("");
            (!out.trim().is_empty()).then(|| MessageBrief {
                role: "tool_result".into(),
                content: truncate(out.trim(), 400),
                timestamp: ts,
                is_error: false,
                tools: Vec::new(),
                tool_use_id: p
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        _ => None,
    }
}

/// 对话流里「一段话」的存储上限（用户下发内容 / 助手回复 / 方案正文）。定得足够大，
/// 让整段内容都进对话流——前端再按需折叠（展开全部/收起）。此前 500/2000 太小，长下发
/// 与长结果在流里被 `…` 砍掉，连「展开」也放不出来。仍留个上限挡住病态超长（如误粘整个
/// 文件），避免每次轮询都把 MB 级文本反复搬运。
const FLOW_TEXT_MAX: usize = 16_000;

/// 解析父会话记录里的一条 —— 子会话（sidechain）记录一律跳过。
///
/// 父会话 jsonl 里混着子代理自己的那些记录（`isSidechain: true`），它们不属于
/// 「我和主会话的对话」，摊进对话流会让一次派活变成几十条噪音。
fn entry_to_brief(v: &Value) -> Option<MessageBrief> {
    parse_entry(v, true)
}

/// 解析一条会话记录。
///
/// `skip_sidechain` 决定要不要跳过子会话记录：读父会话时要跳（见 [`entry_to_brief`]），
/// **读子会话记录本身时绝不能跳** —— `subagents/agent-*.jsonl` 里每一条都是
/// `isSidechain: true`，跳完就一条不剩。实测本机
/// `agent-a9e86999bc2536847.jsonl` 172 行，按父会话那套口径解析出 0 条。
fn parse_entry(v: &Value, skip_sidechain: bool) -> Option<MessageBrief> {
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ty = v.get("type").and_then(Value::as_str)?;
    if skip_sidechain
        && v.get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return None;
    }
    match ty {
        "user" => {
            if v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
                return None;
            }
            let content = v.pointer("/message/content");
            if let Some(text) = user_text(content) {
                return Some(MessageBrief {
                    role: "user".into(),
                    content: text,
                    timestamp: ts,
                    is_error: false,
                    tools: Vec::new(),
                    tool_use_id: String::new(),
                });
            }
            // tool_result：展示简要执行结果
            if let Some(Value::Array(items)) = content {
                for item in items {
                    if item.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let text = tool_result_text(item);
                        return Some(MessageBrief {
                            role: "tool_result".into(),
                            content: truncate(&text, 400),
                            timestamp: ts,
                            tools: Vec::new(),
                            // 它回应的是哪一次调用 —— 记录里本来就有，前端据此把结果
                            // 贴回执行链上对应那一步，不必按先后顺序猜。
                            tool_use_id: item
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            // 这一步是不是跑砸了 —— 记录里本来就有，别再丢一次。
                            // 缺这个键（老记录 / 别的形态）按「没出错」算，与改前一致。
                            is_error: item
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        });
                    }
                }
            }
            None
        }
        // 排队项不进对话流：它们由 Task.queued_inputs 单独上报、前端挂在对话框上方。
        // 若还在这里产出 user 简报，重度排队的会话（如交互式选择时堆了很多待处理输入）
        // 会用 enqueue 简报把最近 N 条窗口挤满，执行中只渲染 assistant/plan 时便显示为
        // 「空会话」；且被接受后 claude 另写真实 user 记录，会重复。故一律不产出。
        "queue-operation" => None,
        "assistant" => {
            let items = v.pointer("/message/content")?.as_array()?;
            let mut text_buf = String::new();
            let mut tools: Vec<crate::model::ToolCall> = Vec::new();
            let mut plan: Option<&str> = None;
            // 交互式选择/权限确认（AskUserQuestion）：把问题与选项整份同步给前端渲染成卡片
            let mut select_input: Option<&Value> = None;
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            text_buf.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
                        // plan 模式给出的待批准方案：正文在 input.plan，
                        // 走 tool_input_hint 的话它不认 plan 字段，整份方案会被丢掉，
                        // 只剩一行光秃秃的 "ExitPlanMode"。
                        if name == "ExitPlanMode" {
                            plan = item
                                .get("input")
                                .and_then(|i| i.get("plan"))
                                .and_then(Value::as_str);
                            continue;
                        }
                        if name == "AskUserQuestion" {
                            select_input = item.get("input");
                            continue;
                        }
                        // 一次调用一个元素、各带自己的 tool_use_id。此前是
                        // `format!("{name}: {hint}")` 推进 Vec<String> 再 `" | "` 拼成
                        // 一个字符串 —— 拼完就再也认不出哪一段是哪一次调用了。
                        tools.push(crate::model::ToolCall {
                            id: item
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            name: name.to_string(),
                            hint: tool_input_hint(item.get("input")),
                        });
                    }
                    _ => {}
                }
            }
            // 方案是本条记录里最要紧的内容，优先于同条的工具流水
            if let Some(p) = plan.filter(|p| !p.trim().is_empty()) {
                return Some(MessageBrief {
                    role: "plan".into(),
                    content: truncate(p.trim(), FLOW_TEXT_MAX),
                    timestamp: ts,
                    is_error: false,
                    tools: Vec::new(),
                    tool_use_id: String::new(),
                });
            }
            // 交互式选择卡片：整份 input（questions/options）序列化给前端
            if let Some(inp) = select_input {
                return Some(MessageBrief {
                    role: "select".into(),
                    content: truncate(&inp.to_string(), 4000),
                    timestamp: ts,
                    is_error: false,
                    tools: Vec::new(),
                    tool_use_id: String::new(),
                });
            }
            if !text_buf.trim().is_empty() {
                Some(MessageBrief {
                    role: "assistant".into(),
                    content: truncate(text_buf.trim(), FLOW_TEXT_MAX),
                    timestamp: ts,
                    is_error: false,
                    tools: Vec::new(),
                    tool_use_id: String::new(),
                })
            } else if !tools.is_empty() {
                Some(MessageBrief {
                    role: "tool".into(),
                    // 正文在 tools 里，不再另拼一份字符串：两份并存必然漂移，
                    // 纯文本出口统一走 MessageBrief::text()。
                    content: String::new(),
                    timestamp: ts,
                    is_error: false,
                    tools,
                    tool_use_id: String::new(),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn tool_result_text(item: &Value) -> String {
    match item.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 提取工具调用的关键入参做展示（命令 / 文件路径 / 描述）
fn tool_input_hint(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "description",
        "prompt",
        "url",
    ] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            return truncate(s, 120);
        }
    }
    String::new()
}

/// 待办清单里的一项
#[derive(Debug, Clone, Serialize)]
struct TodoItem {
    id: String,
    subject: String,
    status: String,
}

/// 后台运行的任务（后台命令 / 异步子代理）。
///
/// 结构与对外形状统一用 [`SubTask`]（`am_core::model`）——此前这里另有一份只在
/// scanner 内部可见的 `BgTask`，序列化成一条 `role:"bgtasks"` 的伪消息塞进消息流；
/// 现在它是 `Task::sub_tasks`，两处不再各写一份。
///
/// 字段语义见 [`SubTask`] 本身；下面这个构造器只是省掉每处都写全 `outcome` / `has_body`
/// 这两个「产出时才定得下来」的字段。
#[allow(clippy::too_many_arguments)]
fn new_sub_task(
    id: String,
    kind: &str,
    label: String,
    status: String,
    started_at: String,
    summary: Option<String>,
    ended_ms: u64,
    tool_use_id: String,
) -> SubTask {
    SubTask {
        id,
        kind: kind.to_string(),
        label,
        status,
        // 占位：真正的归类在 [`BgTracker::reconciled`] 出快照那一刻按磁盘事实定，
        // `items` 里这份只是重放中间态，不会外发。
        outcome: SubTaskOutcome::Running,
        started_at,
        ended_ms,
        summary,
        has_body: false,
        tool_use_id,
        runs: 1,
    }
}

/// 追踪会话里「在后台跑着」的任务。
///
/// 同样是跨记录的状态：
/// - 启动：tool_use 只给得出展示名，任务号与种类都要等它的 tool_result 才成形；
/// - 结束：两条互斥的信号，缺一条就有一整类任务永远停在「执行中」——
///   - 跑到自己结束 → 某条记录里的 `<task-notification>` 带 `<task-id>` 与 `<status>`
///     （**不保证排在起跑记录之后**，见 [`Self::early`]）；
///   - 被主动停掉 → `TaskStop` 那次调用的结果，见 [`stopped_task_id`]，此后没有通知。
///
/// **种类判定只认 `toolUseResult` 里的结构化字段**（`agentId` / `backgroundTaskId`），
/// 不看工具名、也不看入参：工具名是会变的（`Task` 早已改叫 `Agent`），
/// `run_in_background` 这个入参更是压根不出现在派子代理的调用里 ——
/// 按名字或入参判会随上游改名而整条哑掉。
#[derive(Default)]
struct BgTracker {
    /// tool_use_id -> (展示名, 起跑时刻)（等 tool_result 定种类、回填任务号）
    pending: HashMap<String, (String, String)>,
    /// 早到的完成通知：tool_use_id -> (任务号, 终态, 通知时刻)。
    ///
    /// **记录文件不是按时间戳排的**：完成通知是一条 `queue-operation`，落盘时机与
    /// 那次工具调用的 `tool_result` 记录彼此独立，后者常常晚好几行才补上。
    /// 实测本机 4 例（`brbmfiatf` 通知在 :3971、起跑记录在 :3975；`b02t05203`
    /// 通知在 :5414、起跑记录在 :5573，两者时间戳还差 4.5 分钟），通知先到时
    /// [`Self::on_notification`] 找不到条目就把它丢了，条目随后建出来永远停在「执行中」。
    /// 故先按 `<tool-use-id>` 存着，等配对的 `tool_result` 到达时补上。
    early: HashMap<String, EarlyDone>,
    items: Vec<SubTask>,
    /// **磁盘已经判定过的终态**：子会话号 → (终态, 收尾时刻)。
    ///
    /// 父记录没写下收尾通知时，终态只能由 [`Self::reconciled`] 从磁盘推断
    /// （尾形态已收尾 + 静置够久）。此前这个推断**每轮现算、绝不记住**，而判据里
    /// 有个 `now_ms - 文件最后写入`——于是子会话文件只要再被写一下，静置时间就归零，
    /// 同一条记录当场从 `completed` 翻回 `running`，`ended_ms` 也跟着换一个新值。
    /// 确定性复现：静置 10 分钟 → completed(endedMs=T1)；touch 一下 → running(endedMs=0)；
    /// 再静置 10 分钟 → completed(endedMs=T2)。全程 `tool_use_id` 没变，也就是说
    /// **根本没有新的一次派活**——状态在骗人。
    ///
    /// 所以推断一旦落定就记在这里，**终态是吸收态**。要重新打开它只有一条路：
    /// 父记录里出现新的一次派活（新的 `tool_use_id`，见 [`Self::on_tool_result`]），
    /// 或父记录自己发话（收尾通知，见 [`Self::on_notification`]）——那两处都会把这里清掉。
    /// 「文件又被写了一下」不是证据。
    settled: HashMap<String, SettledEnd>,
    /// 已经数过的收尾通知：(子会话号, 通知正文的指纹)。
    ///
    /// **同一条收尾通知会落两次盘**：一条 `queue-operation`（通知挂在顶层 content 上）、
    /// 一条 `user`（挂在 message.content 上），正文逐字节相同，时间戳只差 10~19 毫秒
    /// （实测 a7f78026084ce8753 的 4 次运行全是这个形态：行 369/377、458/460、
    /// 548/550、567/569）。两条都要解析（只认其一会漏收尾，见 [`Self::observe`]），
    /// 但 [`SubTask::runs`] 只能数一次 —— 按时间戳去重会因为那十几毫秒失效，
    /// 所以按**正文指纹**去重：同一份正文就是同一条通知。
    ///
    /// 极端情况下两轮运行的通知正文可能逐字节相同（那样会少数一轮）；正文里嵌着这一轮
    /// 的完整报告（实测 2172~6545 字节），撞上的概率远低于「每轮都数成两次」的代价。
    seen_notif: HashSet<(String, u64)>,
}

/// 磁盘推断出来、已经落定的终态。
struct SettledEnd {
    status: String,
    ended_ms: u64,
}

/// 早到的完成通知：等配对的 `tool_result` 到达时回填给条目。
struct EarlyDone {
    task_id: String,
    status: String,
    summary: Option<String>,
    ended_ms: u64,
}

impl BgTracker {
    fn observe(&mut self, v: &Value) {
        // 完成通知不止一种落法：子代理/后台命令跑完时是一条 queue-operation，
        // 通知文本直接挂在顶层 content（字符串）上，不在 /message/content 里。
        // 只看 /message/content 的话，任务只进不出，永远停在「运行中」。
        let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("");
        if let Some(s) = v.get("content").and_then(Value::as_str) {
            self.on_notification(s, ts);
        }
        // 工具结果的结构化元数据是记录的顶层字段，与 message 平级 ——
        // 种类判定要用它，所以得从这一层带下去。
        let meta = v.get("toolUseResult");
        // 挂在消息体上的：内容可能是纯字符串，也可能是分块数组
        if let Some(c) = v.pointer("/message/content") {
            match c {
                Value::String(s) => self.on_notification(s, ts),
                Value::Array(items) => {
                    for item in items {
                        match item.get("type").and_then(Value::as_str) {
                            Some("tool_use") => self.on_tool_use(item, ts),
                            Some("tool_result") => self.on_tool_result(item, meta, ts),
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(Value::as_str) {
                                    self.on_notification(t, ts);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 每个 tool_use 都先记下展示名与起跑时刻 —— 此刻还判不出它是不是后台任务
    /// （入参里没有任何可靠标记），真正的甄别在 tool_result 那步做。
    /// 条目在配对的 tool_result 到达时立刻移除，所以这张表始终只存「在途」的那几条。
    fn on_tool_use(&mut self, item: &Value, ts: &str) {
        let Some(use_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let name = item.get("name").and_then(Value::as_str).unwrap_or("任务");
        // description 最贴近人看的说明，没有再退回工具名
        let label = item
            .get("input")
            .and_then(|i| i.get("description"))
            .and_then(Value::as_str)
            .map(|s| truncate(s, 80))
            .unwrap_or_else(|| name.to_string());
        self.pending
            .insert(use_id.to_string(), (label, ts.to_string()));
    }

    fn on_tool_result(&mut self, item: &Value, meta: Option<&Value>, ts: &str) {
        let Some(use_id) = item.get("tool_use_id").and_then(Value::as_str) else {
            return;
        };
        let started = self.pending.remove(use_id);
        // 主动停止（TaskStop）不发完成通知，收尾信号只有这条结果本身
        if let Some(id) = stopped_task_id(meta) {
            // 父记录亲口说的，盖过磁盘推断
            self.settled.remove(&id);
            if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
                t.status = "stopped".into();
                // TaskStop 的结果里只有 "Successfully stopped task: <id> (<命令>)"，
                // 复述了 id 与命令、说不出别的（实测 20 条形态一致），当不了原因；
                // 上一轮跑的那句更是过期了，一并清掉。
                t.summary = None;
                t.ended_ms = iso_to_ms(ts).unwrap_or(0);
            }
            return;
        }
        // 只有 toolUseResult 交出任务号的，才是「还在后台跑着」的任务：
        //   agentId          → 异步子代理
        //   backgroundTaskId → 后台命令
        // 其余（绝大多数）工具调用当场就结束了，不进清单。
        let Some((id, kind)) = bg_identity(meta) else {
            return;
        };
        // 这次起跑的完成通知先落盘了？（见 [`Self::early`]）任务号对得上才认，
        // 免得把上一轮同号任务的终态套到复活的这条上。
        let done = self
            .early
            .remove(use_id)
            .filter(|d| d.task_id == id)
            .map(|d| (d.status, d.summary, d.ended_ms));
        let (label, started_at) = match started {
            Some((l, t)) => (l, t),
            // tool_use 落在重放窗口之外（极少见）：退回结果里的说明，时间用当前这条
            None => (
                meta.and_then(|m| m.get("description"))
                    .and_then(Value::as_str)
                    .map(|s| truncate(s, 80))
                    .unwrap_or_else(|| "后台任务".to_string()),
                ts.to_string(),
            ),
        };
        let (status, summary, ended_ms) = done.unwrap_or_else(|| ("running".to_string(), None, 0));
        // 同一个子代理被唤醒续跑时会再来一条结果：原地复活，别堆重复条目
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            // **这是新的一次派活**：同一个子代理被重新叫起来干活（实测本机
            // a7f78026084ce8753 在父记录里有 8 条时刻各不相同的收尾通知）。
            // 本条记录只描述最近一次运行，所以 status/ended_ms/tool_use_id 全换 ——
            // 但 runs 要累加，前端才分得清「它又跑起来了」和「刚才那次判错了」。
            t.runs = t.runs.saturating_add(1);
            t.label = label;
            t.status = status;
            t.summary = summary;
            t.started_at = started_at;
            t.ended_ms = ended_ms;
            // 续跑是新的一次调用，配对键跟着换 —— 留着上一轮的 id 会让执行链把卡片
            // 挂回上一次那一步
            t.tool_use_id = use_id.to_string();
            // 唯一能把磁盘定案的终态重新打开的事实：真的又派了一次活
            self.settled.remove(&id);
            return;
        }
        self.settled.remove(&id);
        self.items.push(new_sub_task(
            id,
            kind,
            label,
            status,
            started_at,
            summary,
            ended_ms,
            // 这条结果的 tool_use_id 就是起跑那次调用的 id，执行链靠它精确配对
            use_id.to_string(),
        ));
    }

    /// 解析 <task-notification>：一条通知可能带多个 task-id，共用一个 status
    fn on_notification(&mut self, text: &str, ts: &str) {
        if !text.contains("<task-notification>") {
            return;
        }
        let Some(status) = tag_value(text, "status") else {
            return;
        };
        // "__orphan_summary__:*" 是会话续跑/压缩恢复时的孤儿汇总标记：语义是「此前所有
        // 后台 shell/子代理都已不在」。它往往只枚举部分 id，漏网的若只按枚举清，会永远
        // 卡在「运行中」（后台起的 dev server / 测试 hub 尤其常见）。
        // 故一旦出现该标记，就把当时所有仍在跑的后台任务统一落到该终态，
        // 只留下最近一次恢复之后新起、当前真在跑的后台任务 —— 即「实时」语义。
        //
        // **只在 `<task-id>` 的值里认这个标记，不扫全文**：通知正文里嵌着刚跑完那个
        // 子代理的完整报告，报告里出现这几个字（比如它写的正是这段源码的分析）就会
        // 被当成孤儿汇总，把同批还在跑的兄弟任务一起误判成已结束。
        // 实测：c7d3a592-…jsonl:88 的通知只报了 af93b9ce77aec80b3，正文里引用到本段
        // 源码，结果把并行跑着的 a33f6420ca1247a8d 一并抹成 completed。
        let ended_ms = iso_to_ms(ts).unwrap_or(0);
        // 孤儿汇总要先认出来：它那句 `<summary>` 讲的是「上一轮整批没留下收尾记录」，
        // 对同一条通知里点名的任务也一样不成立，所以得在分派原因之前就判掉。
        let mut is_orphan_summary = false;
        {
            let mut scan = text;
            while let Some(id) = tag_value(scan, "task-id") {
                if id.starts_with("__orphan_summary__") {
                    is_orphan_summary = true;
                    break;
                }
                let Some(pos) = scan.find("</task-id>") else {
                    break;
                };
                scan = &scan[pos + "</task-id>".len()..];
            }
        }
        // 收尾原因。上界取 300：实测非 completed 的 138 条 summary，p90 = 243、
        // 最长 926（限流那句会带上 request id 与模型名），300 能把 p90 完整收下，
        // 又不至于让一条卡片的文案顶到千字。
        let summary = (!is_orphan_summary)
            .then(|| tag_value(text, "summary").map(|s| truncate(&s, 300)))
            .flatten();
        // 通知里的 `<tool-use-id>` 就是起跑那次调用的 tool_use_id —— 起跑记录还没轮到时
        // 靠它把终态存下来（见 [`Self::early`]）。孤儿汇总没有这个标签，也不需要。
        let use_id = tag_value(text, "tool-use-id");
        let mut rest = text;
        while let Some(id) = tag_value(rest, "task-id") {
            // "__orphan_summary__:*" 是内部扫描标记，不是真任务
            if !id.starts_with("__") {
                // 同一条通知落两次盘，按正文指纹去重，别把一轮数成两轮
                let fresh = self.seen_notif.insert((id.clone(), text_fingerprint(text)));
                if let Some(t) = self.items.iter_mut().find(|t| t.id == id).filter(|_| fresh) {
                    // 同一条通知的第二次落盘直接跳过（上面 `fresh` 判的）：内容逐字节相同，
                    // 应用一遍只会把 ended_ms 挪十几毫秒 —— 白白让下游看到一次值变化。
                    //
                    // **同一个子代理被反复叫起来干活**：每跑完一次就来一条收尾通知。
                    // 实测本机 aad0fb121cab8a31d 有 8 个时刻各不相同的通知 = 跑了 8 次
                    // （a3ac5a59… 3 次、a7f78026… 4 次）。而**重新派活不写新的
                    // tool_result**（实测该 agentId 的 toolUseResult 记录全文只有 1 条），
                    // 所以「第几次运行」只能在这里数：又收到一条更晚的收尾通知，
                    // 就说明刚才那是新的一轮。同一时刻的重复通知（queue-operation 与
                    // user 各落一条）时间戳相同，不会重复计数。
                    if t.status != "running" && ended_ms > t.ended_ms && t.ended_ms > 0 {
                        t.runs = t.runs.saturating_add(1);
                    }
                    t.status = status.clone();
                    t.summary = summary.clone();
                    t.ended_ms = ended_ms;
                    // 父记录亲口说的是最权威的，盖过磁盘那份推断
                    self.settled.remove(&id);
                } else if let Some(u) = &use_id {
                    self.early.insert(
                        u.clone(),
                        EarlyDone {
                            task_id: id.clone(),
                            status: status.clone(),
                            summary: summary.clone(),
                            ended_ms,
                        },
                    );
                }
            }
            let Some(pos) = rest.find("</task-id>") else {
                break;
            };
            rest = &rest[pos + "</task-id>".len()..];
        }
        if is_orphan_summary {
            for t in self.items.iter_mut() {
                if t.status == "running" {
                    t.status = status.clone();
                    t.summary = None;
                    t.ended_ms = ended_ms;
                }
            }
        }
    }

    /// 把父会话记录重放出的清单与磁盘上的子会话记录对齐，**并淘汰过期条目**，
    /// 得到这一刻真正该展示的那份清单。
    ///
    /// # 为什么要淘汰（`永不消失` 那个 bug 的根）
    ///
    /// `items` 是从会话开头全量重放累积出来的，此前**只有状态变更、没有任何淘汰**：
    /// 一条子代理只要出现过就永远在快照里。实测本机
    /// `3603f576-…jsonl` 一条会话累到 **147 条**、最老的是 11 天前的事；
    /// `3df6b1a7-…jsonl` 累到 **122 条**，里头两条 6 天前 failed 的至今还在。
    /// 界面上的表现就是「后台任务」面板永远挂着一堆上周的东西。
    ///
    /// 按 [`SubTask::ended_ms`] 设保留窗口 [`BG_RETAIN_MS`]，再加一道条数上限
    /// [`BG_MAX_ITEMS`]（防一天之内爆量）。**只淘汰终态条目** —— 还在跑的必须留着，
    /// 不管它跑了多久（长命的 dev server 就是这样）。
    ///
    /// # 为什么终态条目也要回看磁盘
    ///
    /// 父记录里的 `<task-notification>` 并不保证写得下来（父进程被打断/退出、机器重启
    /// 时就没了），也不保证说得对。旧实现有一条 early-continue：`status != "running"`
    /// 就直接跳过、不再读磁盘 —— 于是一旦落了终态就再也纠不回来，哪怕子会话自己那份
    /// 记录明明白白收在「已交回结果」的形态上。这条 early-continue 是**要推翻的旧设计**，
    /// 现在终态条目也过一遍磁盘。
    ///
    /// 但方向是**单向的**：磁盘只能把条目往「跑完了」纠，不能反过来把父记录说死的条目
    /// 改判成别的死法 —— [`SUBAGENT_ABANDON_MS`] 那条「停在半路太久 ⇒ 被 kill 了」的
    /// 兜底只对父记录还说在跑的条目生效。否则本机那两条 `failed: Agent stalled`
    /// （`aa8f4211d424433a4` / `a08b04e8a92c83c7c`，父记录给了确切原因）会被这道兜底
    /// 抹成 `stopped`，连 summary 里的原因一起丢掉。
    ///
    /// # 终态是吸收态（`outcome 来回抖` 那个 bug 的根）
    ///
    /// 上面那条「静置够久 ⇒ 跑完了」的推断里有个 `now_ms - 文件最后写入`。此前推断
    /// **每轮现算、绝不记住**（原注释写着「误判可自愈…状态自己翻回 running，
    /// 误判最多让胶囊闪一下」）—— 于是子会话文件只要再被写一下，静置时间就归零，
    /// 同一条记录当场从 `completed` 翻回 `running`，`ended_ms` 还换一个新值。
    /// 确定性复现：静置 10 分钟 → `completed(endedMs=T1)`；touch 一下 →
    /// `running(endedMs=0)`；再静置 10 分钟 → `completed(endedMs=T2)`，
    /// 全程 `tool_use_id` 没变 = **根本没有新的一次派活**。那不是「自愈」，是状态在骗人。
    ///
    /// 所以推断一旦落定就记进 [`Self::settled`]，此后只认两种翻案事实，都在父记录里：
    /// 真的又派了一次活（[`Self::on_tool_result`]，`runs` 跟着 +1），
    /// 或父记录自己发话（[`Self::on_notification`] / `TaskStop`）。
    /// 「文件又被写了一下」不算证据 —— 它既可能是续跑，也可能只是上一次的收尾还在刷盘，
    /// 二者无从区分，而把两种都当成「又跑起来了」就是现在这个抖动。
    ///
    /// 这**不是**退回旧的 early-continue（`status != "running"` 就不读磁盘）：
    /// 落定之前每一轮照样读磁盘、照样能把父记录说错的 `killed` 纠成 `completed`。
    /// 区别只在于「纠完之后记不记得住」。
    ///
    /// 对齐只针对子代理（`kind == "agent"`），后台命令没有这份记录 —— 也不需要：
    /// 它的两条收尾信号（完成通知、`TaskStop` 结果）都在父记录里。
    ///
    /// `tail_of` 只在真需要时才调用（它要读文件尾）。
    fn reconciled(
        &mut self,
        last_write: &HashMap<String, u64>,
        now_ms: u64,
        tail_of: &dyn Fn(&str) -> Option<SubAgentTail>,
        opts: ReconcileOpts,
    ) -> Vec<SubTask> {
        let mut items = self.items.clone();
        // 本轮新落定的磁盘终态（循环里不能同时改 self.settled，攒着出来再写）
        let mut newly_settled: Vec<(String, SettledEnd)> = Vec::new();
        // 父记录漏掉的子会话（阻塞式派活不写 agentId，见 BgTracker 的说明）：
        // 只有全量视角才补进来，热路径那份维持原样、一个字节不多传。
        for extra in opts.extra {
            if !items.iter().any(|t| t.id == extra.id) {
                items.push(extra);
            }
        }
        for t in items.iter_mut().filter(|t| t.kind == "agent") {
            t.has_body = last_write.contains_key(&t.id);
            // 磁盘早就判过它收尾了 → 认定案，不再看文件此刻写没写。
            // 能把它重新打开的只有父记录里的新一次派活 / 新通知，那两处会清掉这份定案。
            if let Some(done) = self.settled.get(&t.id) {
                t.status = done.status.clone();
                t.summary = None;
                t.ended_ms = done.ended_ms;
                continue;
            }
            let Some(&wrote_ms) = last_write.get(&t.id) else {
                continue;
            };
            let terminal = t.status != "running";
            // **通知之后很久还在写 ⇒ 它被重新派活了，正在跑新的一轮。**
            //
            // 这是唯一能观测到「新一轮开始」的信号：重新派活不写新的 tool_result
            // （实测 aad0fb121cab8a31d 的 toolUseResult 全文只有 1 条，收尾通知却有 8 条）。
            // 只对**父记录给出的**终态成立 —— 那种 `ended_ms` 是通知里的真实时刻，
            // 「比它晚 5 分钟还在写」确实只能是新一轮。
            // 磁盘自己推断出来的终态没有这种可信时刻（见 `settled`，上面已 continue），
            // 对它来说「文件又被写了一下」跟「刚才那一猜太早了」根本分不开。
            let resumed = terminal && wrote_ms > t.ended_ms.saturating_add(SUBAGENT_SETTLE_MS);
            let idle_ms = now_ms.saturating_sub(wrote_ms);
            match tail_of(&t.id) {
                // 结果已经交回去了，静置够久就是真跑完了 —— 父记录那句话说错了（或压根没写）
                Some(SubAgentTail::Finished) if idle_ms > SUBAGENT_SETTLE_MS => {
                    t.status = "completed".into();
                    t.summary = None;
                    // 通知缺席时 ended_ms 是 0，拿记录最后写入时刻补上 ——
                    // 没有它，这条就永远过不了下面的保留窗口。
                    if t.ended_ms == 0 {
                        t.ended_ms = wrote_ms;
                    }
                    newly_settled.push((
                        t.id.clone(),
                        SettledEnd {
                            status: t.status.clone(),
                            ended_ms: t.ended_ms,
                        },
                    ));
                }
                // 停在半路太久：被 kill 在半路了。只兜底父记录还说在跑的条目，
                // 已有确切死因的终态条目不碰（见上面的方向说明）。
                Some(SubAgentTail::Midflight) if !terminal && idle_ms > SUBAGENT_ABANDON_MS => {
                    t.status = "stopped".into();
                    t.summary = None;
                    t.ended_ms = wrote_ms;
                    newly_settled.push((
                        t.id.clone(),
                        SettledEnd {
                            status: t.status.clone(),
                            ended_ms: t.ended_ms,
                        },
                    ));
                }
                // 还在写 → 在跑。终态条目只有「通知之后很久还在写」才翻回来（= 新的一轮），
                // 此时 runs 先记上这一轮：它的收尾通知还没到，on_notification 还没数过它。
                Some(_) if !terminal || resumed => {
                    if resumed {
                        t.runs = t.runs.saturating_add(1);
                    }
                    t.status = "running".into();
                    t.summary = None;
                    t.ended_ms = 0;
                }
                _ => {}
            }
        }
        self.settled.extend(newly_settled);
        // 后台命令没有独立记录可纠，收尾信号全在父记录里 —— 父记录要是没写下来
        // （父进程被打断/退出、机器重启），这条就永远停在「执行中」。
        // 但有一个**硬事实**能给它收尾：父会话都结束了，它派生的后台命令不可能还在跑。
        // 判据是「父会话是否已结束」这个事实，不是「挂了超过 N 小时」那种时间阈值。
        if let Some(ended_ms) = opts.parent_ended_ms {
            for t in items
                .iter_mut()
                .filter(|t| t.kind == "bg" && t.status == "running")
            {
                t.status = ORPHANED_STATUS.into();
                t.summary = None;
                // 收尾时刻取父会话最后一次写入 —— 它至迟在那一刻就不在了。
                // 用「此刻」的话年龄恒为 0，保留窗口永远淘汰不掉它。
                t.ended_ms = ended_ms;
            }
        }
        // 归类：判据是磁盘事实（有没有把结果交回去），不是上游那句英文文案
        for t in items.iter_mut() {
            t.outcome = outcome_of(&t.status);
        }
        if opts.full {
            // 全量视角（历史会话展开）：保留窗口是「当前状态面板」的口径，这里不适用
            items
        } else {
            retain_recent(items, now_ms)
        }
    }
}

/// [`BgTracker::reconciled`] 的可选项。默认值 = 热路径那份口径（带保留窗口、
/// 不补目录、不知道父会话是否结束），既有调用点与测试都用它。
#[derive(Default)]
struct ReconcileOpts {
    /// 父会话已结束，值是它最后一次写入的时刻（epoch 毫秒）。
    /// `None` = 父会话还活着（或调用方不知道），此时不给后台命令收尾。
    parent_ended_ms: Option<u64>,
    /// 全量视角：不套保留窗口。给「展开一条历史会话的全部子会话」用。
    full: bool,
    /// 父记录里没有、但 `subagents/` 目录里确实存在的子会话（全量视角才补）
    extra: Vec<SubTask>,
}

/// 「父会话都结束了，它还挂着」—— 由我们合成的收尾状态，上游不会写出这个词。
///
/// 不复用 `killed` / `stopped`：那两个是上游真写过的原文，混进来就分不清
/// 「上游说它被停了」和「我们据事实推断它不可能还在跑」。归类同样是
/// [`SubTaskOutcome::Interrupted`] —— 它不是自己跑砸的。
const ORPHANED_STATUS: &str = "orphaned";

/// 由上游原文状态归出 [`SubTaskOutcome`]。
///
/// 到这一步 `status` 已经被 [`BgTracker::reconciled`] 按磁盘纠过了：凡是把结果交回去的
/// 都已经是 `completed`，所以这里剩下的 `killed` / `stopped` 必然是**没交回结果就被
/// 掐断的** —— 父会话退出连带（`killed`，实测是父会话被中断时一次性发给当时所有在跑
/// 子代理的统一通知，同一时刻三条同状态）或有人主动 `TaskStop`（`stopped`）。
/// 这两种都不是「它自己跑砸了」，归 [`SubTaskOutcome::Interrupted`]。
///
/// 只有 `failed` 才是真跑砸（卡死、API 报错、退出码非零），`summary` 里带着原因。
///
/// 认不出来的新状态一律当 `Failed`：这是保守方向 —— 宁可让用户多看一眼，
/// 也别把真出的错悄悄画成灰色。
fn outcome_of(status: &str) -> SubTaskOutcome {
    match status {
        "running" => SubTaskOutcome::Running,
        "completed" => SubTaskOutcome::Completed,
        "killed" | "stopped" | ORPHANED_STATUS => SubTaskOutcome::Interrupted,
        _ => SubTaskOutcome::Failed,
    }
}

/// 终态条目的保留窗口：结束超过这么久就不再出现在会话的子任务清单里。
///
/// 取 24 小时。这份清单是**「这个会话当下在忙什么」的状态面板**，不是归档：
/// 一天之内跑过的还可能被回看（改完隔夜回来看昨天那批子代理的结论），
/// 再往前就属于历史 —— 真要翻，从子会话正文接口按 id 拉。
///
/// 不取更短是因为一次大活动辄跨越午休/过夜；不取更长是因为本机实测单会话
/// 11 天能累 147 条，窗口越宽越接近「没有窗口」。
const BG_RETAIN_MS: u64 = 24 * 3600 * 1000;

/// 终态条目的条数上限：一天之内也可能爆量（实测单会话最多 234 份子会话记录），
/// 光有时间窗兜不住。超出时按结束时刻**留最近的**。
const BG_MAX_ITEMS: usize = 50;

/// 按 [`BG_RETAIN_MS`] + [`BG_MAX_ITEMS`] 淘汰终态条目，保持原有顺序。
///
/// **在跑的一条都不淘汰**：跑了多久都得看得见（长命 dev server 就是这样）。
/// 终态但 `ended_ms == 0`（父记录给了终态却没带时间戳，且磁盘也没纠出时刻）的，
/// 退回用起跑时刻算年龄 —— 总比无限期挂着强；两个时刻都拿不到才保留。
fn retain_recent(items: Vec<SubTask>, now_ms: u64) -> Vec<SubTask> {
    let age_of = |t: &SubTask| -> Option<u64> {
        let at = if t.ended_ms > 0 {
            t.ended_ms
        } else {
            iso_to_ms(&t.started_at)?
        };
        Some(now_ms.saturating_sub(at))
    };
    let mut kept: Vec<SubTask> = items
        .into_iter()
        .filter(|t| {
            t.outcome == SubTaskOutcome::Running || age_of(t).is_none_or(|age| age <= BG_RETAIN_MS)
        })
        .collect();
    let over = kept
        .iter()
        .filter(|t| t.outcome != SubTaskOutcome::Running)
        .count()
        .saturating_sub(BG_MAX_ITEMS);
    if over > 0 {
        // 只淘汰终态条目里最老的 `over` 条：先挑出它们的下标，再按下标剔除，
        // 这样剩下的仍是原始顺序（面板是按派活先后读的）。
        let mut idx: Vec<usize> = kept
            .iter()
            .enumerate()
            .filter(|(_, t)| t.outcome != SubTaskOutcome::Running)
            .map(|(i, _)| i)
            .collect();
        idx.sort_by_key(|&i| (kept[i].ended_ms, i));
        let drop: std::collections::HashSet<usize> = idx.into_iter().take(over).collect();
        kept = kept
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !drop.contains(i))
            .map(|(_, t)| t)
            .collect();
    }
    kept
}

/// 通知正文的指纹，用于「同一条通知落了两次盘」的去重（见 [`BgTracker::seen_notif`]）。
/// 只求区分同一个子会话名下的不同通知，不求抗碰撞，标准库的 hasher 足够。
fn text_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// 取出 <tag>值</tag> 里的值
fn tag_value(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = text.find(&open)? + open.len();
    let e = text[s..].find(&close)? + s;
    Some(text[s..e].trim().to_string())
}

/// 从 `toolUseResult` 判定「这次调用有没有留下一个还在后台跑的东西」，
/// 返回 (任务号, 种类)。判据只取结构化字段，理由：
///
/// - 工具名会变：派子代理的工具从 `Task` 改成了 `Agent`，按名字判会整条哑掉；
/// - 入参不可靠：派子代理的 input 里根本没有 `run_in_background`；
/// - 文本标记会误伤：结果正文里出现 "with ID: " 的普通命令（比如 grep 到了这段
///   源码本身）会被错认成后台任务。
///
/// 而任务号本身就只由这两个字段给出，没有它就不可能有后续的完成通知 ——
/// 「有任务号」与「是后台任务」是同一件事，用它当判据不会错位。
fn bg_identity(meta: Option<&Value>) -> Option<(String, &'static str)> {
    let m = meta?;
    for (field, kind) in [("agentId", "agent"), ("backgroundTaskId", "bg")] {
        if let Some(id) = m.get(field).and_then(Value::as_str) {
            if !id.is_empty() {
                return Some((id.to_string(), kind));
            }
        }
    }
    None
}

/// 从 `toolUseResult` 认出「这次调用是主动停掉了一个后台任务」，返回被停的任务号。
///
/// **`TaskStop` 停掉的任务不会再发 `<task-notification>`** —— 这条结果就是它唯一的
/// 收尾信号。实测本机 17 条永远停在「执行中」的后台命令里，13 条正是这么来的
/// （`83fd6036…jsonl:3414` 停掉 `bwqopavu5`、`460884c8…jsonl:27790` 停掉
/// `b19cm04gi`，等等），全文再无第二处提到它们。
///
/// 判据同样只取结构化字段：`task_id` + `task_type` 这一对只有停止结果才给
/// （实测 20 条，形态清一色 `command,message,task_id,task_type`），
/// 不去正文里匹配 "Successfully stopped" 那句话 —— 那句话是会改的。
fn stopped_task_id(meta: Option<&Value>) -> Option<String> {
    let m = meta?;
    m.get("task_type").and_then(Value::as_str)?;
    let id = m.get("task_id").and_then(Value::as_str)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// 「已交回结果」的静置窗口：收尾形态还要静这么久才作数。
///
/// 一条 assistant 回复是**按内容块拆开落盘**的（thinking 一条、text 一条、tool_use 一条），
/// 所以流式写入途中，最后一条常常正好是 thinking 或 text —— 形态与「已收尾」一模一样。
/// 没有这道窗口就会把正在跑的子会话当场判死（第一版就是这么误判本机唯一在跑的那个的）。
///
/// **余量只有约 1.31 倍，不是「绰绰有余」**。实测本机 341 份子会话记录、18122 个
/// 「收尾形态 → 还有下一条」的样本：剔掉跨过一条完成通知的（那些是续跑）之后剩 18073 个，
/// 最大回合内空窗 189.5s（`a28f3cb5109c6d226`，`text` → `tool_use`），超过 300s 的 0 个；
/// 若把 `a6ec3c1479bfb1f93` 那对算进来（它起点比通知早 28ms，算不算续跑两可），
/// 最大是 229.2s。300 / 229.2 ≈ 1.31。机器负载重一点是有可能越过 300s 的。
///
/// 之所以还敢用这个数，是因为**误判可自愈**：判定结果只在 [`BgTracker::reconciled`] 里
/// 现算、绝不写回 `items`，客户端那层缓存也带着子会话目录 mtime 与最长寿命做键
/// （见 `client/src/agent.rs` 的 `MsgCache`）。子会话下一次写入就把 `idle_ms` 打回去，
/// 状态自己翻回 running —— 误判最多让胶囊闪一下，不会像原 bug 那样永久挂着。
/// 所以这个数不该靠调大来「买余量」，那只是把误判概率往后挪。
///
/// 只在父会话没写下完成通知时才用得上 —— 通知在的话早就收尾了，这几分钟无人察觉。
///
/// **缓存 [`SessionScanner::messages`] 的一方不能把结果留过这个时长**：这两道判定
/// （静置够久 → 收尾、停在半路太久 → 放弃）是随墙钟翻的，那一刻没有任何文件在变，
/// 只按文件 mtime 做缓存键的话它们永远轮不到执行。
pub const SUBAGENT_SETTLE_MS: u64 = 5 * 60 * 1000;

/// 「被 kill 在半路」的兜底时限。
///
/// 只用于停在半路（最后一条不是收尾形态）、父会话又没写下通知的条目 —— 正常收尾靠
/// [`SubAgentTail::Finished`] 认，不靠时间。取 2 小时，比一次慢构建/跑测试宽得多。
const SUBAGENT_ABANDON_MS: u64 = 2 * 3600 * 1000;

/// 子会话记录最后一条长什么样 —— 「它还在不在干活」的结构判据。
///
/// 子会话把结果交回父会话时，最后落下的必然是一条 `type:"assistant"` 且 content 里
/// 没有 `tool_use` 的记录（说完就交差，不会再写）；停在别的形态就是还在半路
/// （等工具返回 / 刚吃进一条工具结果 / 记录正被写入）。
///
/// 实测本机 341 份子会话记录的末条：334 份是 `assistant`-无-`tool_use`，全是已完成的；
/// 2 份是 `assistant`+`tool_use`，正是当时真在跑的；5 份是 `user`，父记录里对应的状态
/// 全是 killed/failed（半路被打断）。非 `assistant` 的末条不止 `user` 一种，还见过
/// `attachment` —— 都归 [`SubAgentTail::Midflight`]，无须逐种枚举：**判据是「是不是
/// 收尾形态」，不是「属于哪一种半路形态」**，所以上游新增记录类型不会让它失灵。
///
/// **判形态而不判时间**，是因为纯按时间会误杀正在等慢工具的子会话；反过来，形态也
/// 得配一道静置窗口才作数（assistant 按内容块拆条落盘，流式途中形态与收尾一样），
/// 两边的实测数据见 [`SUBAGENT_SETTLE_MS`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubAgentTail {
    /// 已交回结果，不会再写
    Finished,
    /// 停在半路（等工具 / 刚收到工具结果 / 正被写入）
    Midflight,
}

/// 一个会话的子会话记录目录：`<父会话 jsonl 同级>/<会话号>/subagents/`。
///
/// 只在构造时 `read_dir` + `stat`（实测最大的目录 234 项，1.15ms），不读任何内容；
/// 真要看形态时才按 id 读那一份的尾部 —— 每轮需要看的通常只有一两条。
struct SubAgentDir {
    dir: PathBuf,
    /// 子会话号（= 父记录里的 `agentId`）→ 最后写入时刻（epoch 毫秒）
    last_write: HashMap<String, u64>,
}

/// 一条会话派过几个子代理：只数 `<会话 jsonl 同级>/<会话号>/subagents/` 下的
/// `agent-*.jsonl` 文件数，**不打开任何文件、不 stat**。
///
/// 与 [`SubAgentDir::scan`] 的区别就在这：那个要逐个 `stat` 拿最后写入时刻（对齐状态用），
/// 这个只要一次 `read_dir` 把文件名过一遍。目录不存在（绝大多数会话都没派过子代理）时
/// `read_dir` 当场返回 ENOENT，代价接近于零。
///
/// 只数 `agent-*`：后台命令（`kind:"bg"`）没有独立记录，本来就不在这个目录里，
/// 侧栏的树也只列子代理。
fn count_sub_agents(session_path: &Path) -> usize {
    let Some(dir) = session_path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|stem| {
            session_path
                .parent()
                .map(|p| p.join(stem).join("subagents"))
        })
    else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
        })
        .count()
}

impl SubAgentDir {
    fn scan(session_path: &Path) -> Self {
        let dir = session_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| {
                session_path
                    .parent()
                    .map(|p| p.join(stem).join("subagents"))
            })
            .unwrap_or_default();
        let mut last_write = HashMap::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name
                    .strip_prefix("agent-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                else {
                    continue;
                };
                let ms = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                last_write.insert(id.to_string(), ms);
            }
        }
        Self { dir, last_write }
    }

    /// 某个子会话记录的路径（不保证存在）
    fn file_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("agent-{id}.jsonl"))
    }

    /// 子会话的展示名，取自它旁边那份 `agent-<id>.meta.json`。
    ///
    /// 父记录漏掉的子会话（阻塞式派活不写 `agentId`）在父记录里没有任何展示名，
    /// 全量清单里只剩一个 id 没法看。上游自己把派活说明写在这份 sidecar 里
    /// （实测形如 `{"agentType":"Explore","description":"Extract VitaAgent UI reference",…}`），
    /// 直接取 `description`，没有就退回 `agentType`。读不到就返回 None。
    fn meta_label(&self, id: &str) -> Option<String> {
        for key in ["description", "agentType"] {
            if let Some(t) = self.meta_str(id, key) {
                return Some(truncate(&t, 80));
            }
        }
        None
    }

    /// 读 `agent-<id>.meta.json` 里的一个字符串字段（空串当没有）。
    /// 实测内容形如
    /// `{"agentType":"Explore","description":"…","toolUseId":"toolu_016u29…","model":"opus"}`。
    fn meta_str(&self, id: &str, key: &str) -> Option<String> {
        let txt = fs::read_to_string(self.dir.join(format!("agent-{id}.meta.json"))).ok()?;
        let v: Value = serde_json::from_str(&txt).ok()?;
        let t = v.get(key).and_then(Value::as_str)?;
        (!t.trim().is_empty()).then(|| t.to_string())
    }

    /// 目录里所有子会话记录的最新写入时刻（没有就 0）。供上层做缓存键。
    fn newest_ms(&self) -> u64 {
        self.last_write.values().copied().max().unwrap_or(0)
    }

    /// 读某个子会话记录的最后一条完整记录，判形态。
    ///
    /// 只读尾部 256KB：一条记录再大也进得来，而整份记录可达数 MB，每轮全读吃不消。
    /// 末尾那行可能正被写入（只有半截）——解析不了就说明它此刻正在写，算半路。
    fn tail(&self, id: &str) -> Option<SubAgentTail> {
        let text = read_tail(&self.file_of(id), 256 * 1024).ok()?;
        let mut lines = text.lines().rev();
        let last = lines.next()?;
        let v: Value = match serde_json::from_str(last) {
            Ok(v) => v,
            // 半截行 = 正在写，就是还在跑
            Err(_) => return Some(SubAgentTail::Midflight),
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            return Some(SubAgentTail::Midflight);
        }
        let has_tool_use = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            });
        Some(if has_tool_use {
            SubAgentTail::Midflight
        } else {
            SubAgentTail::Finished
        })
    }
}

/// 重放 TaskCreate / TaskUpdate，还原终端里那份「任务清单」。
///
/// 清单是跨多条记录累积出来的状态，没法在 entry_to_brief 里按行无状态解析：
/// - TaskCreate 的 tool_use 只带 subject，任务号要等它的 tool_result
///   （"Task #N created successfully: ..."）才拿得到，故需按 tool_use_id 暂存；
/// - TaskUpdate 只带 taskId 与新状态，必须落到已有的那一项上。
#[derive(Default)]
struct TodoTracker {
    /// tool_use_id -> subject（等待对应 tool_result 回填任务号）
    pending: HashMap<String, String>,
    items: Vec<TodoItem>,
    dirty: bool,
}

impl TodoTracker {
    fn observe(&mut self, v: &Value) {
        let Some(items) = v.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("tool_use") => self.on_tool_use(item),
                Some("tool_result") => self.on_tool_result(item),
                _ => {}
            }
        }
    }

    fn on_tool_use(&mut self, item: &Value) {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let input = item.get("input");
        match name {
            "TaskCreate" => {
                let (Some(id), Some(subject)) = (
                    item.get("id").and_then(Value::as_str),
                    input.and_then(|i| i.get("subject")).and_then(Value::as_str),
                ) else {
                    return;
                };
                self.pending.insert(id.to_string(), subject.to_string());
            }
            "TaskUpdate" => {
                let Some(input) = input else { return };
                let Some(task_id) = input.get("taskId").and_then(Value::as_str) else {
                    return;
                };
                let status = input.get("status").and_then(Value::as_str);
                let subject = input.get("subject").and_then(Value::as_str);
                // status=deleted 表示该任务被移除，清单里也不该再留着
                if status == Some("deleted") {
                    let before = self.items.len();
                    self.items.retain(|t| t.id != task_id);
                    self.dirty |= self.items.len() != before;
                    return;
                }
                if let Some(t) = self.items.iter_mut().find(|t| t.id == task_id) {
                    if let Some(s) = status {
                        t.status = s.to_string();
                    }
                    if let Some(s) = subject {
                        t.subject = s.to_string();
                    }
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn on_tool_result(&mut self, item: &Value) {
        let Some(use_id) = item.get("tool_use_id").and_then(Value::as_str) else {
            return;
        };
        let Some(subject) = self.pending.remove(use_id) else {
            return;
        };
        // "Task #12 created successfully: xxx" —— 取出任务号
        let text = tool_result_text(item);
        let Some(id) = text
            .split_once('#')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        else {
            return;
        };
        self.items.push(TodoItem {
            id: id.to_string(),
            subject,
            status: "pending".into(),
        });
        self.dirty = true;
    }

    /// 有变化时产出一份当前清单快照（JSON，交前端渲染成勾选列表）
    fn take_snapshot(&mut self, ts: &str) -> Option<MessageBrief> {
        if !self.dirty || self.items.is_empty() {
            return None;
        }
        self.dirty = false;
        Some(MessageBrief {
            role: "todos".into(),
            content: serde_json::to_string(&self.items).ok()?,
            timestamp: ts.to_string(),
            is_error: false,
            tools: Vec::new(),
            tool_use_id: String::new(),
        })
    }
}

/// 解析文件头部：初始 cwd、第一条真实用户提示词、会话开始时间
fn parse_head(path: &Path) -> HeadInfo {
    let mut info = HeadInfo::default();
    let Ok(mut f) = fs::File::open(path) else {
        return info;
    };
    let mut buf = vec![0u8; HEAD_BYTES];
    let Ok(n) = f.read(&mut buf) else { return info };
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if info.cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                info.cwd = Some(c.to_string());
            }
        }
        if info.started_at.is_none() {
            if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
                info.started_at = Some(ts.to_string());
            }
        }
        if info.prompt.is_none()
            && v.get("type").and_then(Value::as_str) == Some("user")
            && !v.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
            && !v
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            let content = v.pointer("/message/content");
            if let Some(t) = user_text(content) {
                info.prompt = Some(t);
            } else if let Some(args) = content
                .and_then(Value::as_str)
                .and_then(extract_command_args)
            {
                // 会话以斜杠命令开场（如 /goal <目标>）：把命令参数当作任务描述
                info.prompt = Some(args);
            }
        }
        if info.cwd.is_some() && info.prompt.is_some() && info.started_at.is_some() {
            break;
        }
    }
    info
}

/// 从 <command-args>…</command-args> 提取斜杠命令参数
fn extract_command_args(text: &str) -> Option<String> {
    let start = text.find("<command-args>")? + "<command-args>".len();
    let end = text[start..].find("</command-args>")? + start;
    let args = text[start..end].trim();
    if args.is_empty() {
        None
    } else {
        Some(truncate(args, 500))
    }
}

// ---------- 文件工具 ----------

/// 会话**此刻**的工作目录：现读 jsonl 尾部，取最后一条记录的 `cwd`。
///
/// 与 [`SessionSummary::shell_cwd`] 同源，区别只在时机：那份来自定期扫描的快照，
/// 而扫描循环在 macOS 后台被 App Nap 压到一两分钟一轮；本函数是**按需现读**，
/// 新鲜度等同于调用它的那一刻。目录浏览、文件夹操作、文件落盘都要用它 ——
/// 定位差一个 `cd`，给会话的路径它自己去看就是错的。
///
/// 只读尾部 64KB：会话 jsonl 动辄几十 MB，这里要的只是最后一条记录，
/// 而每次目录查询都会调用它，不能走完整解析。
pub fn current_cwd_of_session(jsonl: &Path) -> Option<String> {
    const TAIL: u64 = 64 * 1024;
    let tail = read_tail(jsonl, TAIL).ok()?;
    // 从后往前找第一条能解析出 cwd 的记录。逐行反向比整体解析便宜得多，
    // 而且尾部第一行常是被截断的半行，正向扫描反而更容易踩空。
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(c) = v.get("cwd").and_then(Value::as_str) {
            if !c.is_empty() {
                return Some(c.to_string());
            }
        }
    }
    None
}

impl SessionScanner {
    /// 按会话 id 找到它的 jsonl（在扫描目录下逐个项目目录找 `<id>.jsonl`）。
    ///
    /// 项目目录通常十来个，一次 `join + exists` 就命中，不做递归。
    pub fn session_path(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.is_empty() || session_id.contains(['/', '\\']) {
            return None; // 防路径穿越：id 来自 hub 下发
        }
        let file = format!("{session_id}.jsonl");
        let rd = fs::read_dir(&self.projects_dir).ok()?;
        for e in rd.flatten() {
            let cand = e.path().join(&file);
            if cand.is_file() {
                return Some(cand);
            }
        }
        None
    }

    /// 会话此刻的工作目录（找不到会话或读不出 cwd 时 None）
    pub fn session_cwd_now(&self, session_id: &str) -> Option<String> {
        current_cwd_of_session(&self.session_path(session_id)?)
    }
}

/// 文件最后修改时刻（epoch 毫秒）；取不到就 0
fn file_mtime_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_tail(path: &Path, max_bytes: u64) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    let mut s = String::from_utf8_lossy(&buf).to_string();
    // 跳过可能被截断的第一行
    if start > 0 {
        if let Some(pos) = s.find('\n') {
            s = s.split_off(pos + 1);
        }
    }
    Ok(s)
}

fn count_lines_from(path: &Path, offset: u64) -> Result<u64> {
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
    }
    Ok(count)
}

/// 与 Claude Code 的项目目录命名一致：非字母数字字符替换为 '-'
/// 从 `dir` 向上找最近的 git 仓库根（含 `.git` 的目录），找不到返回 None。
///
/// `.git` 可能是目录（普通仓库）也可能是文件（worktree / submodule 里是一行 gitdir 指向），
/// 所以只判存在、不判类型。`ancestors()` 走到根自然结束，不会无限向上。
fn git_root_of(dir: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    std::path::Path::new(dir)
        .ancestors()
        .find(|a| a.join(".git").exists())
        .map(|a| a.to_string_lossy().to_string())
}

pub fn encode_path(p: &str) -> String {
    // 先去掉尾随分隔符再编码：Windows 上 sysinfo 上报的进程 cwd 常带尾随反斜杠
    // （D:\proj\），而 ~/.claude/projects 下的项目目录名由无尾随分隔符的 cwd
    // 编码而来（D--proj）。不去掉，进程侧会多一个尾随 '-'（D--proj-），与会话的
    // project_key 对不上、配不成对 —— 会话就沦为「等待输入」的占位进程、内容永不
    // 同步。macOS 的 cwd 无尾随斜杠，故此前只在 Windows 上暴露。
    let s: String = p
        .trim_end_matches(['/', '\\'])
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    normalize_key_case(s)
}

/// Windows 路径大小写不敏感：sysinfo 上报的进程 cwd 与 Claude Code 建目录时记录的
/// 盘符/路径大小写可能不同（实测同一盘符既出现 `D--` 又出现 `d--`），不统一大小写
/// 就会配不成对、会话永远「等待输入」。故 Windows 下把配对键统一小写；其它平台
/// 路径大小写敏感，保持原样。project_key（目录名）也要走同一化，两侧才对得上。
#[cfg(windows)]
pub fn normalize_key_case(s: String) -> String {
    s.to_ascii_lowercase()
}
#[cfg(not(windows))]
pub fn normalize_key_case(s: String) -> String {
    s
}

fn short_name(cwd: &str) -> String {
    // 跳过空段：Windows 上报的 cwd 常带尾随反斜杠（D:\proj\），
    // 直接取最后一段会得到空串 → 标题/项目名显示成空
    cwd.split(['/', '\\'])
        .rev()
        .find(|s| !s.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars).collect();
        format!("{t}…")
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod todo_tests {
    use super::*;

    fn assistant_tool(id: &str, name: &str, input: Value) -> Value {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-17T10:00:00Z",
            "message": { "content": [
                { "type": "tool_use", "id": id, "name": name, "input": input }
            ]}
        })
    }

    fn tool_result(use_id: &str, text: &str) -> Value {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-17T10:00:01Z",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": use_id, "content": text }
            ]}
        })
    }

    /// 任务号来自 tool_result，不在 TaskCreate 的入参里 —— 必须等结果回来才成形
    #[test]
    fn task_id_comes_from_tool_result() {
        let mut t = TodoTracker::default();
        t.observe(&assistant_tool(
            "u1",
            "TaskCreate",
            serde_json::json!({ "subject": "甲" }),
        ));
        // 只有 tool_use 时还拿不到任务号，清单应为空
        assert!(t.items.is_empty());
        assert!(t.take_snapshot("ts").is_none());

        t.observe(&tool_result("u1", "Task #7 created successfully: 甲"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, "7");
        assert_eq!(t.items[0].status, "pending");
        assert!(t.take_snapshot("ts").is_some());
        // 快照取走后不再重复产出
        assert!(t.take_snapshot("ts").is_none());
    }

    #[test]
    fn update_changes_status_and_delete_removes() {
        let mut t = TodoTracker::default();
        t.observe(&assistant_tool(
            "u1",
            "TaskCreate",
            serde_json::json!({ "subject": "甲" }),
        ));
        t.observe(&tool_result("u1", "Task #1 created successfully: 甲"));
        t.observe(&assistant_tool(
            "u2",
            "TaskCreate",
            serde_json::json!({ "subject": "乙" }),
        ));
        t.observe(&tool_result("u2", "Task #2 created successfully: 乙"));
        let _ = t.take_snapshot("ts");

        t.observe(&assistant_tool(
            "u3",
            "TaskUpdate",
            serde_json::json!({ "taskId": "2", "status": "completed" }),
        ));
        assert_eq!(
            t.items.iter().find(|i| i.id == "2").unwrap().status,
            "completed"
        );
        assert!(t.take_snapshot("ts").is_some());

        t.observe(&assistant_tool(
            "u4",
            "TaskUpdate",
            serde_json::json!({ "taskId": "1", "status": "deleted" }),
        ));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, "2");
    }

    /// 更新一个不存在的任务号不应产生脏快照（否则前端会收到无意义的重复清单）
    #[test]
    fn update_unknown_task_is_ignored() {
        let mut t = TodoTracker::default();
        t.observe(&assistant_tool(
            "u1",
            "TaskUpdate",
            serde_json::json!({ "taskId": "99", "status": "completed" }),
        ));
        assert!(t.items.is_empty());
        assert!(t.take_snapshot("ts").is_none());
    }

    /// plan 模式的方案正文要完整取出，而不是只剩工具名
    #[test]
    fn exit_plan_mode_yields_plan_role() {
        let v = assistant_tool(
            "u1",
            "ExitPlanMode",
            serde_json::json!({ "plan": "1. 改 A\n2. 改 B" }),
        );
        let m = entry_to_brief(&v).expect("应产出一条消息");
        assert_eq!(m.role, "plan");
        assert_eq!(m.content, "1. 改 A\n2. 改 B");
    }
}

#[cfg(test)]
mod bg_tests {
    use super::*;

    /// 派活的 tool_use：故意不带 `run_in_background` —— 真实记录里派子代理时它
    /// 压根不存在，判定不该依赖它，也不该依赖工具名。
    fn bg_use(id: &str, name: &str, desc: &str) -> Value {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-17T10:00:00Z",
            "message": { "content": [
                { "type": "tool_use", "id": id, "name": name,
                  "input": { "command": "yarn start", "description": desc } }
            ]}
        })
    }

    /// 工具结果 + 顶层 toolUseResult 元数据（种类与任务号的唯一来源）
    fn result_with(use_id: &str, text: &str, meta: Value) -> Value {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-17T10:00:01Z",
            "toolUseResult": meta,
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": use_id, "content": text }
            ]}
        })
    }

    /// 后台命令：任务号在 backgroundTaskId
    fn bg_result(use_id: &str, task_id: &str) -> Value {
        result_with(
            use_id,
            "Command running in background",
            serde_json::json!({ "stdout": "", "stderr": "", "backgroundTaskId": task_id }),
        )
    }

    /// 异步子代理：任务号在 agentId
    fn agent_result(use_id: &str, agent_id: &str) -> Value {
        result_with(
            use_id,
            "Async agent launched successfully.",
            serde_json::json!({
                "isAsync": true, "status": "async_launched",
                "agentId": agent_id, "description": "审查后端"
            }),
        )
    }

    fn notification(text: &str) -> Value {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-17T10:00:02Z",
            "message": { "content": text }
        })
    }

    #[test]
    fn tracks_background_command_until_notification() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "启动前端 dev server"));
        assert!(t.items.is_empty(), "拿到任务号前不该成形");

        t.observe(&bg_result("u1", "bhb69r9ff"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, "bhb69r9ff");
        assert_eq!(t.items[0].label, "启动前端 dev server");
        assert_eq!(t.items[0].status, "running");

        t.observe(&notification(
            "<task-notification>\n<task-id>bhb69r9ff</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        assert_eq!(t.items[0].status, "completed");
    }

    /// 异步子代理的任务号来自 agentId，不是 "with ID:"
    #[test]
    fn tracks_background_agent_by_agent_id() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Agent", "审查后端"));
        t.observe(&agent_result("u1", "a21278fd478be0810"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, "a21278fd478be0810");
    }

    /// 一条通知可带多个 task-id 共用一个 status；__orphan_summary__ 是内部标记要跳过
    #[test]
    fn notification_with_multiple_ids_skips_internal_markers() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&bg_result("u1", "b5eauqs4i"));
        t.observe(&bg_use("u2", "Bash", "乙"));
        t.observe(&bg_result("u2", "bmojunb33"));

        t.observe(&notification(
            "<task-notification>\n<task-id>b5eauqs4i</task-id>\n<task-id>bmojunb33</task-id>\n<task-id>__orphan_summary__:shell</task-id>\n<status>stopped</status>\n</task-notification>",
        ));
        assert!(t.items.iter().all(|i| i.status == "stopped"));
        assert_eq!(t.items.len(), 2, "内部标记不该混进清单");
    }

    /// 通知正文里嵌着刚跑完那个子代理的完整报告。报告里出现 `__orphan_summary__`
    /// 这几个字（比如它分析的正是这段源码）不该被当成孤儿汇总，把同批还在跑的
    /// 兄弟任务一起抹掉。判据只看 `<task-id>` 的值。
    #[test]
    fn orphan_marker_inside_report_body_is_not_a_summary() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Agent", "还在跑的"));
        t.observe(&agent_result("u1", "a11111111"));
        t.observe(&bg_use("u2", "Agent", "跑完的"));
        t.observe(&agent_result("u2", "a22222222"));

        t.observe(&notification(
            "<task-notification>\n<task-id>a22222222</task-id>\n<status>completed</status>\n             <result>报告：`__orphan_summary__` 那段判定有问题</result>\n</task-notification>",
        ));
        let by = |id: &str| t.items.iter().find(|i| i.id == id).unwrap().status.clone();
        assert_eq!(by("a22222222"), "completed");
        assert_eq!(
            by("a11111111"),
            "running",
            "正文里的字样不该顺手把兄弟任务停掉"
        );
    }

    /// 孤儿汇总只枚举了部分 id 时，漏网的运行中任务也应一并落终态：
    /// 会话续跑/压缩恢复后，此前所有后台 shell 都已不在，不能永远卡在「运行中」。
    #[test]
    fn orphan_summary_stops_unlisted_running_tasks() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "被枚举的"));
        t.observe(&bg_result("u1", "b5eauqs4i"));
        t.observe(&bg_use("u2", "Bash", "漏网的 dev server"));
        t.observe(&bg_result("u2", "bvvgsfndf"));

        // 通知里只列了 b5eauqs4i，没列 bvvgsfndf，但带 __orphan_summary__ 标记
        t.observe(&notification(
            "<task-notification>\n<task-id>b5eauqs4i</task-id>\n<task-id>__orphan_summary__:shell</task-id>\n<status>stopped</status>\n</task-notification>",
        ));
        assert!(
            t.items.iter().all(|i| i.status == "stopped"),
            "孤儿汇总应连同未枚举的运行中任务一起停掉"
        );
    }

    /// 普通(非孤儿汇总)通知不得波及未点名的任务：只有被 task-id 点到的才改状态
    #[test]
    fn normal_notification_leaves_unlisted_tasks_running() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&bg_result("u1", "b1"));
        t.observe(&bg_use("u2", "Bash", "乙"));
        t.observe(&bg_result("u2", "b2"));

        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        let b2 = t.items.iter().find(|i| i.id == "b2").unwrap();
        assert_eq!(b2.status, "running", "没点名的任务不该被普通通知波及");
    }

    /// Windows：路径大小写不敏感，sysinfo 报的盘符大小写可能与目录名不同
    /// （实测同一盘符既有 D-- 又有 d--），encode_path 统一小写后才配得上。
    #[cfg(windows)]
    #[test]
    fn windows_pairing_is_case_insensitive() {
        assert_eq!(
            encode_path("D:\\Program\\Foo"),
            encode_path("d:\\program\\foo")
        );
        assert_eq!(encode_path("D:\\Program\\Foo"), "d--program-foo");
    }

    /// 前台命令不该被收进来 —— 哪怕它的输出里恰好出现 "with ID: " 这串字。
    /// （真事：grep 源码时把这段注释本身 grep 出来了。按文本标记判会把它当后台任务。）
    #[test]
    fn foreground_command_is_ignored_even_if_output_mentions_id() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "列目录"));
        t.observe(&result_with(
            "u1",
            "scanner.rs:1: Command running in background with ID: zzz",
            serde_json::json!({ "stdout": "…with ID: zzz", "stderr": "" }),
        ));
        assert!(t.items.is_empty(), "没有任务号就不是后台任务");
    }

    /// 工具名换了也要照认：判据是 toolUseResult.agentId，不是 "Task"/"Agent" 这些名字
    #[test]
    fn agent_detected_regardless_of_tool_name() {
        for name in ["Task", "Agent", "SomeFutureName"] {
            let mut t = BgTracker::default();
            t.observe(&bg_use("u1", name, "调研"));
            t.observe(&agent_result("u1", "a1"));
            assert_eq!(t.items.len(), 1, "工具名 {name} 应照样识别");
            assert_eq!(t.items[0].kind, "agent");
            assert_eq!(t.items[0].label, "调研");
            assert_eq!(
                t.items[0].started_at, "2026-07-17T10:00:00Z",
                "起跑时刻取 tool_use 那条"
            );
        }
    }

    /// 同一个子代理被唤醒续跑（再来一条结果）应原地复活，而不是堆出重复条目
    #[test]
    fn resumed_agent_revives_in_place() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Agent", "调研"));
        t.observe(&agent_result("u1", "a1"));
        t.observe(&notification(
            "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        assert_eq!(t.items[0].status, "completed");

        t.observe(&bg_use("u2", "Agent", "调研"));
        t.observe(&agent_result("u2", "a1"));
        assert_eq!(t.items.len(), 1, "同一个 agentId 不该堆两条");
        assert_eq!(t.items[0].status, "running");
    }

    /// 清单反映的始终是当前状态：完成通知是原地改状态，不往后追加一条
    #[test]
    fn snapshot_reflects_current_state_once() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&bg_result("u1", "b1"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].status, "running");

        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        assert_eq!(t.items.len(), 1, "应是原地更新而非追加一条");
        assert_eq!(t.items[0].status, "completed");
    }

    /// TaskStop 的结果（`task_id` + `task_type`）
    fn stop_result(use_id: &str, task_id: &str, task_type: &str) -> Value {
        result_with(
            use_id,
            "{\"message\":\"Successfully stopped task\"}",
            serde_json::json!({
                "message": format!("Successfully stopped task: {task_id} (yarn start)"),
                "task_id": task_id, "task_type": task_type, "command": "yarn start"
            }),
        )
    }

    /// 主动停掉的后台命令此后不会再发通知 —— 停止结果就是它的收尾信号。
    /// 实测本机 13 条永远停在「执行中」的后台命令就是这么来的
    /// （如 `83fd6036…jsonl:3414` 停掉 `bwqopavu5`）。
    #[test]
    fn task_stop_ends_background_command() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&bg_result("u1", "b1"));
        assert_eq!(t.items[0].status, "running");

        t.observe(&bg_use("u2", "TaskStop", "停掉它"));
        t.observe(&stop_result("u2", "b1", "local_bash"));
        assert_eq!(t.items.len(), 1, "停止结果不该新建条目");
        assert_eq!(t.items[0].status, "stopped");
        assert!(
            t.items[0].ended_ms > 0,
            "收尾时刻要落下来，否则复活判定会误翻"
        );
    }

    /// 子代理同样可以被主动停掉（实测 20 条停止结果里 5 条是 `local_agent`）
    #[test]
    fn task_stop_ends_async_agent() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Agent", "审查后端"));
        t.observe(&agent_result("u1", "a1"));
        t.observe(&stop_result("u2", "a1", "local_agent"));
        assert_eq!(t.items[0].status, "stopped");
    }

    /// 停的不是清单里的任务时，不许凭空造条目
    #[test]
    fn task_stop_of_unknown_task_adds_nothing() {
        let mut t = BgTracker::default();
        t.observe(&stop_result("u1", "bzzz", "local_bash"));
        assert!(t.items.is_empty());
    }

    /// 记录文件不按时间戳排：完成通知那行常常落在起跑那行之前。
    /// 实测 `83fd6036…jsonl` 通知在 :3971、起跑记录在 :3975，
    /// `383ed76b…jsonl` 更是差了 159 行、时间戳差 4.5 分钟。
    /// 通知先到时必须按 `<tool-use-id>` 存着，等起跑记录到了补上。
    #[test]
    fn notification_ahead_of_launch_still_ends_task() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>u1</tool-use-id>\n<status>completed</status>\n</task-notification>",
        ));
        assert!(t.items.is_empty(), "此刻条目还没成形");

        t.observe(&bg_result("u1", "b1"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].status, "completed", "早到的通知要补回来");
        assert!(t.items[0].ended_ms > 0);
    }

    /// 早到的通知只认任务号对得上的那次起跑 —— 同号任务再起一次时不许套用旧终态
    #[test]
    fn early_notification_only_applies_to_its_own_task() {
        let mut t = BgTracker::default();
        t.observe(&notification(
            "<task-notification>\n<task-id>bold</task-id>\n<tool-use-id>u1</tool-use-id>\n<status>failed</status>\n</task-notification>",
        ));
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&bg_result("u1", "bnew"));
        assert_eq!(t.items[0].status, "running", "任务号对不上就别套");
    }

    /// 失败原因就是通知里的 `<summary>` 原文 —— 界面能说出「为什么」全靠它。
    /// 样本取自本机真实记录：后台命令带退出码、子代理带限流/卡死的原话。
    #[test]
    fn failure_summary_is_carried_verbatim() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "Start web dev server"));
        t.observe(&bg_result("u1", "byk6o6vis"));
        t.observe(&notification(
            "<task-notification>\n<task-id>byk6o6vis</task-id>\n<status>failed</status>\n<summary>Background command \"Start web dev server\" failed with exit code 137</summary>\n</task-notification>",
        ));
        assert_eq!(t.items[0].status, "failed");
        assert_eq!(
            t.items[0].summary.as_deref(),
            Some("Background command \"Start web dev server\" failed with exit code 137"),
            "退出码嵌在这句话里，原样带出去，不去抠数字"
        );

        let mut t = BgTracker::default();
        t.observe(&bg_use("u2", "Agent", "Deploy third release"));
        t.observe(&agent_result("u2", "a1111111111111111"));
        t.observe(&notification(
            "<task-notification>\n<task-id>a1111111111111111</task-id>\n<status>failed</status>\n<summary>Agent \"Deploy third release\" failed: Agent stalled: no progress for 600s (stream watchdog did not recover)</summary>\n</task-notification>",
        ));
        assert_eq!(
            t.items[0].summary.as_deref(),
            Some(
                "Agent \"Deploy third release\" failed: Agent stalled: no progress for 600s (stream watchdog did not recover)"
            )
        );
    }

    /// 通知不带 `<summary>` 时行为与改前一致：状态照落，原因留空（不编一个出来）
    #[test]
    fn notification_without_summary_leaves_reason_empty() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&bg_result("u1", "b1"));
        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>failed</status>\n</task-notification>",
        ));
        assert_eq!(t.items[0].status, "failed");
        assert_eq!(t.items[0].summary, None);
        // 没有原因时该字段整个不下发，前端拿到的就是 undefined
        let json = serde_json::to_string(&t.items[0]).unwrap();
        assert!(!json.contains("summary"), "为空时不该出现在报文里：{json}");
    }

    /// 早到的通知（起跑记录还没轮到）也要把原因存住，等条目成形时一并补上
    #[test]
    fn early_notification_carries_its_summary() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "Poll windows build again"));
        t.observe(&notification(
            "<task-notification>\n<task-id>b6a9qfb2x</task-id>\n<tool-use-id>u1</tool-use-id>\n<status>failed</status>\n<summary>Background command \"Poll windows build again\" failed with exit code 1</summary>\n</task-notification>",
        ));
        t.observe(&bg_result("u1", "b6a9qfb2x"));
        assert_eq!(t.items[0].status, "failed");
        assert_eq!(
            t.items[0].summary.as_deref(),
            Some("Background command \"Poll windows build again\" failed with exit code 1")
        );
    }

    /// 孤儿汇总那句 summary 讲的是「上一轮整批没留下收尾记录」，不对应任何一条任务，
    /// 挂上去就成了一句人人都有、谁也不对应的假原因。
    #[test]
    fn orphan_summary_does_not_become_a_per_task_reason() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&bg_result("u1", "b5eauqs4i"));
        t.observe(&bg_use("u2", "Bash", "漏网的 dev server"));
        t.observe(&bg_result("u2", "bvvgsfndf"));
        t.observe(&notification(
            "<task-notification>\n<task-id>b5eauqs4i</task-id>\n<task-id>__orphan_summary__:shell</task-id>\n<status>stopped</status>\n<summary>No completion record was found for this background shell command from the previous session.</summary>\n</task-notification>",
        ));
        assert!(t.items.iter().all(|i| i.status == "stopped"));
        assert!(
            t.items.iter().all(|i| i.summary.is_none()),
            "整批的说明不该冒充单条任务的原因"
        );
    }

    /// 主动停掉（TaskStop）没有原因可说 —— 它的 message 只复述 id 与命令；
    /// 上一轮跑留下的那句更是过期的，必须清掉。
    #[test]
    fn task_stop_clears_stale_reason() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&bg_result("u1", "b1"));
        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>failed</status>\n<summary>Background command \"起 dev server\" failed with exit code 1</summary>\n</task-notification>",
        ));
        assert!(t.items[0].summary.is_some());
        t.observe(&stop_result("u9", "b1", "shell"));
        assert_eq!(t.items[0].status, "stopped");
        assert_eq!(t.items[0].summary, None, "状态改了，旧原因就过期了");
    }

    /// 通知没带 tool-use-id（孤儿汇总、MCP 任务）时维持原样：只作用于已成形的条目
    #[test]
    fn notification_without_tool_use_id_is_not_buffered() {
        let mut t = BgTracker::default();
        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        t.observe(&bg_use("u1", "Bash", "起 dev server"));
        t.observe(&bg_result("u1", "b1"));
        assert_eq!(t.items[0].status, "running");
    }
}

#[cfg(test)]
mod bg_notification_path_tests {
    use super::*;

    /// 完成通知也可能是一条 queue-operation、文本挂在顶层 content 上。
    /// 漏掉这条路径的话后台任务只进不出，永远停在「运行中」。
    #[test]
    fn picks_up_notification_from_top_level_content() {
        let mut t = BgTracker::default();
        t.observe(&serde_json::json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "tool_use", "id": "u1", "name": "Agent",
                  "input": { "prompt": "审查", "description": "审查 Rust 后端", "run_in_background": true } }
            ]}
        }));
        t.observe(&serde_json::json!({
            "type": "user",
            "toolUseResult": { "isAsync": true, "agentId": "a21278fd478be0810" },
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "u1",
                  "content": "Async agent launched successfully." }
            ]}
        }));
        assert_eq!(t.items[0].status, "running");

        // queue-operation：通知在顶层 content，message 整个不存在
        t.observe(&serde_json::json!({
            "type": "queue-operation",
            "operation": "enqueue",
            "content": "<task-notification>\n<task-id>a21278fd478be0810</task-id>\n<status>completed</status>\n</task-notification>"
        }));
        assert_eq!(
            t.items[0].status, "completed",
            "顶层 content 的通知必须被接住"
        );
    }
}

#[cfg(test)]
mod subagent_tests {
    use super::*;

    const HOUR: u64 = 3600 * 1000;

    fn agent(id: &str, status: &str, ended_ms: u64) -> SubTask {
        new_sub_task(
            id.into(),
            "agent",
            "子会话".into(),
            status.into(),
            "2026-09-08T02:55:52.284Z".into(),
            Some("Agent \"子会话\" failed: Agent stalled".into()),
            ended_ms,
            "toolu_test_agent".into(),
        )
    }

    fn writes(id: &str, ms: u64) -> HashMap<String, u64> {
        HashMap::from([(id.to_string(), ms)])
    }

    /// 完成通知没落到父会话记录里（父进程被打断/退出）时，条目会永远停在「执行中」。
    /// 实测线上有两条从 02:55 挂到 14:49、其间后起的 16 条早已 completed。
    /// 而它自己那份记录已经收在「交回结果」的形态上 —— 那就是跑完了。
    #[test]
    fn finished_subagent_without_notification_is_closed() {
        let mut t = BgTracker::default();
        t.items.push(agent("a320f1242950d09d0", "running", 0));
        let now = 12 * HOUR;
        let out = t.reconciled(
            &writes("a320f1242950d09d0", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "completed", "交回结果了就是跑完了，别再挂着");
        assert_eq!(
            t.items[0].status, "running",
            "判定不写回状态，续跑时才翻得回来"
        );
    }

    /// 靠磁盘改判状态时，通知里那句原因就过期了 —— 它说的是「failed」，
    /// 而这里判出来的是「completed」，留着就成了自相矛盾的一张卡。
    #[test]
    fn reconcile_clears_the_stale_reason_when_it_overrides_status() {
        let mut t = BgTracker::default();
        // agent() 造出来就带一句 failed 的原因
        t.items.push(agent("a1", "failed", 0));
        let now = 12 * HOUR;
        // 重新派了一次活 → 上一轮的终态与原因都作废
        t.on_tool_result(
            &serde_json::json!({"type":"tool_result","tool_use_id":"toolu_RUN2","content":"ok"}),
            Some(&serde_json::json!({"agentId":"a1"})),
            "2026-09-13T00:00:00.000Z",
        );
        let out = t.reconciled(
            &writes("a1", now - 60_000),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "running");
        assert_eq!(out[0].summary, None, "翻回执行中就不该再挂着失败原因");

        // 父记录漏了通知、子会话自己早已交回结果 → 判 completed，旧原因同样作废
        let mut t = BgTracker::default();
        t.items.push(agent("a2", "running", 0));
        t.items[0].summary = Some("Agent \"x\" failed: 过期的原因".into());
        let out = t.reconciled(
            &writes("a2", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "completed");
        assert_eq!(out[0].summary, None);
    }

    /// 流式写入途中，最后一条常常正好是 thinking / text（assistant 按内容块拆条落盘），
    /// 形态与「已交回结果」一模一样。没静置窗口的话，正在跑的子会话会被当场判死 ——
    /// 实测就是这么把本机唯一在跑的 `a7bca81f9e85bfd33` 误判成跑完的。
    #[test]
    fn finished_shape_needs_to_settle_first() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let now = 12 * HOUR;
        // 才静了 1 分钟：可能只是下一个内容块还没落盘
        let out = t.reconciled(
            &writes("a1", now - 60_000),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "running", "回合内空窗不该判死");
    }

    /// 停在一个 tool_use 上就是在等工具返回。实测 340 份子会话记录里 12.9% 至少有一次
    /// 超过 5 分钟的合法空窗（一次慢构建 / 跑测试），按时间判会把约一成正在跑的判死 ——
    /// 那正是本次要修的症状本身。
    #[test]
    fn long_gap_but_pending_tool_use_stays_running() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let now = 12 * HOUR;
        // 空窗 30 分钟，远超原先那个 5 分钟阈值
        let out = t.reconciled(
            &writes("a1", now - 30 * 60 * 1000),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "running", "慢工具的合法空窗不该判死");
    }

    /// 半路被 kill、父会话又没写下通知：只剩时间能兜底
    #[test]
    fn abandoned_midflight_subagent_is_stopped() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let now = 12 * HOUR;
        let out = t.reconciled(
            &writes("a1", now - 3 * HOUR),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "stopped");
    }

    /// 被 kill 的子会话，最后一笔落盘比通知晚 0.0~0.1 秒（纯写入竞争）。据此判「复活」
    /// 会把 30 小时前 failed 的 `aa8f4211d424433a4` 重新点亮成执行中 —— 实测 340 个
    /// 子会话里，这个间隔的 p99 只有 0.1 秒。
    ///
    /// 注意现在**会**去读文件（终态条目也回看磁盘），但读到「停在半路」不构成翻案：
    /// 它没把结果交回去，父记录那句 `failed` 仍然是唯一说得出死因的证词。
    #[test]
    fn write_race_after_notification_is_not_a_resume() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        // 通知在 10 小时前，记录最后写入只比它晚 100 毫秒
        t.items.push(agent("a1", "failed", now - 10 * HOUR));
        let out = t.reconciled(
            &writes("a1", now - 10 * HOUR + 100),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "failed");
        assert_eq!(out[0].outcome, SubTaskOutcome::Failed);
        assert!(out[0].summary.is_some(), "死因不能被兜底逻辑抹掉");
    }

    /// **B1 的正主**：父会话被打断时给所有在跑子代理统一发 `killed`，可子会话自己那份
    /// 记录明明收在「已交回结果」的形态上 —— 旧实现有一条 early-continue，落了终态就
    /// 再也不读磁盘，于是这条永远显示失败。现在终态条目也回看一次。
    #[test]
    fn killed_but_delivered_is_corrected_to_completed() {
        let mut t = BgTracker::default();
        let now = 200 * HOUR;
        t.items.push(agent("a1", "killed", now - HOUR));
        let out = t.reconciled(
            &writes("a1", now - HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "completed");
        assert_eq!(out[0].outcome, SubTaskOutcome::Completed);
    }

    /// 真被掐断的（没交回结果）保留 `killed` 原文，但归类是「被连带终止」而不是失败 ——
    /// 前端据此分开配色，不必自己去猜 `killed` 这个词是什么意思。
    #[test]
    fn killed_without_delivery_is_interrupted_not_failed() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "killed", now - HOUR));
        let out = t.reconciled(
            &writes("a1", now - HOUR),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "killed");
        assert_eq!(out[0].outcome, SubTaskOutcome::Interrupted);
    }

    /// **B1 的另一半**：终态条目过了保留窗口就该消失。实测本机一条会话累到 147 条，
    /// 最老的是 11 天前的事，旧实现永远不淘汰。
    #[test]
    fn stale_terminal_items_are_evicted() {
        let mut t = BgTracker::default();
        let now = 300 * HOUR;
        // 209 小时前收尾（就是那 3 条 killed 的年纪）
        t.items.push(agent("old", "killed", now - 209 * HOUR));
        // 1 小时前收尾
        t.items.push(agent("fresh", "killed", now - HOUR));
        // 还在跑的：多久都不淘汰
        t.items.push(agent("live", "running", 0));
        let out = t.reconciled(&HashMap::new(), now, &|_| None, ReconcileOpts::default());
        let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["fresh", "live"]);
    }

    /// 通知缺席（`ended_ms == 0`）的条目，按磁盘纠成 completed 时要把记录最后写入时刻
    /// 补成收尾时刻 —— 否则它没有年龄，永远过不了保留窗口。
    #[test]
    fn disk_corrected_item_gets_an_end_time() {
        let mut t = BgTracker::default();
        let now = 300 * HOUR;
        // 1 小时前交回结果、父记录没写通知 → 纠成 completed，收尾时刻用记录最后写入
        let mut fresh = BgTracker::default();
        fresh.items.push(agent("a1", "running", 0));
        let out = fresh.reconciled(
            &writes("a1", now - HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "completed");
        assert_eq!(out[0].ended_ms, now - HOUR);
        // 209 小时前交回的：补上收尾时刻后当场就过了保留窗口，同一轮即消失
        t.items.push(agent("a1", "running", 0));
        assert!(t
            .reconciled(
                &writes("a1", now - 209 * HOUR),
                now,
                &|_| Some(SubAgentTail::Finished),
                ReconcileOpts::default()
            )
            .is_empty());
    }

    fn bg(id: &str, status: &str) -> SubTask {
        new_sub_task(
            id.into(),
            "bg",
            "npm run dev".into(),
            status.into(),
            "2026-09-08T02:55:52.284Z".into(),
            None,
            0,
            "toolu_test_bg".into(),
        )
    }

    /// **后台命令永远停在「执行中」**：它没有独立记录可纠，两条收尾信号都在父记录里，
    /// 父记录没写下来（父进程被打断/退出/机器重启）就永远挂着。
    ///
    /// 收尾判据是一个硬事实 —— 父会话都结束了，它派生的后台命令不可能还在跑。
    /// 不是「挂了超过 N 小时」那种时间阈值（那是在症状处打补丁）。
    #[test]
    fn running_bg_is_closed_when_parent_session_ended() {
        let now = 300 * HOUR;
        let parent_ms = now - 200 * HOUR;
        let mut t = BgTracker::default();
        t.items.push(bg("b1", "running"));
        // 父会话还活着 → 不动它，跑多久都算在跑
        let alive = t.reconciled(&HashMap::new(), now, &|_| None, ReconcileOpts::default());
        assert_eq!(alive[0].status, "running");
        assert_eq!(alive[0].outcome, SubTaskOutcome::Running);
        // 父会话已结束 → 合成终态，归「被连带终止」而不是失败，收尾时刻取父会话最后写入
        let full = t.reconciled(
            &HashMap::new(),
            now,
            &|_| None,
            ReconcileOpts {
                parent_ended_ms: Some(parent_ms),
                full: true,
                extra: Vec::new(),
            },
        );
        assert_eq!(full[0].status, ORPHANED_STATUS);
        assert_eq!(full[0].outcome, SubTaskOutcome::Interrupted);
        assert_eq!(full[0].ended_ms, parent_ms);
        // 落了终态就吃保留窗口：200 小时前的这条在热路径那份里直接消失
        let hot = t.reconciled(
            &HashMap::new(),
            now,
            &|_| None,
            ReconcileOpts {
                parent_ended_ms: Some(parent_ms),
                ..Default::default()
            },
        );
        assert!(hot.is_empty(), "收尾后应被保留窗口淘汰");
    }

    /// 子代理有磁盘记录可纠，不该被「父会话结束」这条兜底碰到
    #[test]
    fn parent_ended_does_not_touch_subagents() {
        let now = 12 * HOUR;
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let out = t.reconciled(
            &writes("a1", now - 60_000),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts {
                parent_ended_ms: Some(now - HOUR),
                ..Default::default()
            },
        );
        assert_eq!(out[0].status, "running");
    }

    /// 全量视角：父记录漏掉的子会话（阻塞式派活不写 agentId）要从目录补进来，
    /// 且不受保留窗口限制 —— 历史会话展开靠的就是它。
    #[test]
    fn full_view_adds_missing_subagents_and_keeps_old_ones() {
        let now = 300 * HOUR;
        let mut t = BgTracker::default();
        t.items.push(agent("known", "completed", now - 209 * HOUR));
        let extra = vec![new_sub_task(
            "missing".into(),
            "agent",
            "阻塞式子会话".into(),
            "running".into(),
            String::new(),
            None,
            0,
            String::new(),
        )];
        let wrote = HashMap::from([
            ("known".to_string(), now - 209 * HOUR),
            ("missing".to_string(), now - 208 * HOUR),
        ]);
        let out = t.reconciled(
            &wrote,
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts {
                full: true,
                extra,
                ..Default::default()
            },
        );
        let ids: Vec<&str> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["known", "missing"],
            "两条都要在，且不被保留窗口淘汰"
        );
        assert!(out.iter().all(|t| t.outcome == SubTaskOutcome::Completed));
        assert!(out.iter().all(|t| t.has_body));
        // 配不上起跑调用的（sidecar 里没有 toolUseId 的老数据）：字段整个不下发，
        // 不报错、也不拿 label 去凑 —— 前端据此不把它画成执行链上的智能体卡。
        let wire: Value = serde_json::from_str(&serde_json::to_string(&out[1]).unwrap()).unwrap();
        assert!(
            wire.get("toolUseId").is_none(),
            "拿不到起跑调用 id 时不该下发这个键"
        );
        assert!(wire.get("label").is_some(), "其余字段照常");
    }

    /// 一天之内也能爆量，光有时间窗兜不住：超过上限时按收尾时刻留最近的，顺序不乱。
    #[test]
    fn terminal_items_are_capped_by_count() {
        let mut t = BgTracker::default();
        let now = 100 * HOUR;
        for i in 0..(BG_MAX_ITEMS + 10) {
            t.items.push(agent(
                &format!("a{i}"),
                "completed",
                now - (60 - i as u64) * 1000,
            ));
        }
        let out = t.reconciled(&HashMap::new(), now, &|_| None, ReconcileOpts::default());
        assert_eq!(out.len(), BG_MAX_ITEMS);
        // 留下的是最近的那批，且保持原始先后
        assert_eq!(out[0].id, "a10");
        assert_eq!(out[BG_MAX_ITEMS - 1].id, format!("a{}", BG_MAX_ITEMS + 9));
    }

    /// **父记录给出的终态**之后很久还在写 ⇒ 它被重新派活了，正在跑新的一轮。
    ///
    /// 这是唯一能观测到「新一轮开始」的信号：重新派活**不写**新的 tool_result
    /// （实测 aad0fb121cab8a31d 的 toolUseResult 记录全文只有 1 条，收尾通知却有 8 条）。
    /// 那种 ended_ms 是通知里的真实时刻，「比它晚 5 分钟还在写」只能是新一轮。
    /// 同时 runs 要 +1，前端才分得清「它又跑起来了」和「刚才那次判错了」。
    #[test]
    fn notified_terminal_reopens_as_a_new_run() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "completed", now - HOUR));
        let out = t.reconciled(
            &writes("a1", now - 5_000),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "running");
        assert_eq!(out[0].runs, 2, "这是第二轮");
    }

    /// 同一个子代理跑完多次：每来一条**更晚**的收尾通知就是又跑完了一轮。
    /// 同一时刻的重复通知（queue-operation 与 user 各落一条）不重复计数。
    #[test]
    fn each_later_notification_counts_as_another_run() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let notif = |body: &str| {
            format!(
                "<task-notification><task-id>a1</task-id><status>completed</status>\
                 <summary>{body}</summary></task-notification>"
            )
        };
        t.on_notification(&notif("第一轮的报告"), "2026-09-12T17:45:02.754Z");
        assert_eq!(t.items[0].runs, 1, "第一条通知只是第一轮跑完");
        // **同一条通知落两次盘**：queue-operation 与 user 各一条，正文逐字节相同、
        // 时间戳差十几毫秒（实测 a7f78026084ce8753 就是这形态）。按时间戳去重会失效。
        let ended_after_first = t.items[0].ended_ms;
        t.on_notification(&notif("第一轮的报告"), "2026-09-12T17:45:02.764Z");
        assert_eq!(t.items[0].runs, 1, "同一条通知的第二次落盘不算新一轮");
        assert_eq!(
            t.items[0].ended_ms, ended_after_first,
            "重复落盘的同一条通知不该把收尾时刻挪十几毫秒 —— 下游会看到一次无谓的值变化"
        );
        t.on_notification(&notif("第二轮的报告"), "2026-09-12T17:59:02.100Z");
        t.on_notification(&notif("第二轮的报告"), "2026-09-12T17:59:02.119Z");
        assert_eq!(t.items[0].runs, 2);
        t.on_notification(&notif("第三轮的报告"), "2026-09-12T18:24:10.000Z");
        assert_eq!(t.items[0].runs, 3);
    }

    /// **磁盘推断出来的终态同样是吸收态** —— 这条是 `outcome 来回抖` 的正主。
    ///
    /// 父记录没写下收尾通知时，终态只能靠「尾形态已收尾 + 静置够久」推断，而判据里有个
    /// `now - 文件最后写入`。旧实现每轮现算、不记住：文件被再写一下，静置归零，当场从
    /// completed 翻回 running，`ended_ms` 还换一个新值 —— 全程没有任何新派活。
    #[test]
    fn disk_inferred_terminal_survives_a_later_write() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "running", 0));
        // 第一轮：静置够久 + 已交回结果 → 推断 completed
        let first = t.reconciled(
            &writes("a1", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(first[0].status, "completed");
        let ended = first[0].ended_ms;
        assert!(ended > 0);
        // 第二轮：文件刚被写过（静置归零）——旧实现在这里翻回 running
        let second = t.reconciled(
            &writes("a1", now - 1_000),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(second[0].status, "completed", "定案之后不因一次写入翻案");
        assert_eq!(
            second[0].ended_ms, ended,
            "收尾时刻必须稳定，不能每轮换一个"
        );
        assert_eq!(second[0].runs, 1, "没有新派活，runs 不该动");
    }

    /// 父记录亲口发话盖过磁盘那份推断：定案之后收到通知，以通知为准。
    #[test]
    fn notification_overrides_a_settled_guess() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "running", 0));
        t.reconciled(
            &writes("a1", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        // 通知晚到，说它其实是 failed
        t.on_notification(
            "<task-notification><task-id>a1</task-id><status>failed</status>\
             <summary>Agent \"x\" failed: 卡死了</summary></task-notification>",
            "2026-09-13T00:00:00.000Z",
        );
        let out = t.reconciled(
            &writes("a1", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Midflight),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "failed", "父记录说的盖过磁盘推断");
        assert!(out[0].summary.is_some(), "死因要留着");
    }

    /// 已经是终态、磁盘也说收尾了 → 保持终态（两边一致的平凡情形，别被改坏）
    #[test]
    fn terminal_and_finished_tail_stays_terminal() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "completed", now - 2 * HOUR));
        let out = t.reconciled(
            &writes("a1", now - HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "completed");
    }

    /// 两道时间判定都是**严格大于**，边界值那一刻还不算数
    #[test]
    fn thresholds_are_exclusive_at_the_boundary() {
        let now = 12 * HOUR;
        let run = |idle: u64, tail: SubAgentTail| {
            let mut t = BgTracker::default();
            t.items.push(agent("a1", "running", 0));
            t.reconciled(
                &writes("a1", now - idle),
                now,
                &|_| Some(tail),
                ReconcileOpts::default(),
            )[0]
            .status
            .clone()
        };
        // 静置窗口：正好 300s 还不收尾，多 1ms 才收
        assert_eq!(run(SUBAGENT_SETTLE_MS, SubAgentTail::Finished), "running");
        assert_eq!(
            run(SUBAGENT_SETTLE_MS + 1, SubAgentTail::Finished),
            "completed"
        );
        // 半路兜底：正好 2h 还算在跑，多 1ms 才放弃
        assert_eq!(run(SUBAGENT_ABANDON_MS, SubAgentTail::Midflight), "running");
        assert_eq!(
            run(SUBAGENT_ABANDON_MS + 1, SubAgentTail::Midflight),
            "stopped"
        );
    }

    /// 终态被换成**另一个**终态：跑砸过的条目被重新派活、这回跑完了 → completed
    /// （光测 completed→completed 覆盖不到这条路）
    #[test]
    fn redispatched_failed_item_settles_to_completed() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "failed", now - 3 * HOUR));
        // 重新派一次活（新的 tool_use_id），这才是翻案的硬证据
        t.on_tool_result(
            &serde_json::json!({"type":"tool_result","tool_use_id":"toolu_RUN2","content":"ok"}),
            Some(&serde_json::json!({"agentId":"a1"})),
            "2026-09-13T00:00:00.000Z",
        );
        // 这一轮已交回结果且静置够久
        let out = t.reconciled(
            &writes("a1", now - HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(
            out[0].status, "completed",
            "重新派活跑完了就不该还挂着 failed"
        );
        assert_eq!(out[0].runs, 2);
        assert_eq!(out[0].tool_use_id, "toolu_RUN2");
    }

    /// 后台命令（kind=bg）没有子会话记录，不参与对齐
    #[test]
    fn background_commands_are_untouched() {
        let mut t = BgTracker::default();
        let mut cmd = agent("b1", "running", 0);
        cmd.kind = "bg".into();
        t.items.push(cmd);
        let now = 12 * HOUR;
        let out = t.reconciled(
            &writes("b1", now - 8 * HOUR),
            now,
            &|_| Some(SubAgentTail::Finished),
            ReconcileOpts::default(),
        );
        assert_eq!(out[0].status, "running");
    }

    /// 没派过子会话 / 旧版 Claude Code 不建这个目录时原样返回，不改任何判定
    #[test]
    fn without_subagent_dir_nothing_changes() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let out = t.reconciled(
            &HashMap::new(),
            12 * HOUR,
            &|_| unreachable!(),
            ReconcileOpts::default(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, "running");
    }

    /// 目录扫描 + 四种收尾形态的判定。
    /// 形态取自实测：334/340 收在 assistant-无-tool_use（已完成），5 份收在 user
    /// （父记录里都是 killed/failed），1 份收在 assistant+tool_use（正在跑）。
    #[test]
    fn scans_dir_and_classifies_tails() {
        let root = std::env::temp_dir().join(format!("am-sub-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("s").join("subagents");
        fs::create_dir_all(&dir).unwrap();
        let write = |id: &str, tail: &str| {
            let head = "{\"type\":\"user\",\"timestamp\":\"2026-09-08T14:35:47.772Z\"}\n";
            fs::write(
                dir.join(format!("agent-{id}.jsonl")),
                format!("{head}{tail}"),
            )
            .unwrap();
        };
        write(
            "adone",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"结论"}]}}"#,
        );
        write(
            "apend",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x","name":"Bash"}]}}"#,
        );
        write(
            "akill",
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#,
        );
        write(
            "ahalf",
            r#"{"type":"assistant","message":{"content":[{"type":"te"#,
        );
        // 不是子会话记录的文件不该被收进来
        fs::write(dir.join("agent-adone.meta.json"), "{}").unwrap();

        let d = SubAgentDir::scan(&root.join("s.jsonl"));
        let mut ids: Vec<&str> = d.last_write.keys().map(String::as_str).collect();
        ids.sort();
        assert_eq!(ids, ["adone", "ahalf", "akill", "apend"]);
        assert!(d.newest_ms() > 0);
        assert_eq!(d.tail("adone"), Some(SubAgentTail::Finished));
        assert_eq!(
            d.tail("apend"),
            Some(SubAgentTail::Midflight),
            "等工具返回 = 还在跑"
        );
        assert_eq!(
            d.tail("akill"),
            Some(SubAgentTail::Midflight),
            "停在工具结果 = 还在半路"
        );
        assert_eq!(
            d.tail("ahalf"),
            Some(SubAgentTail::Midflight),
            "半截行 = 正在写"
        );
        assert_eq!(d.tail("anope"), None, "没有这份记录就别猜");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod replay_state_tests {
    use super::*;
    use std::io::Write;

    fn line(v: Value) -> String {
        format!("{}\n", serde_json::to_string(&v).unwrap())
    }

    fn create(id: &str, subject: &str) -> String {
        line(serde_json::json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "tool_use", "id": id, "name": "TaskCreate",
                  "input": { "subject": subject } }
            ]}
        }))
    }

    fn created(use_id: &str, n: u32, subject: &str) -> String {
        line(serde_json::json!({
            "type": "user",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": use_id,
                  "content": format!("Task #{n} created successfully: {subject}") }
            ]}
        }))
    }

    fn chatter(n: usize) -> String {
        // 每行塞一段填充，确保总量能越过 8MB 的尾部窗口
        let pad = "填充".repeat(60);
        (0..n)
            .map(|i| {
                line(serde_json::json!({
                    "type": "assistant",
                    "timestamp": "2026-07-17T10:00:00Z",
                    "message": { "content": [{ "type": "text", "text": format!("闲聊 {i} {pad}") }] }
                }))
            })
            .collect()
    }

    /// 清单必须从会话开头重放：TaskCreate 常常远在尾部窗口之外
    /// （真实会话可达数十 MB，早期的 TaskCreate 一条都读不到）。
    #[test]
    fn replays_todos_created_far_before_the_tail_window() {
        let dir = std::env::temp_dir().join(format!("am-replay-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        // 开头建任务，随后堆入远超尾部窗口的内容
        f.write_all(create("u1", "甲").as_bytes()).unwrap();
        f.write_all(created("u1", 1, "甲").as_bytes()).unwrap();
        f.write_all(chatter(60_000).as_bytes()).unwrap();
        f.flush().unwrap();
        assert!(
            fs::metadata(&path).unwrap().len() > 8 * 1024 * 1024,
            "样本需大于尾部窗口才有意义"
        );

        let mut sc = SessionScanner::new(dir.clone());
        let (todos, _) = sc.replay_state(&path, false, false).unwrap();
        let todos = todos.expect("尾部窗口读不到的 TaskCreate 也必须被重放到");
        assert!(todos.contains("\"id\":\"1\""));
        assert!(todos.contains("甲"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// 增量重放：文件追加后只解析新增字节，且状态在多次调用间累积
    #[test]
    fn replay_is_incremental_and_accumulates() {
        let dir = std::env::temp_dir().join(format!("am-inc-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        fs::write(&path, create("u1", "甲") + &created("u1", 1, "甲")).unwrap();

        let mut sc = SessionScanner::new(dir.clone());
        let (todos, _) = sc.replay_state(&path, false, false).unwrap();
        assert!(todos.unwrap().contains("甲"));
        let after_first = sc.state_cache.get(&path).unwrap().offset;
        assert!(after_first > 0);

        // 追加第二个任务，只该解析新增部分
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all((create("u2", "乙") + &created("u2", 2, "乙")).as_bytes())
            .unwrap();
        f.flush().unwrap();

        let (todos, _) = sc.replay_state(&path, false, false).unwrap();
        let todos = todos.unwrap();
        assert!(todos.contains("甲"), "旧状态应保留");
        assert!(todos.contains("乙"), "新增应被解析");
        assert!(sc.state_cache.get(&path).unwrap().offset > after_first);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 半截的末行不能消费，否则下次续读会从行中间开始，整行永久丢失
    #[test]
    fn partial_trailing_line_is_not_consumed() {
        let dir = std::env::temp_dir().join(format!("am-partial-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        // 完整两行 + 半行（模拟正在写入）
        let half = create("u2", "乙");
        let half = &half[..half.len() / 2];
        fs::write(&path, create("u1", "甲") + &created("u1", 1, "甲") + half).unwrap();

        let mut sc = SessionScanner::new(dir.clone());
        let (todos, _) = sc.replay_state(&path, false, false).unwrap();
        assert!(todos.unwrap().contains("甲"));
        let off = sc.state_cache.get(&path).unwrap().offset;

        // 补全那半行
        let rest = create("u2", "乙");
        let rest = &rest[rest.len() / 2..];
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(rest.as_bytes()).unwrap();
        f.write_all(created("u2", 2, "乙").as_bytes()).unwrap();
        f.flush().unwrap();

        let (todos, _) = sc.replay_state(&path, false, false).unwrap();
        assert!(
            todos.unwrap().contains("乙"),
            "补全后该行必须被完整解析（偏移没有停在行中间）"
        );
        assert!(sc.state_cache.get(&path).unwrap().offset > off);

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod noise_filter_tests {
    use super::*;

    fn write_session(dir: &Path, id: &str, lines: &[String]) -> PathBuf {
        let proj = dir.join("-tmp-p");
        fs::create_dir_all(&proj).unwrap();
        let p = proj.join(format!("{id}.jsonl"));
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    fn row(ty: &str, content: Value, cwd: &str) -> String {
        serde_json::json!({
            "type": ty, "cwd": cwd, "sessionId": "s", "isSidechain": false,
            "message": { "role": ty, "content": content },
            "timestamp": "2026-09-13T00:00:00.000Z"
        })
        .to_string()
    }

    /// **只在斜杠命令里打过转的会话不产出**：`/clear`、`/model` 这些被记成 type=user，
    /// 正文是 `<command-name>…</command-name>` 信封，剥完什么都不剩。
    /// 实测本机 105 条里有 11 条是这种空壳，侧栏里就是一串一模一样的空白行。
    #[test]
    fn command_only_session_is_not_reported() {
        let tmp = std::env::temp_dir().join(format!("am-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let home = "/Users/u/proj";
        let p = write_session(
            &tmp,
            "11111111-1111-1111-1111-111111111111",
            &[
                row(
                    "user",
                    Value::String("<command-name>/clear</command-name>".into()),
                    home,
                ),
                row(
                    "user",
                    Value::String("<command-name>/model</command-name>".into()),
                    home,
                ),
            ],
        );
        let mut sc = SessionScanner::new(tmp.clone());
        let meta = fs::metadata(&p).unwrap();
        let sum = sc
            .summarize(&p, meta.len(), now_ms())
            .expect("解析层不再拦空壳 —— 它认不出这条空壳是死的还是刚 /clear 出来的");
        assert!(sum.is_empty_shell(), "全是命令信封 → 空壳");
        // 没有任何进程占着它 → 产出任务时丢掉，历史列表里不会多出一行空白
        let tasks = build_tasks(
            &[sum],
            &[],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(tasks.is_empty(), "没进程占着的空壳不该出现在任务列表里");
        let _ = fs::remove_dir_all(&tmp);
    }

    /// **别误伤**：没有可解析的用户文本、但助手回过话的会话要保留
    /// （只发了图片、或只有工具调用时标题就是空的）。判据是「用户输入**或**助手回复」。
    #[test]
    fn session_with_assistant_output_is_kept_even_without_user_text() {
        let tmp = std::env::temp_dir().join(format!("am-img-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let home = "/Users/u/proj";
        let p = write_session(
            &tmp,
            "22222222-2222-2222-2222-222222222222",
            &[
                // 只发了一张图片：user_text 取不出文本
                row(
                    "user",
                    serde_json::json!([{ "type": "image", "source": {} }]),
                    home,
                ),
                row(
                    "assistant",
                    serde_json::json!([{ "type": "text", "text": "看到了" }]),
                    home,
                ),
            ],
        );
        let mut sc = SessionScanner::new(tmp.clone());
        let meta = fs::metadata(&p).unwrap();
        assert!(
            sc.summarize(&p, meta.len(), now_ms()).is_some(),
            "有助手回复就说明这条会话有内容，不能丢"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    /// **一次性目录里的会话不算项目**：判据是「操作系统说这块地方是临时的」，
    /// 不是枚举 `claude-501` / `am-verify-wt` 这类具体名字（那是字面量匹配）。
    #[test]
    fn throwaway_cwd_is_recognised_by_system_temp_root() {
        let scratch = std::env::temp_dir().join("am-scratch/xyz");
        assert!(is_throwaway_cwd(&scratch.to_string_lossy()));
        if cfg!(unix) {
            assert!(is_throwaway_cwd("/tmp/claude-501/whatever/scratchpad"));
            // 名字里带 tmp 但不在临时根下的真项目不能误伤
            assert!(!is_throwaway_cwd("/Users/u/tmpproj"));
            assert!(!is_throwaway_cwd("/Users/u/Desktop/Program/tmp/real"));
        }
        assert!(!is_throwaway_cwd(""), "拿不到 cwd 时不做判断");
    }

    /// 列表里那个「有没有子代理」的数：只 readdir 数文件名，不打开任何文件。
    #[test]
    fn sub_agent_count_only_counts_agent_records() {
        let tmp = std::env::temp_dir().join(format!("am-cnt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let proj = tmp.join("-p");
        let sess = proj.join("sess");
        fs::create_dir_all(sess.join("subagents")).unwrap();
        let jsonl = proj.join("sess.jsonl");
        fs::write(&jsonl, "").unwrap();
        // 目录存在但空 → 0
        assert_eq!(count_sub_agents(&jsonl), 0);
        for name in [
            "agent-a1.jsonl",
            "agent-a2.jsonl",
            // 这些都不该算：sidecar、别的前缀、别的后缀
            "agent-a1.meta.json",
            "tool-results.jsonl",
            "agent-a3.txt",
        ] {
            fs::write(sess.join("subagents").join(name), "x").unwrap();
        }
        assert_eq!(count_sub_agents(&jsonl), 2, "只数 agent-*.jsonl");

        // 目录整个不存在（绝大多数会话）→ 0，不是报错
        let none = proj.join("other.jsonl");
        fs::write(&none, "").unwrap();
        assert_eq!(count_sub_agents(&none), 0);
        let _ = fs::remove_dir_all(&tmp);
    }

    /// 同一个会话号在两个项目目录下各有一份时，产出侧只留一条（留 mtime 大的）。
    /// 此前不去重，重复与否全靠运气，只是网页那头用 Map 压着才没发作。
    #[test]
    fn duplicate_session_ids_collapse_to_one() {
        let mk = |id: &str, mtime: u64| {
            let mut s = SessionSummary {
                provider: "claude".into(),
                desktop: false,
                session_id: id.into(),
                project_key: "k".into(),
                cwd: "/p".into(),
                live_cwd: String::new(),
                shell_cwd: String::new(),
                title: String::new(),
                prompt: String::new(),
                last_action: String::new(),
                turn_ended: true,
                cleared: false,
                clear_born: false,
                has_content: true,
                sub_agent_count: 0,
                started_at: None,
                last_active_at: None,
                version: None,
                git_branch: None,
                mtime_ms: mtime,
                created_ms: 0,
                line_count: 0,
                used_tokens_5h: 0,
                queued_inputs: Vec::new(),
                select_answered_ms: None,
            };
            s.title = format!("t{mtime}");
            s
        };
        // 入参按 mtime 倒序（与 scan 里一致）
        let out = dedup_by_session_id(vec![mk("a", 200), mk("a", 100), mk("b", 50)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "t200", "同号冲突留 mtime 大的那份");
    }
}

#[cfg(test)]
mod brief_tests {
    use super::*;

    #[test]
    fn ask_user_question_becomes_select() {
        // 真实终端 Claude Code 写出的 AskUserQuestion 记录结构
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "type":"assistant",
              "isSidechain":false,
              "message":{"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"AskUserQuestion","caller":"x",
                 "input":{"questions":[{"question":"选哪个?","options":[{"label":"A"},{"label":"B"}]}]}}
              ]},
              "timestamp":"2026-07-28T08:00:00.000Z"
            }"#,
        )
        .unwrap();
        let b = entry_to_brief(&v).expect("应产出一条简报");
        assert_eq!(b.role, "select", "AskUserQuestion 必须解析成 select 角色");
        assert!(b.content.contains("questions"), "select 内容应含 questions");
    }

    /// 一条 assistant 记录里的多次 `tool_use` 必须拆成多个元素、各带自己的 id。
    ///
    /// 旧实现把它们 `format!("{name}: {hint}")` 后 `" | "` 拼成一个字符串 —— 拼完就
    /// 再也认不出哪一段对应哪一次调用，执行链上那次派子代理的调用便无法与它派出的
    /// [`SubTask`] 对应（两边唯一的交集是展示名，截断长度还不一样：120 vs 80）。
    ///
    /// **本机 67 份会话记录里一条这样的样本都没有**（上游目前一条记录只写一次
    /// `tool_use`），所以这是个没被触发过的隐患而不是现行 bug —— 正因为触发不到，
    /// 更要用构造样本把行为钉住，别等上游哪天改了批量下发才发现链全乱了。
    #[test]
    fn multiple_tool_uses_in_one_record_stay_separate() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "type":"assistant",
              "message":{"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_A","name":"Read","input":{"file_path":"/a.rs"}},
                {"type":"tool_use","id":"toolu_B","name":"Bash","input":{"command":"cargo test"}},
                {"type":"tool_use","id":"toolu_C","name":"Agent","input":{"description":"查一下根因"}}
              ]},
              "timestamp":"2026-09-12T17:05:07.934Z"
            }"#,
        )
        .unwrap();
        let m = entry_to_brief(&v).expect("应产出一条 tool 简报");
        assert_eq!(m.role, "tool");
        assert_eq!(m.tools.len(), 3, "三次调用必须是三个元素");
        let ids: Vec<&str> = m.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["toolu_A", "toolu_B", "toolu_C"], "各带自己的 id");
        assert_eq!(m.tools[2].hint, "查一下根因");
        assert!(
            m.content.is_empty(),
            "正文在 tools 里，content 不再另存一份"
        );
        // 纯文本出口（钉钉推送 / MCP 摘要）的能力不能丢
        assert_eq!(
            m.text(),
            "Read: /a.rs | Bash: cargo test | Agent: 查一下根因"
        );
    }

    /// 子会话记录里**每一条**都是 `isSidechain: true`。读父会话时要跳过它们
    /// （否则一次派活变成几十条噪音），读子会话记录本身时跳完就一条不剩 ——
    /// 实测 `agent-a9e86999bc2536847.jsonl` 172 行按父会话口径解析出 0 条。
    #[test]
    fn sidechain_entry_is_readable_when_reading_the_subagent_itself() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "type":"user",
              "isSidechain":true,
              "agentId":"a9e86999bc2536847",
              "message":{"role":"user","content":"去查一下这个"},
              "timestamp":"2026-09-12T17:05:07.934Z"
            }"#,
        )
        .unwrap();
        assert!(entry_to_brief(&v).is_none(), "父会话流里不该出现子会话记录");
        let b = parse_entry(&v, false).expect("读子会话正文时必须解析得出");
        assert_eq!(b.role, "user");
        assert_eq!(b.content, "去查一下这个");
    }
}

/// 「正等你选」的了结信号：远端的选项卡靠它撤下（PostToolUse hook 缺席时的唯一依据）
#[cfg(test)]
mod select_close_tests {
    use super::*;

    fn ask(id: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"role":"assistant","content":[
               {{"type":"tool_use","id":"{id}","name":"AskUserQuestion",
                 "input":{{"questions":[{{"question":"选哪个?"}}]}}}}]}}}}"#
        )
        .replace('\n', "")
    }

    fn answer(id: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{ts}","message":{{"role":"user","content":[
               {{"type":"tool_result","tool_use_id":"{id}","content":"选了 A"}}]}}}}"#
        )
        .replace('\n', "")
    }

    fn parse(lines: &[String]) -> Option<u64> {
        let tail = lines.join("\n");
        parse_tail("s1", std::path::Path::new("/proj/-proj/s1.jsonl"), &tail)
            .expect("应能解析")
            .select_answered_ms
    }

    /// 答完 → 记下 tool_result 的时刻，客户端据此撤卡
    #[test]
    fn answered_records_result_time() {
        let t = parse(&[
            ask("toolu_1", "2026-08-14T08:00:00.000Z"),
            answer("toolu_1", "2026-08-14T08:01:00.000Z"),
        ]);
        assert_eq!(t, iso_to_ms("2026-08-14T08:01:00.000Z"));
    }

    /// 还没答 → None。**这条最要紧**：判错就是把一张正等着人回答的卡片提前撤掉。
    #[test]
    fn unanswered_stays_open() {
        assert_eq!(parse(&[ask("toolu_1", "2026-08-14T08:00:00.000Z")]), None);
    }

    /// 等待期间 claude 并行跑的别的工具，其结果不能算作这张卡的答案
    #[test]
    fn other_tool_result_does_not_close_it() {
        let other = answer("toolu_OTHER", "2026-08-14T08:00:30.000Z");
        assert_eq!(
            parse(&[ask("toolu_1", "2026-08-14T08:00:00.000Z"), other]),
            None
        );
    }

    /// 按 Esc 打断：永远等不到 tool_result，同样要撤卡
    #[test]
    fn interrupt_closes_it() {
        let esc = r#"{"type":"user","timestamp":"2026-08-14T08:02:00.000Z","message":{"role":"user","content":"[Request interrupted by user]"}}"#;
        let t = parse(&[ask("toolu_1", "2026-08-14T08:00:00.000Z"), esc.to_string()]);
        assert_eq!(t, iso_to_ms("2026-08-14T08:02:00.000Z"));
    }

    /// 答完又弹一张新的：了结时刻要清掉，否则新卡一出生就被判成「早答过了」
    #[test]
    fn new_card_resets() {
        let t = parse(&[
            ask("toolu_1", "2026-08-14T08:00:00.000Z"),
            answer("toolu_1", "2026-08-14T08:01:00.000Z"),
            ask("toolu_2", "2026-08-14T08:02:00.000Z"),
        ]);
        assert_eq!(t, None, "新卡未答，不该带着上一张的了结时刻");
    }
}

#[cfg(test)]
mod pairing_tests {
    use super::*;

    fn sess(id: &str, started: &str, mtime_ms: u64) -> SessionSummary {
        SessionSummary {
            provider: "claude".into(),
            desktop: false,
            session_id: id.into(),
            project_key: "-proj".into(),
            cwd: "/proj".into(),
            live_cwd: "/proj".into(),
            shell_cwd: "/proj".into(),
            title: id.into(),
            prompt: String::new(),
            last_action: String::new(),
            turn_ended: true,
            cleared: false,
            clear_born: false,
            has_content: true,
            sub_agent_count: 0,
            started_at: Some(started.into()),
            last_active_at: None,
            version: None,
            git_branch: None,
            mtime_ms,
            created_ms: 0,
            line_count: 1,
            used_tokens_5h: 0,
            queued_inputs: Vec::new(),
            select_answered_ms: None,
        }
    }

    fn proc(pid: u32, start_time: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            agent: "claude".into(),
            tty: format!("/dev/ttys00{pid}"),
            cwd: "/proj".into(),
            ide: crate::model::IdeKind::Terminal,
            ide_name: "Terminal".into(),
            start_time,
            cpu_usage: 0.0,
            memory: 0,
            command: "claude".into(),
            shell_pid: None,
            shell_start: None,
            shared_host: false,
        }
    }

    /// parse_tail 应把「只含 /clear 命令、未输入」的新会话标记 cleared=true、prompt 空。
    /// 用真实 /clear 会话的行结构（mode/snapshot/caveat/command-name/local_command）验证。
    #[test]
    fn parse_tail_flags_cleared_session() {
        let tail = concat!(
            r#"{"type":"mode","mode":"normal","sessionId":"s1"}"#,
            "\n",
            r#"{"type":"file-history-snapshot","messageId":"m1"}"#,
            "\n",
            r#"{"type":"user","cwd":"/proj","message":{"role":"user","content":"<local-command-caveat>Caveat: local command</local-command-caveat>"}}"#,
            "\n",
            r#"{"type":"user","cwd":"/proj","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-args></command-args>"}}"#,
            "\n",
            r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout></local-command-stdout>"}"#,
            "\n",
        );
        let s = parse_tail("s1", std::path::Path::new("/x/-proj/s1.jsonl"), tail).unwrap();
        assert!(s.cleared, "只含 /clear 的新会话应标记 cleared");
        assert!(s.clear_born, "见过 /clear 命令块 → 生于 /clear");
        assert!(s.prompt.is_empty(), "cleared 会话不该有真实 prompt");
        assert!(s.turn_ended, "cleared 会话视为回合结束（Idle）");
    }

    /// Claude Code 注入的系统内容一律不得冒充「用户发的话」。
    ///
    /// 这些都被记成 type=user，只能靠正文特征认出来。名单漏一种，对话流里就会
    /// 突然冒出一大段不是自己写的东西 —— compact 的续接摘要（几百行英文）尤其扎眼。
    #[test]
    fn user_text_rejects_all_injected_kinds() {
        for injected in [
            "<local-command-caveat>x</local-command-caveat>",
            "<command-name>/clear</command-name>",
            "<system-reminder>别忘了</system-reminder>",
            "<task-notification>后台任务完成</task-notification>",
            "Caveat: local command",
            "[Request interrupted by user]",
            "This session is being continued from a previous conversation that ran out of context.\n\nSummary:\n1. Primary Request...",
        ] {
            let v = serde_json::json!(injected);
            assert!(
                user_text(Some(&v)).is_none(),
                "注入内容不该被当成用户输入: {}",
                &injected[..injected.len().min(40)]
            );
        }
        // 反例：真实输入照常通过，别把人家正常打的字也滤掉
        let real = serde_json::json!("帮我看看这个 session 的问题");
        assert_eq!(
            user_text(Some(&real)).as_deref(),
            Some("帮我看看这个 session 的问题")
        );
    }

    /// 在终端按 Esc 中断后，会话应判为「回合已结束」（Idle），而不是卡在执行中。
    ///
    /// 中断记录被 user_text 当系统内容滤掉（对的，它不是用户发言），若不显式收口，
    /// turn_ended 会保持中断前的值 —— 那时最后一条是 assistant 带 tool_use，即 false，
    /// 于是界面一直显示「执行中」，而终端早已在等你输入。
    #[test]
    fn parse_tail_interrupt_ends_turn() {
        let base = concat!(
            r#"{"type":"user","cwd":"/proj","message":{"role":"user","content":"跑一下测试"}}"#,
            "\n",
            r#"{"type":"assistant","cwd":"/proj","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}}"#,
            "\n",
        );
        // 中断前：正在调用工具 → 未结束
        let s = parse_tail("s1", std::path::Path::new("/x/-proj/s1.jsonl"), base).unwrap();
        assert!(!s.turn_ended, "工具执行中，回合不该算结束");

        // 两种中断文案都要认（纯字符串 / text 数组两种记法）
        for marker in [
            r#""[Request interrupted by user]""#,
            r#"[{"type":"text","text":"[Request interrupted by user for tool use]"}]"#,
        ] {
            let tail = format!(
                "{base}{}\n",
                serde_json::json!({
                    "type": "user",
                    "cwd": "/proj",
                    "message": {
                        "role": "user",
                        "content": serde_json::from_str::<Value>(marker).unwrap(),
                    },
                }),
            );
            let s = parse_tail("s1", std::path::Path::new("/x/-proj/s1.jsonl"), &tail).unwrap();
            assert!(s.turn_ended, "中断后应判回合结束: {marker}");
            // 中断标记不是用户输入，不能顶掉原来的 prompt
            assert_eq!(s.prompt, "跑一下测试", "中断标记不该被当成新输入: {marker}");
        }
    }

    /// 反例：清空后又输入了真实内容 → 不再是空会话，cleared=false。
    #[test]
    fn parse_tail_cleared_reset_after_real_input() {
        let tail = concat!(
            r#"{"type":"user","cwd":"/proj","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            "\n",
            r#"{"type":"user","cwd":"/proj","message":{"role":"user","content":"hello world"}}"#,
            "\n",
        );
        let s = parse_tail("s2", std::path::Path::new("/x/-proj/s2.jsonl"), tail).unwrap();
        assert!(!s.cleared, "清空后有真实输入 → 不再 cleared");
        assert!(
            s.clear_born,
            "「生于 /clear」是既成事实，不该随用户输入消失（继任关系建在它上面）"
        );
        assert_eq!(s.prompt, "hello world");
    }

    /// /clear 回归：进程 P 在启动时创建了旧会话 old（created≈start），运行中执行 /clear，
    /// claude 另起一个「只含 /clear、未输入」的新会话 fresh（cleared=true，created=现在）。
    /// P 实际已转到 fresh，但 tier② 会按「created 最早」把 P 粘回 old → 网页定格清空前内容。
    /// clear-follow 据缓存（上一轮 P 配的是 old）把 P 迁到 fresh，old 落 Finished。
    #[test]
    fn clear_follow_moves_process_to_fresh_session() {
        let now = now_ms();
        let start_s = now / 1000 - 3600; // 进程 1h 前启动
        let mut old = sess("old", "2026-07-23T00:00:00Z", now - 1000);
        old.created_ms = start_s * 1000 + 1000; // 旧会话创建≈进程启动
        old.cleared = false;
        let mut fresh = sess("fresh", "2026-07-23T09:00:00Z", now - 500);
        fresh.title = String::new();
        fresh.prompt = String::new();
        fresh.created_ms = now - 2000; // 刚 /clear 出来
        fresh.cleared = true;
        fresh.clear_born = true; // 生于 /clear（继任关系据此算，与用户有没有输入无关）
        let p = proc(200, start_s);
        // 缓存：上一轮 P 配在 old
        let mut cached = HashMap::new();
        cached.insert(200u32, "old".to_string());

        let tasks = build_tasks(
            &[old, fresh],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let f = tasks.iter().find(|t| t.id == "fresh").unwrap();
        let o = tasks.iter().find(|t| t.id == "old").unwrap();
        assert_eq!(f.pid, Some(200), "clear-follow 应把进程迁到刚清空的新会话");
        assert_eq!(o.pid, None, "被清空取代的旧会话不该再占着进程");
        assert_eq!(o.status, TaskStatus::Finished);
    }

    /// 闪烁回归：/clear 迁移后，下一轮缓存已指向新会话，进程必须**继续粘在新会话**，
    /// 不能被 tier② 按「created 最早」又拽回旧会话——否则 旧↔新 每轮抖动 = 卡片闪烁。
    /// 模拟第二轮：cached={200: fresh}，fresh 仍 cleared（用户还没输入）。
    #[test]
    fn clear_follow_stable_no_flicker_next_round() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut old = sess("old", "2026-07-23T00:00:00Z", now - 3000);
        old.created_ms = start_s * 1000 + 1000; // 旧会话 created 最早（tier② 会想抢它）
        old.cleared = false;
        let mut fresh = sess("fresh", "2026-07-23T09:00:00Z", now - 500);
        fresh.title = String::new();
        fresh.prompt = String::new();
        fresh.created_ms = now - 2000;
        fresh.cleared = true; // 用户还没输入，仍是空会话
        fresh.clear_born = true;
        let p = proc(200, start_s);
        let mut cached = HashMap::new();
        cached.insert(200u32, "fresh".to_string()); // 上一轮已迁到 fresh
        let tasks = build_tasks(
            &[old, fresh],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let f = tasks.iter().find(|t| t.id == "fresh").unwrap();
        let o = tasks.iter().find(|t| t.id == "old").unwrap();
        assert_eq!(f.pid, Some(200), "进程应继续粘在新会话（不回抖）");
        assert_eq!(o.pid, None, "旧会话不该被 tier② 又抢回进程");
    }

    /// `/clear` 的继任关系必须**作为数据**出现在 Task 上：新会话 `supersedes` 指向旧会话。
    /// 这是「网页停在清空前的旧会话」那个 bug 的修法 —— 此前这层关系只有扫描器自己知道，
    /// 算完配完进程就扔了，消费方只能拿 pid 在两帧之间的转移去猜。
    #[test]
    fn supersedes_names_the_cleared_predecessor() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut old = sess("old", "2026-07-23T00:00:00Z", now - 1000); // /clear 只顶了一下 mtime
        old.created_ms = start_s * 1000 + 1000;
        let mut fresh = sess("fresh", "2026-07-23T09:00:00Z", now - 500);
        fresh.title = String::new();
        fresh.prompt = String::new();
        fresh.created_ms = now - 1000; // 与旧会话最后一次写入同一瞬间
        fresh.cleared = true;
        fresh.clear_born = true;
        let p = proc(200, start_s);
        let mut cached = HashMap::new();
        cached.insert(200u32, "old".to_string());

        let tasks = build_tasks(
            &[old, fresh],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let f = tasks.iter().find(|t| t.id == "fresh").unwrap();
        let o = tasks.iter().find(|t| t.id == "old").unwrap();
        assert_eq!(
            f.supersedes.as_deref(),
            Some("old"),
            "新会话必须说得出它接替了谁"
        );
        assert_eq!(o.supersedes, None, "旧会话没有前任");
    }

    /// 继任关系**不依赖进程配到了谁**：hook 自报（pinned）会直接把 pid 指到新会话，
    /// 从而整层跳过 clear-follow 的迁移；缓存也可能压根没有旧会话那条。若靠配对结果回推，
    /// 同一件事就会时有时无 —— 而它必须每一轮都说得出口。
    #[test]
    fn supersedes_holds_when_hook_pins_pid_to_the_new_session() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut old = sess("old", "2026-07-23T00:00:00Z", now - 1000);
        old.created_ms = start_s * 1000 + 1000;
        let mut fresh = sess("fresh", "2026-07-23T09:00:00Z", now - 500);
        fresh.title = String::new();
        fresh.prompt = String::new();
        fresh.created_ms = now - 1000;
        fresh.cleared = true;
        fresh.clear_born = true;
        let p = proc(200, start_s);
        // hook 自报：pid 已经指在新会话上，clear-follow 迁移层无事可做
        let mut pinned = HashMap::new();
        pinned.insert(200u32, "fresh".to_string());

        let tasks = build_tasks(
            &[old, fresh],
            &[p],
            &|_| false,
            &pinned,
            &HashSet::new(),
            &HashMap::new(), // 缓存为空：继任关系不该受它影响
        );
        let f = tasks.iter().find(|t| t.id == "fresh").unwrap();
        assert_eq!(f.pid, Some(200));
        assert_eq!(
            f.supersedes.as_deref(),
            Some("old"),
            "配对走的是哪条路，与继任关系无关"
        );
    }

    /// 用户在清空后已经输入了内容（`cleared` 翻回 false）——继任关系是既成事实，
    /// 不该跟着消失。这正是不能把它建在 `cleared` 上的原因：那等于给它按了个几秒的保质期。
    #[test]
    fn supersedes_survives_after_user_types_in_the_new_session() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut old = sess("old", "2026-07-23T00:00:00Z", now - 60_000);
        old.created_ms = start_s * 1000 + 1000;
        let mut fresh = sess("fresh", "2026-07-23T09:00:00Z", now - 500);
        fresh.created_ms = now - 60_000; // 一分钟前 /clear 出来的
        fresh.prompt = "接着干".into(); // 已经输入过 → cleared=false
        fresh.cleared = false;
        fresh.clear_born = true;
        let p = proc(200, start_s);
        let mut cached = HashMap::new();
        cached.insert(200u32, "fresh".to_string());

        let tasks = build_tasks(
            &[old, fresh],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let f = tasks.iter().find(|t| t.id == "fresh").unwrap();
        assert_eq!(f.supersedes.as_deref(), Some("old"));
    }

    /// 没发生过 /clear 的普通会话：继任指针指向**它顶掉的那条占位任务**（`pid-<pid>`），
    /// 而不是隔壁那条同项目的老会话 —— 别把「同一个项目里还有别的会话」当成继任。
    #[test]
    fn supersedes_points_at_placeholder_when_no_clear_happened() {
        let now = now_ms();
        let start_s = now / 1000 - 60;
        let mut a = sess("a", "2026-07-23T00:00:00Z", now - 1000);
        a.created_ms = start_s * 1000 + 1000;
        let p = proc(201, start_s);
        let tasks = build_tasks(
            &[a],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        let t = tasks.iter().find(|t| t.id == "a").unwrap();
        assert_eq!(t.supersedes.as_deref(), Some("pid-201"));
    }

    /// 刚 `/clear` 出来、还一个字都没输入的新会话，**从真实文件内容**解析出来长什么样。
    /// 不手工摆 flag：这条链的第一环就是「这份 jsonl 里只有一个 /clear 信封」，
    /// 手摆等于把要验的东西当前提。
    fn shell_from_real_clear_jsonl(id: &str, created_ms: u64, mtime_ms: u64) -> SessionSummary {
        let tail = format!(
            "{}\n{}\n",
            serde_json::json!({
                "type": "user", "cwd": "/proj", "isSidechain": false,
                "message": { "role": "user", "content":
                    "<command-name>/clear</command-name>\n<command-message>clear</command-message>" },
                "timestamp": "2026-07-23T09:00:00.000Z"
            }),
            serde_json::json!({ "type": "system", "cwd": "/proj", "isMeta": false }),
        );
        let mut s = parse_tail(id, std::path::Path::new("/x/-proj/x.jsonl"), &tail).unwrap();
        assert!(s.is_empty_shell(), "只有 /clear 信封 → 空壳");
        assert!(s.clear_born, "它生于一次 /clear");
        s.created_ms = created_ms;
        s.mtime_ms = mtime_ms;
        s
    }

    /// **没装 hook 的机器上，`/clear` 之后进程必须跟到新会话，而且新任务得真的发得出去。**
    ///
    /// 这是本轮修的那条断链的回归用例，四种起手式各验一遍 —— 它们在真机上都发生过：
    /// ① 刚清空、还没输入（旧路径在这里就把新会话整个丢了，后面全线落空）；
    /// ② 用户已经敲了字（旧路径里 `cleared` 已翻回 false，迁移层再不肯动）；
    /// ③ 客户端刚重启、配对缓存是空的（没人封住旧会话，tier② 立刻把进程粘回去）；
    /// ④ 累积 pin 表里还留着一条指向旧会话的陈旧 pin（它是最高优先级，直接压过一切）。
    ///
    /// 四种情况的验收是同一句话：进程在新会话上、新任务不是 Finished（否则 hub 那层
    /// `status != Finished` 的过滤会把它拦下，前端根本收不到）、且它说得出接替了谁。
    #[test]
    fn clear_follow_without_hook_moves_process_to_the_successor() {
        let now = now_ms();
        let start_s = now / 1000 - 3600; // 进程一小时前起的
        let clear_at = now - 2000; // 两秒前 /clear

        struct Case {
            name: &'static str,
            /// 新会话里用户是否已经敲过字（敲过 → `cleared` 已翻回 false）
            typed: bool,
            /// 上一轮的配对缓存指向哪条会话（None = 客户端刚重启，缓存是空的）
            cached: Option<&'static str>,
            /// 累积 pin 表里那条陈旧记录指向哪条会话（None = 表里没有）
            pinned: Option<&'static str>,
        }
        let cases = [
            Case {
                name: "刚清空、还没输入",
                typed: false,
                cached: Some("old"),
                pinned: None,
            },
            Case {
                name: "用户已经敲了字",
                typed: true,
                cached: Some("new"),
                pinned: None,
            },
            Case {
                name: "客户端刚重启、缓存为空",
                typed: true,
                cached: None,
                pinned: None,
            },
            // 长命子进程（`npm start` 之类）env 里带的是它被拉起那一刻的会话号
            Case {
                name: "累积 pin 表里留着旧会话",
                typed: true,
                cached: None,
                pinned: Some("old"),
            },
        ];

        for Case {
            name,
            typed,
            cached,
            pinned,
        } in cases
        {
            let one = |sid: Option<&str>| -> HashMap<u32, String> {
                sid.into_iter().map(|s| (200u32, s.to_string())).collect()
            };
            let (cached, pinned) = (one(cached), one(pinned));
            let mut old = sess("old", "2026-07-23T00:00:00Z", clear_at);
            old.created_ms = start_s * 1000 + 1000;
            // `/clear` 只往旧文件顶了一下 mtime，此后再没人碰它 → 时间戳冻在清空那一刻
            let mut new = shell_from_real_clear_jsonl("new", clear_at, clear_at);
            new.project_key = old.project_key.clone();
            new.cwd = old.cwd.clone();
            if typed {
                // 用户敲了一句 → parse_tail 会把 cleared 翻回 false（见
                // `parse_tail_cleared_reset_after_real_input`），clear_born 则原样留着。
                new.prompt = "接着干".into();
                new.has_content = true;
                new.turn_ended = false;
                new.cleared = false;
                new.mtime_ms = now - 500;
            }

            let tasks = build_tasks(
                &[old, new],
                &[proc(200, start_s)],
                &|_| false,
                &pinned,
                &HashSet::new(),
                &cached,
            );
            let n = tasks
                .iter()
                .find(|t| t.id == "new")
                .unwrap_or_else(|| panic!("[{name}] 继任任务必须在列表里"));
            assert_eq!(n.pid, Some(200), "[{name}] 进程该跟到新会话");
            assert_ne!(
                n.status_dsr,
                TaskStatus::Finished.dsr(),
                "[{name}] Finished 会被 hub 的活跃列表过滤掉，前端收不到"
            );
            assert_eq!(
                n.supersedes.as_deref(),
                Some("old"),
                "[{name}] 新任务要说得出它接替了谁"
            );
            let o = tasks.iter().find(|t| t.id == "old").unwrap();
            assert_eq!(o.pid, None, "[{name}] 旧会话已经死了，不许再顶着进程");
        }
    }

    /// **一条 `clear_born` 会话压根没有前任时，别把隔壁正在跑的会话拉来顶包。**
    ///
    /// 场景：用户新开一个终端，第一句就敲 `/clear` —— 这条新会话没有前任。而同项目里
    /// 另一个终端正干着活，它的 mtime 一直在往前走。认错前任的代价是致命的：那条活会话
    /// 会被判成「已被接替」，随即被全部配对层封锁，用户眼看着自己跑着的会话变 Finished，
    /// 进程还被搬去了隔壁那个空会话。
    ///
    /// 这里把兄弟会话的最后写入摆在 18 秒前 —— 原先 60 秒的容差正好把它圈进去。
    #[test]
    fn a_running_sibling_is_not_mistaken_for_a_predecessor() {
        let now = now_ms();
        let busy_s = now / 1000 - 3600; // 干活的那个终端，一小时前起的
        let fresh_s = now / 1000 - 3; // 刚开的新终端
        let mut live = sess("live", "2026-07-23T00:00:00Z", now - 18_000);
        live.created_ms = busy_s * 1000 + 1000;
        // 新终端起手第一句就 /clear：没有前任
        let shell = shell_from_real_clear_jsonl("shell-01", now - 2000, now - 2000);
        let mut cached = HashMap::new();
        cached.insert(200u32, "live".to_string());

        let tasks = build_tasks(
            &[live, shell],
            &[proc(200, busy_s), proc(201, fresh_s)],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let l = tasks.iter().find(|t| t.id == "live").expect("活会话得在");
        assert_eq!(l.pid, Some(200), "正在跑的会话不许被误判成前任而丢掉进程");
        assert_ne!(l.status_dsr, TaskStatus::Finished.dsr(), "更不许变成已结束");
        let s = tasks.iter().find(|t| t.id == "shell-01").unwrap();
        assert_eq!(s.pid, Some(201), "空会话归它自己那个新终端");
        assert_eq!(s.supersedes.as_deref(), Some("pid-201"), "它没有前任");
    }

    /// **空壳会话不许被 tier③（`--continue`）/ tier④（mtime 兜底）挑走。**
    ///
    /// 空壳留在 `sessions` 里是为了给「刚 /clear 出来的活会话」一个落点，它该归谁有明确
    /// 依据（pin / clear-follow / tier② 自建 / `--resume` 点名）。③④ 是纯 mtime 启发式，
    /// 拿不到那个依据 —— 同项目另开一个终端就可能凭「mtime 最新」把它挑走，用户得到一个
    /// 没标题、也不属于他那个终端的格子。这个口子是「不再在解析层丢空壳」新开的。
    #[test]
    fn empty_shell_is_not_grabbed_by_mtime_heuristics() {
        let now = now_ms();
        let busy_s = now / 1000 - 3600;
        let mut live = sess("live", "2026-07-23T00:00:00Z", now - 30_000);
        live.created_ms = busy_s * 1000 + 1000;
        // 一分钟前留下、刚刚又被顶过 mtime 的空壳，它那个终端早没了
        let shell = shell_from_real_clear_jsonl("shell-01", now - 60_000, now - 2000);
        let mut cont = proc(202, now / 1000); // 刚起、带 --continue → 走 tier③
        cont.command = "claude --continue".into();
        let idle = proc(203, now / 1000 - 30); // 无命令行线索、创建窗口够不着 → 走 tier④

        let tasks = build_tasks(
            &[live, shell],
            &[proc(200, busy_s), cont, idle],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(
            !tasks.iter().any(|t| t.id == "shell-01"),
            "没有明确依据的进程不该认领空壳，认了它就会作为任务冒出来"
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "live").unwrap().pid,
            Some(200),
            "干活的那条不受影响"
        );
        for pid in [202u32, 203] {
            assert!(
                tasks.iter().any(|t| t.id == format!("pid-{pid}")),
                "够不着任何会话的进程该留成占位任务，而不是去抢空壳"
            );
        }
    }

    /// 连着 `/clear` 两次：进程得一路跟到最后那条，不许被中间那条（它自己也被接替了）粘住。
    #[test]
    fn clear_follow_chains_through_a_second_clear() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut a = sess("a", "2026-07-23T00:00:00Z", now - 10_000);
        a.created_ms = start_s * 1000 + 1000;
        let b = shell_from_real_clear_jsonl("b", now - 10_000, now - 2000);
        let c = shell_from_real_clear_jsonl("c", now - 2000, now - 2000);
        // 上一轮进程配在 b 上（第一次 /clear 之后），这一轮用户又清了一次
        let mut cached = HashMap::new();
        cached.insert(200u32, "b".to_string());

        let tasks = build_tasks(
            &[a, b, c],
            &[proc(200, start_s)],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        let t = tasks.iter().find(|t| t.id == "c").expect("最后那条得在");
        assert_eq!(t.pid, Some(200), "进程该跟到最后一条");
        assert_eq!(t.supersedes.as_deref(), Some("b"));
        assert!(
            tasks.iter().all(|t| t.id == "c" || t.pid.is_none()),
            "中间那条和最初那条都已经死了"
        );
    }

    /// `--resume` 把一条早被 `/clear` 甩掉的老会话捞回来接着用 —— 命令行是用户亲口说的，
    /// 压过「被接替 ⇒ 已死」这条由时间戳推出来的判断，否则这条会话再也配不上进程。
    #[test]
    fn explicit_resume_revives_a_superseded_session() {
        let now = now_ms();
        let start_s = now / 1000 - 10;
        let mut old = sess("old-sess", "2026-07-23T00:00:00Z", now - 3_600_000);
        old.created_ms = now - 7_200_000;
        // 一小时前那次 /clear 留下的空壳，至今没人碰过
        let shell = shell_from_real_clear_jsonl("shell-01", now - 3_600_000, now - 3_600_000);
        let mut p = proc(300, start_s);
        p.command = "claude --resume old-sess".into();

        let tasks = build_tasks(
            &[old, shell],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        let t = tasks.iter().find(|t| t.id == "old-sess").unwrap();
        assert_eq!(t.pid, Some(300), "--resume 指名要它，就得配上");
        assert!(
            !tasks.iter().any(|t| t.id == "shell-01"),
            "那条没人占的空壳不该冒出来"
        );
    }

    /// clear-follow 不误伤：没有 /clear（无 cleared 会话）时，配对行为与既有一致。
    /// 两个进程各自的旧会话都不该因缓存被搬走。
    #[test]
    fn clear_follow_noop_without_cleared_session() {
        let now = now_ms();
        let start_s = now / 1000 - 3600;
        let mut a = sess("a", "2026-07-23T00:00:00Z", now - 1000);
        a.created_ms = start_s * 1000 + 1000;
        let mut b = sess("b", "2026-07-23T00:10:00Z", now - 800);
        b.created_ms = start_s * 1000 + 2000;
        let pa = proc(201, start_s);
        let pb = proc(202, start_s + 1);
        let mut cached = HashMap::new();
        cached.insert(201u32, "a".to_string());
        cached.insert(202u32, "b".to_string());
        let tasks = build_tasks(
            &[a, b],
            &[pa, pb],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &cached,
        );
        // 两条会话各自保住自己的进程，没有互串
        assert!(tasks.iter().any(|t| t.id == "a" && t.pid.is_some()));
        assert!(tasks.iter().any(|t| t.id == "b" && t.pid.is_some()));
    }

    /// 真实场景（Windows/Cursor）：空白新会话创建于进程启动那刻（created_ms≈start），
    /// 但没写内容 → mtime 旧、无 started_at；而旧的有内容会话 mtime 反而更新。
    /// 必须按 created_ms 把进程配给空白会话，旧会话落 Finished —— 不能被 mtime 抢走。
    #[test]
    fn pairs_by_created_time_not_mtime() {
        let now = now_ms();
        let start_s = now / 1000 - 60; // 进程 60s 前启动（秒）
        let mut blank = sess("blank", "2026-07-20T00:00:00Z", now - 55_000);
        blank.created_ms = start_s * 1000 + 2000; // 创建≈进程启动
        blank.started_at = None; // 空白会话没有首条用户消息
        let mut old = sess("old", "2026-07-19T00:00:00Z", now - 1_000); // mtime 更新
        old.created_ms = now - 6 * 3600 * 1000; // 6 小时前创建
        let mut p = proc(200, start_s);
        p.command = "claude".into();

        let tasks = build_tasks(
            &[blank, old],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        let b = tasks.iter().find(|t| t.id == "blank").unwrap();
        let o = tasks.iter().find(|t| t.id == "old").unwrap();
        assert_eq!(b.pid, Some(200), "进程应配给创建时刻≈启动的空白会话");
        assert_eq!(o.pid, None, "旧会话不该抢到进程（尽管 mtime 更新）");
        assert_eq!(o.status, TaskStatus::Finished);
    }

    /// 命令行 --resume <id>：恢复的会话 created_ms/started_at 都很旧，只能靠命令行认出。
    #[test]
    fn resume_command_pairs_old_session() {
        assert_eq!(
            resume_session_id("claude --resume abc12345-ef"),
            Some("abc12345-ef")
        );
        assert_eq!(
            resume_session_id("node x/claude.js -r sess-9999"),
            Some("sess-9999")
        );
        assert_eq!(resume_session_id("claude --continue"), None);

        let now = now_ms();
        let mut resumed = sess("resumed-xyz", "2026-06-01T00:00:00Z", now - 2_000);
        resumed.created_ms = now - 20 * 24 * 3600 * 1000; // 20 天前创建
        let mut p = proc(300, now / 1000 - 30);
        p.command = "claude --resume resumed-xyz".into();

        let tasks = build_tasks(
            &[resumed],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "resumed-xyz").unwrap().pid,
            Some(300)
        );
    }

    /// 「按打开文件配对」优先于 mtime：已关闭的会话 mtime 更新，但活进程占着的是
    /// 另一个 mtime 更旧的会话（如 Cursor 里闲置的会话）。pinned 指定后应把进程配给
    /// 它真正打开的会话，已关闭的落 Finished —— 而非被 mtime 抢走。
    #[test]
    fn pinned_overrides_mtime_pairing() {
        let now = now_ms();
        // 已关闭：mtime 更新（刚关不久）
        let closed = sess("closed", "2026-07-20T22:00:00Z", now - 60_000);
        // 活着但闲置：mtime 更旧
        let cursor = sess("cursor", "2026-07-20T21:00:00Z", now - 600_000);
        let p = proc(100, now);
        let mut pinned = HashMap::new();
        pinned.insert(100u32, "cursor".to_string()); // 进程真正打开的是 cursor 会话

        let tasks = build_tasks(
            &[closed, cursor],
            &[p],
            &|_| false,
            &pinned,
            &HashSet::new(),
            &HashMap::new(),
        );
        let cur = tasks.iter().find(|t| t.id == "cursor").unwrap();
        let clo = tasks.iter().find(|t| t.id == "closed").unwrap();
        assert_eq!(cur.pid, Some(100), "活进程应配给它打开的 cursor 会话");
        assert_ne!(cur.status, TaskStatus::Finished);
        assert_eq!(clo.pid, None, "已关闭会话不该抢到进程");
        assert_eq!(clo.status, TaskStatus::Finished);
    }

    /// Windows：sysinfo 上报的进程 cwd 带尾随反斜杠（D:\proj\），会话目录名却是
    /// 无尾随的 D--proj。encode_path 必须先去尾随分隔符，两者才能配成对，
    /// 否则会话永远沦为「等待输入」的占位进程、内容不同步。
    #[test]
    fn windows_trailing_backslash_cwd_still_pairs() {
        assert_eq!(encode_path("D:\\proj\\"), encode_path("D:\\proj"));
        // 期望值随平台大小写策略走：Windows 统一小写（d--proj），其它平台保持原样（D--proj）
        assert_eq!(
            encode_path("D:\\proj\\"),
            normalize_key_case("D--proj".to_string())
        );

        let now = now_ms();
        let mut s = sess("live", "2026-07-20T00:00:00Z", now - 30_000);
        // 会话目录名由 encode_path(cwd) 而来，与生产一致（含平台大小写同一化）
        s.project_key = encode_path("D:\\proj");
        s.cwd = "D:\\proj".into();
        s.created_ms = now - 30_000; // 会话在进程启动时创建
        let mut p = proc(4242, now / 1000 - 30); // 进程 30s 前启动
        p.cwd = "D:\\proj\\".into(); // 进程 cwd 带尾随反斜杠
        p.tty = String::new();

        let tasks = build_tasks(
            &[s],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        // 配对成功 = 恰好一条任务、带 pid、状态非 Finished（不是占位进程）
        assert_eq!(tasks.len(), 1, "应配成一条，而非会话+占位进程两条");
        assert_eq!(tasks[0].pid, Some(4242));
        assert_ne!(tasks[0].status, TaskStatus::Finished);
        assert!(!tasks[0].prompt.contains("尚未产生记录"), "不该是占位进程");
    }

    /// 续跑/压缩恢复的长会话：文件很久前创建、进程比它晚启动、命令行也无 --continue
    /// （Claude Code 自动续跑正是如此），但一直在写、mtime 很新 → 必须配上（phase ④）；
    /// 同时 8h 没动的旧会话不该被抢。
    #[test]
    fn recently_active_long_session_pairs_via_mtime() {
        let now = now_ms();
        let mut cont = sess("cont", "2026-07-16T00:00:00Z", now - 60_000); // 1min 前还在写
        cont.created_ms = now - 20 * 3600 * 1000; // 20h 前创建（远早于进程）
        let mut old = sess("old", "2026-07-15T00:00:00Z", now - 8 * 3600 * 1000);
        old.created_ms = now - 30 * 3600 * 1000;
        let mut p = proc(700, now / 1000 - 3600); // 进程 1h 前启动、无 --continue
        p.command = "claude".into();

        let tasks = build_tasks(
            &[cont, old],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "cont").unwrap().pid,
            Some(700),
            "续跑的近活会话应配上"
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "old").unwrap().pid,
            None,
            "8h 没动的旧会话不该被抢"
        );
    }

    /// 回归：某进程 --resume 了老会话、写了几句后退出；用户又在同目录开一个【全新空白】
    /// 终端（无 --resume/--continue、还没产生自己的会话）。那条老会话虽然 mtime 还很新
    /// （5min 前刚写），但它的最后写入发生在新进程【启动之前】—— 新空白进程绝不能凭 mtime
    /// 新就把它抢来一直显示旧内容，应留占位（pid-<pid>）。
    #[test]
    fn fresh_blank_process_does_not_grab_recently_closed_resumed_session() {
        let now = now_ms();
        // 老会话：5min 前最后写入（在 30min 窗口内、mtime 很新），2h 前创建
        let mut resumed = sess("resumed", "2026-07-17T00:00:00Z", now - 5 * 60_000);
        resumed.created_ms = now - 2 * 3600 * 1000;
        // 新空白进程：1min 前才启动（晚于老会话最后写入），命令行无 --resume/--continue
        let mut p = proc(902, now / 1000 - 60);
        p.command = "claude".into();

        let tasks = build_tasks(
            &[resumed],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "resumed").unwrap().pid,
            None,
            "启动前就停笔的老会话不该被新空白进程抢去"
        );
        assert!(
            tasks.iter().any(|t| t.id == "pid-902"),
            "新空白进程应留占位任务，而非顶着旧会话内容"
        );
    }

    /// 回归：会话闲置很久（几小时没输入）但进程仍活着，且该会话的最后写入晚于进程启动
    /// （确是本进程写的）。此时不该因「mtime 不在最近 30min 内」就把它判 Finished、让进程
    /// 冒一条空白占位。widen ④ 后应继续配上。（created_ms=0 模拟无出生时间、②配不上的平台）
    #[test]
    fn long_idle_session_still_pairs_not_finished() {
        let now = now_ms();
        // 进程 5h 前启动、无 --resume/--continue
        let mut p = proc(1500, now / 1000 - 5 * 3600);
        p.command = "claude".into();
        // 会话：3h 没写了（闲置），但最后写入（now-3h）晚于进程启动（now-5h）；出生时间不可得
        let mut idle = sess("idle", "2026-07-25T00:00:00Z", now - 3 * 3600 * 1000);
        idle.created_ms = 0;

        let tasks = build_tasks(
            &[idle],
            &[p],
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(
            tasks.iter().find(|t| t.id == "idle").unwrap().pid,
            Some(1500),
            "闲置数小时但进程仍在的会话应保持配对，而非被判关闭"
        );
        assert!(
            !tasks.iter().any(|t| t.id == "pid-1500"),
            "不该再冒出空白占位终端会话"
        );
    }

    /// `claude --continue` 恢复最近改动的会话：该会话创建于很久前（不是本进程建的），
    /// 只能靠命令行 --continue 认出，配给剩余里 mtime 最近的会话（alive）；另一个 4 小时
    /// 没动静的老会话配不到、落 Finished。
    #[test]
    fn continue_command_pairs_most_recent_session() {
        let now = now_ms();
        // 活着：刚刚还在写（mtime 最新）
        let alive = sess("alive", "2026-06-26T02:29:18Z", now - 60_000);
        // 已死：4 小时没动静了
        let dead = sess("dead", "2026-07-16T16:16:19Z", now - 4 * 3600 * 1000);
        let sessions = vec![alive, dead];
        // 进程用 --continue 恢复最近会话
        let mut p = proc(3191, now / 1000 - 100);
        p.command = "claude --continue".into();
        let procs = vec![p];

        let tasks = build_tasks(
            &sessions,
            &procs,
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        let by_id = |id: &str| tasks.iter().find(|t| t.id == id).unwrap().clone();

        assert_eq!(by_id("alive").pid, Some(3191), "正在写入的会话必须拿到进程");
        assert_eq!(by_id("dead").pid, None, "四小时没动静的会话不该顶着进程");
        assert_eq!(
            by_id("dead").status,
            TaskStatus::Finished,
            "配不到进程即已结束（列表会过滤掉）"
        );
    }

    /// 多进程时：各进程配「自己启动时创建」的会话（created≈start），多余的老会话
    /// （创建于任何进程启动之前）落到 Finished。
    #[test]
    fn pairs_by_created_close_to_start() {
        let now = now_ms();
        let start_a = now / 1000 - 100; // 进程 A 100s 前启动
        let start_b = now / 1000 - 50; // 进程 B 50s 前启动
        let mut newest = sess("newest", "2026-07-01T00:00:00Z", now - 10_000);
        newest.created_ms = start_b * 1000 + 1000; // ≈ 进程 B 启动
        let mut middle = sess("middle", "2026-07-02T00:00:00Z", now - 20_000);
        middle.created_ms = start_a * 1000 + 1000; // ≈ 进程 A 启动
        let mut stale = sess("stale", "2026-07-03T00:00:00Z", now - 3 * 3600 * 1000);
        stale.created_ms = now - 6 * 3600 * 1000; // 远早于任何进程启动
        let sessions = vec![newest, middle, stale];
        let procs = vec![proc(100, start_a), proc(200, start_b)];

        let tasks = build_tasks(
            &sessions,
            &procs,
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        let pid = |id: &str| tasks.iter().find(|t| t.id == id).unwrap().pid;

        assert_eq!(pid("newest"), Some(200), "进程配自己启动时创建的会话");
        assert_eq!(pid("middle"), Some(100));
        assert_eq!(pid("stale"), None, "创建于进程启动之前的旧会话不该拿到进程");
    }
}

#[cfg(test)]
mod user_text_tests {
    use super::*;

    fn user_entry(text: &str) -> Value {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-17T10:00:00Z",
            "message": { "content": text }
        })
    }

    /// 后台任务跑完时 Claude Code 会往会话里塞一条 type=user 的回执。
    /// 不滤掉的话，用户会在对话流里看到自己「发」了一段 XML。
    #[test]
    fn task_notification_is_not_a_user_message() {
        let v = user_entry(
            "<task-notification>\n<task-id>abc123</task-id>\n<status>completed</status>\n<summary>Background command \"Start dev server\" completed</summary>\n</task-notification>",
        );
        assert!(entry_to_brief(&v).is_none(), "系统回执不该冒充用户消息");
    }

    /// 其余系统注入块同样不该出现在对话流里
    #[test]
    fn other_injected_blocks_are_filtered() {
        for t in [
            "<system-reminder>别忘了 X</system-reminder>",
            "<local-command-stdout>输出</local-command-stdout>",
            "<command-name>/goal</command-name>",
            "Caveat: The messages below were generated…",
            "[Request interrupted by user]",
        ] {
            assert!(entry_to_brief(&user_entry(t)).is_none(), "应被滤掉: {t}");
        }
    }

    /// 别误伤真正的用户消息
    #[test]
    fn real_user_message_survives() {
        let m =
            entry_to_brief(&user_entry("把服务启动，然后打开前台")).expect("真实用户消息必须保留");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "把服务启动，然后打开前台");
    }
}

/// `tool_result` 的成败标记（`is_error`）能不能原样带到前端。
///
/// 这几条盯的是**下发形态**而不只是字段值：为真时键要在、为假与缺失时
/// 整个键都不出现（见 `MessageBrief::is_error` 的说明）。只断言字段值的话，
/// 哪天 `skip_serializing_if` 被摘掉，两万多条 `isError: false` 会悄悄
/// 涌进每次轮询，测试却照样绿。
#[cfg(test)]
mod tool_result_error_tests {
    use super::*;

    /// 真实形态：`type=user` 的记录里挂一个 `tool_result` 块
    fn result_entry(err: Option<bool>) -> Value {
        let mut item = serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01",
            "content": "npm ERR! code ELIFECYCLE"
        });
        if let Some(e) = err {
            item["is_error"] = Value::Bool(e);
        }
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-09-09T10:00:00Z",
            "message": { "content": [item] }
        })
    }

    fn wire(v: &Value) -> Value {
        serde_json::to_value(entry_to_brief(v).expect("这条应当进对话流")).unwrap()
    }

    #[test]
    fn failed_step_carries_is_error() {
        let m = entry_to_brief(&result_entry(Some(true))).expect("tool_result 应当进对话流");
        assert_eq!(m.role, "tool_result");
        assert!(m.is_error, "记录里写着 is_error=true，这一步就是跑砸的");
        assert_eq!(
            wire(&result_entry(Some(true)))["isError"],
            Value::Bool(true),
            "为真时必须下发 isError，否则前端标不出失败的那一步"
        );
    }

    /// 为假：字段值是 false，且**键整个不出现** —— 少数派才占带宽
    #[test]
    fn successful_step_omits_the_key() {
        let m = entry_to_brief(&result_entry(Some(false))).expect("tool_result 应当进对话流");
        assert!(!m.is_error);
        assert!(
            wire(&result_entry(Some(false))).get("isError").is_none(),
            "为假时不该下发这个键"
        );
    }

    /// 缺字段（老记录 / 别的形态）：行为与改前一致 —— 不出错、不出键
    #[test]
    fn missing_field_behaves_as_before() {
        let m = entry_to_brief(&result_entry(None)).expect("tool_result 应当进对话流");
        assert!(!m.is_error);
        assert_eq!(m.content, "npm ERR! code ELIFECYCLE");
        assert!(wire(&result_entry(None)).get("isError").is_none());
    }

    /// 别的 role 一律不带这个键：它只描述「一次工具调用的收场」
    #[test]
    fn other_roles_never_carry_it() {
        let v = serde_json::json!({
            "type": "user",
            "timestamp": "2026-09-09T10:00:00Z",
            "message": { "content": "把服务启动" }
        });
        let w = wire(&v);
        assert_eq!(w["role"], "user");
        assert!(w.get("isError").is_none());
    }
}

#[cfg(test)]
mod codex_tests {
    use super::*;

    fn line(payload: Value) -> Value {
        serde_json::json!({
            "timestamp": "2026-07-17T07:23:17.458Z",
            "type": "response_item",
            "payload": payload
        })
    }

    /// 用户消息：真实输入进流；<permissions> 等注入块滤掉（与真实文件格式一致）
    #[test]
    fn user_text_filters_injected_blocks() {
        let real = line(serde_json::json!({
            "type": "message", "role": "user",
            "content": [{ "type": "input_text", "text": "帮我修这个 bug" }]
        }));
        let m = codex_entry_to_brief(&real).expect("真实输入应进流");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "帮我修这个 bug");

        let injected = line(serde_json::json!({
            "type": "message", "role": "user",
            "content": [{ "type": "input_text", "text": "<permissions instructions>\nFilesystem..." }]
        }));
        assert!(
            codex_entry_to_brief(&injected).is_none(),
            "注入块不该冒充用户消息"
        );

        let dev = line(serde_json::json!({
            "type": "message", "role": "developer",
            "content": [{ "type": "input_text", "text": "system prompt" }]
        }));
        assert!(codex_entry_to_brief(&dev).is_none(), "developer 角色不进流");
    }

    #[test]
    fn assistant_and_tools_map_to_feed_roles() {
        let a = line(serde_json::json!({
            "type": "message", "role": "assistant",
            "content": [{ "type": "output_text", "text": "改好了" }]
        }));
        assert_eq!(codex_entry_to_brief(&a).unwrap().role, "assistant");

        let f = line(serde_json::json!({
            "type": "function_call", "name": "spawn_agent", "call_id": "call_7",
            "arguments": "{\"task\":\"x\"}"
        }));
        let m = codex_entry_to_brief(&f).unwrap();
        assert_eq!(m.role, "tool");
        // 结构化：一次调用一个元素、带自己的 id；纯文本出口走 text()
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].id, "call_7");
        assert_eq!(m.tools[0].name, "spawn_agent");
        assert!(m.text().starts_with("spawn_agent"));

        let o = line(serde_json::json!({
            "type": "custom_tool_call_output", "call_id": "call_7", "output": "done"
        }));
        let ob = codex_entry_to_brief(&o).unwrap();
        assert_eq!(ob.role, "tool_result");
        // 结果认得回它对应的那次调用
        assert_eq!(ob.tool_use_id, "call_7");

        let r = line(serde_json::json!({ "type": "reasoning", "summary": [] }));
        assert!(codex_entry_to_brief(&r).is_none(), "思考过程不进流");
    }

    /// 多 provider 配对：codex 会话配 codex 进程；claude 会话不会被 codex 进程抢走；
    /// 没有会话解析器的代理（gemini）以进程任务出现且可控。
    #[test]
    fn multi_provider_pairing_and_process_tasks() {
        let now = now_ms();
        let mk = |provider: &str, id: &str, key: &str| SessionSummary {
            provider: provider.into(),
            desktop: false,
            session_id: id.into(),
            project_key: key.into(),
            cwd: format!("/w/{key}"),
            live_cwd: format!("/w/{key}"),
            shell_cwd: format!("/w/{key}"),
            title: id.into(),
            prompt: String::new(),
            last_action: String::new(),
            turn_ended: true,
            cleared: false,
            clear_born: false,
            has_content: true,
            sub_agent_count: 0,
            started_at: None,
            last_active_at: None,
            version: None,
            git_branch: None,
            mtime_ms: now - 5_000,
            created_ms: now - 60_000,
            line_count: 1,
            used_tokens_5h: 0,
            queued_inputs: Vec::new(),
            select_answered_ms: None,
        };
        let proc = |agent: &str, pid: u32, key: &str| ProcessInfo {
            pid,
            agent: agent.into(),
            tty: format!("/dev/ttys{pid}"),
            cwd: format!("/w/{key}"),
            ide: crate::model::IdeKind::Terminal,
            ide_name: "Terminal".into(),
            start_time: now_ms() / 1000 - 60,
            cpu_usage: 0.0,
            memory: 0,
            command: agent.into(),
            shell_pid: None,
            shell_start: None,
            shared_host: false,
        };
        let sessions = vec![mk("claude", "c1", "-w-app"), mk("codex", "x1", "-w-app")];
        let procs = vec![
            proc("claude", 11, "app"),
            proc("codex", 22, "app"),
            proc("gemini", 33, "app"),
        ];

        let tasks = build_tasks(
            &sessions,
            &procs,
            &|_| false,
            &HashMap::new(),
            &HashSet::from(["c1".to_string(), "x1".to_string()]),
            &HashMap::new(),
        );
        let by = |id: &str| tasks.iter().find(|t| t.id == id).unwrap();

        assert_eq!(by("c1").pid, Some(11), "claude 会话配 claude 进程");
        assert_eq!(by("c1").provider, "claude");
        assert_eq!(by("x1").pid, Some(22), "codex 会话配 codex 进程");
        assert_eq!(by("x1").provider, "codex");

        let g = by("pid-33");
        assert_eq!(g.provider, "gemini", "无解析器代理以进程任务出现");
        assert_eq!(g.pid, Some(33), "pid 在 → 暂停/中断等信号控制可用");
        assert_eq!(g.provider_dsr, "Gemini CLI");
        assert_eq!(g.title, "Gemini CLI", "占位标题只显示模型名");

        // 回归：每个未配对进程必须恰好一条任务 —— 历史上 pid-/proc- 两个
        // 循环并存，同一进程会重复出现两次（用户看到 5 会话变 14 条）
        let dup: Vec<_> = tasks.iter().filter(|t| t.pid == Some(33)).collect();
        assert_eq!(dup.len(), 1, "未配对进程只能生成一条任务，不得重复");
        assert!(
            !tasks.iter().any(|t| t.id.starts_with("proc-")),
            "占位任务统一 pid- 前缀（attach_machine 只给 pid- 加机器前缀）"
        );
    }

    /// 标题开头的路径只留文件名 —— 列表里只看得到头 20 来字，目录前缀会把额度吃光。
    /// 用例取自真实的会话列表（几条并排全是 `./tmp/…`，一个需求字都露不出来）。
    #[test]
    fn title_keeps_filename_drops_leading_dirs() {
        use super::shorten_leading_paths as f;
        // 典型：先甩路径、再说需求 —— 省下的字数正好留给需求
        assert_eq!(
            f("./tmp/图片.jpg 根据图片里的需求进行修改"),
            "图片.jpg 根据图片里的需求进行修改"
        );
        // 多个文件连着甩，也一并缩短
        assert_eq!(
            f("./tmp/a.md ./tmp/b.md 对比这两个"),
            "a.md b.md 对比这两个"
        );
        // 整条消息就是一个路径：留下文件名，别剥成空
        assert_eq!(
            f("./tmp/监控IP分页-用户类型多选-前端交接"),
            "监控IP分页-用户类型多选-前端交接"
        );
        // Windows 分隔符同样处理
        assert_eq!(f("tmp\\图片.jpg 改一下"), "图片.jpg 改一下");
        // 正文中间的路径是有意引用，原样保留
        assert_eq!(f("看看 src/main.rs 里的实现"), "看看 src/main.rs 里的实现");
        // 裸文件名本来就短，不动
        assert_eq!(f("a.md 这个文件"), "a.md 这个文件");
        // 普通话原样
        assert_eq!(f("提交修改"), "提交修改");
        // 以分隔符结尾时 basename 为空，保持原样而不是把整段吞掉
        assert_eq!(f("./tmp/ 看看这个目录"), "./tmp/ 看看这个目录");
    }

    /// cwd 取不到目录名时，标题只显示 provider，不留悬空的「 · 」
    #[test]
    fn placeholder_title_without_dangling_separator() {
        let procs = vec![ProcessInfo {
            pid: 77,
            agent: "claude".into(),
            tty: "/dev/ttys077".into(),
            cwd: String::new(),
            ide: crate::model::IdeKind::Terminal,
            ide_name: "Terminal".into(),
            start_time: 1000,
            cpu_usage: 0.0,
            memory: 0,
            command: "claude".into(),
            shell_pid: None,
            shell_start: None,
            shared_host: false,
        }];
        let tasks = build_tasks(
            &[],
            &procs,
            &|_| false,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Claude Code", "空目录名不该带「 · 」尾巴");
        assert!(!tasks[0].title.contains('·'));
    }
}

#[cfg(test)]
mod live_cwd_tests {
    use super::*;
    use std::io::Write;

    fn line(cwd: &str, text: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "cwd": cwd,
                "timestamp": "2026-08-18T03:25:00Z",
                "message": { "content": [{ "type": "text", "text": text }] }
            })
        )
    }

    /// 核心回归：会话 `cd` 进子目录后，`cwd` 仍是归一化的项目根（配对/分组靠它稳定），
    /// 而 `live_cwd` 跟到最新的那个目录。
    ///
    /// 线上就栽在这个差上：网页拿项目根当上传落点、又回填相对路径 `./tmp/x.png`，
    /// 终端按自己当前的目录解析 —— 文件写进了项目根的 tmp，终端在子目录的 tmp 里找，
    /// 报「文件不存在」，而文件明明好好躺在盘上。
    #[test]
    fn live_cwd_follows_drift_while_cwd_stays_canonical() {
        let root = "/tmp/amlive/proj";
        let deep = "/tmp/amlive/proj/desktop/src-tauri";
        let dir = std::env::temp_dir()
            .join(format!("am-live-{}", std::process::id()))
            .join(encode_path(root));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        // 先在项目根，后 cd 进子目录 —— 最后一条才是终端此刻所在
        f.write_all(line(root, "在项目根").as_bytes()).unwrap();
        f.write_all(line("/tmp/amlive/proj/desktop", "cd 了一层").as_bytes())
            .unwrap();
        f.write_all(line(deep, "又深了一层").as_bytes()).unwrap();
        f.flush().unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mut sc = SessionScanner::new(dir.parent().unwrap().to_path_buf());
        let sum = sc.summarize(&path, meta.len(), 0).expect("应能解析出摘要");

        assert_eq!(sum.cwd, root, "项目根必须归一化，配对/分组靠它");
        assert_eq!(sum.live_cwd, deep, "live_cwd 必须跟到最新的工作目录");
    }

    /// 锚定到 git 仓库根：会话 `cd` 进仓库里的子目录后，`live_cwd` 收到仓库根（稳定，
    /// 不随每一轮 cd 跳），`shell_cwd` 保留真实所在（前端据此决定要不要改用绝对路径）。
    ///
    /// 0.11.50 就是栽在这一步没做：直接拿最后那个 cwd 当上传落点，落点跟着会话在
    /// `desktop`、`desktop/src-tauri`、`desktop/web/dist` 之间乱跳，连构建产物目录都能落进去。
    #[test]
    fn live_cwd_anchors_to_git_root() {
        let base = std::env::temp_dir().join(format!("am-git-{}", std::process::id()));
        let repo = base.join("repo");
        let deep = repo.join("desktop/src-tauri");
        let _ = fs::create_dir_all(&deep);
        // 仓库标记：`.git` 是目录还是文件都算（worktree 里是文件）
        let _ = fs::create_dir_all(repo.join(".git"));

        let repo_s = repo.to_string_lossy().to_string();
        let deep_s = deep.to_string_lossy().to_string();
        let dir = base.join("projects").join(encode_path(&repo_s));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(line(&repo_s, "在仓库根").as_bytes()).unwrap();
        f.write_all(line(&deep_s, "cd 进子目录").as_bytes())
            .unwrap();
        f.flush().unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mut sc = SessionScanner::new(dir.parent().unwrap().to_path_buf());
        let sum = sc.summarize(&path, meta.len(), 0).expect("应能解析出摘要");

        assert_eq!(sum.live_cwd, repo_s, "锚定目录必须收到 git 仓库根");
        assert_eq!(
            sum.shell_cwd, deep_s,
            "shell_cwd 保留真实所在，供前端判断漂移"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 不在任何 git 仓库里：没有仓库根可收，退回 shell_cwd 本身，不能凭空往上跳。
    #[test]
    fn live_cwd_falls_back_when_not_in_repo() {
        let root = "/tmp/amlive3/proj";
        let deep = "/tmp/amlive3/proj/sub";
        let dir = std::env::temp_dir()
            .join(format!("am-live3-{}", std::process::id()))
            .join(encode_path(root));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        fs::write(&path, line(root, "起点") + &line(deep, "进子目录")).unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mut sc = SessionScanner::new(dir.parent().unwrap().to_path_buf());
        let sum = sc.summarize(&path, meta.len(), 0).expect("应能解析出摘要");
        assert_eq!(sum.live_cwd, deep, "不在仓库里就用 shell_cwd 本身");
        assert_eq!(sum.shell_cwd, deep);
    }

    /// 按需现读：`current_cwd_of_session` 必须拿到**最后一条**记录的 cwd，
    /// 而且要能跟上刚追加的内容 —— 目录浏览/落盘的定位准不准全看它。
    #[test]
    fn current_cwd_reads_latest_and_follows_appends() {
        let dir = std::env::temp_dir().join(format!("am-now-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        fs::write(
            &path,
            line("/a/proj", "起点") + &line("/a/proj/sub", "cd 了"),
        )
        .unwrap();
        assert_eq!(
            current_cwd_of_session(&path).as_deref(),
            Some("/a/proj/sub")
        );

        // 再 cd 一次：不带任何缓存，下一次调用就该看到新值
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(line("/a/proj/sub/deeper", "又深了").as_bytes())
            .unwrap();
        f.flush().unwrap();
        assert_eq!(
            current_cwd_of_session(&path).as_deref(),
            Some("/a/proj/sub/deeper"),
            "定期扫描的快照会滞后一两分钟，这条路径必须是现读的"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 尾部第一行常是被截断的半行（只读最后 64KB），不能因此整个取空。
    #[test]
    fn current_cwd_tolerates_truncated_first_line() {
        let dir = std::env::temp_dir().join(format!("am-trunc-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        // 头一行是被字节截断的半行（后面跟着换行，与 read_tail 的真实产物一致），
        // 其后才是完整记录
        fs::write(
            &path,
            "{\"cwd\":\"/a/br\n".to_string() + &line("/a/proj", "好行"),
        )
        .unwrap();
        assert_eq!(current_cwd_of_session(&path).as_deref(), Some("/a/proj"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 没漂移过的会话：两者相同，调用方按 `live_cwd == cwd` 走老路即可。
    #[test]
    fn live_cwd_equals_cwd_without_drift() {
        let root = "/tmp/amlive2/proj";
        let dir = std::env::temp_dir()
            .join(format!("am-live2-{}", std::process::id()))
            .join(encode_path(root));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("s.jsonl");
        fs::write(&path, line(root, "一直没动")).unwrap();

        let meta = fs::metadata(&path).unwrap();
        let mut sc = SessionScanner::new(dir.parent().unwrap().to_path_buf());
        let sum = sc.summarize(&path, meta.len(), 0).expect("应能解析出摘要");
        assert_eq!(sum.live_cwd, sum.cwd);
    }
}

#[cfg(test)]
mod short_name_tests {
    use super::short_name;

    /// Windows 上报的 cwd 常带尾随反斜杠，不能取出空项目名
    #[test]
    fn handles_trailing_separators() {
        assert_eq!(short_name(r"D:\Program\Jingxin-Agent\"), "Jingxin-Agent");
        assert_eq!(short_name("/Users/x/proj/"), "proj");
        assert_eq!(short_name("/Users/x/proj"), "proj");
        assert_eq!(short_name(r"D:\cursor\"), "cursor");
    }
}

#[cfg(test)]
mod desktop_session_tests {
    use super::*;
    use crate::model::TaskStatus;

    fn sess(provider: &str, id: &str, desktop: bool, cwd: &str, mtime_ms: u64) -> SessionSummary {
        SessionSummary {
            provider: provider.into(),
            desktop,
            session_id: id.into(),
            project_key: encode_path(cwd),
            cwd: cwd.into(),
            live_cwd: cwd.into(),
            shell_cwd: cwd.into(),
            title: id.into(),
            prompt: String::new(),
            last_action: String::new(),
            turn_ended: true,
            cleared: false,
            clear_born: false,
            has_content: true,
            sub_agent_count: 0,
            started_at: None,
            last_active_at: None,
            version: None,
            git_branch: None,
            mtime_ms,
            created_ms: mtime_ms,
            line_count: 1,
            used_tokens_5h: 0,
            queued_inputs: Vec::new(),
            select_answered_ms: None,
        }
    }

    /// ChatGPT 桌面版的共享宿主：cwd 恒为 `/`、无 tty、一个进程托着全部桌面会话。
    fn host(pid: u32, start_time: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            agent: "codex".into(),
            tty: String::new(),
            cwd: "/".into(),
            ide: crate::model::IdeKind::Desktop,
            ide_name: "ChatGPT".into(),
            start_time,
            cpu_usage: 0.0,
            memory: 0,
            command: "/Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server".into(),
            shell_pid: None,
            shell_start: None,
            shared_host: true,
        }
    }

    /// Claude 桌面版本地代理跑在宿主机上时（`hostLoopMode`）的进程形态：装在应用包里的
    /// claude、没有 tty、cwd 就是那条会话的工作目录 —— 一进程一会话，与终端会话同构。
    fn cowork_proc(pid: u32, cwd: &str, start_time: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            agent: "claude".into(),
            tty: String::new(),
            cwd: cwd.into(),
            ide: crate::model::IdeKind::Desktop,
            ide_name: "Claude".into(),
            start_time,
            cpu_usage: 0.0,
            memory: 0,
            command: "…/Claude/claude-code/2.1.260/claude.app/Contents/MacOS/claude".into(),
            shell_pid: None,
            shell_start: None,
            shared_host: false,
        }
    }

    fn build(sessions: &[SessionSummary], procs: &[ProcessInfo]) -> Vec<Task> {
        let paused: Box<dyn Fn(u32) -> bool> = Box::new(|_| false);
        build_tasks(
            sessions,
            procs,
            &paused,
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
        )
    }

    /// Claude 桌面版本地代理不走共享宿主那条路：它跑在宿主机上时是「一进程一会话、
    /// cwd 即工作目录」，与终端会话同构，直接用既有的按 cwd 配对层（tier①）——
    /// 唯一的区别是它没有 tty，那道闸门由 [`crate::model::IdeKind::Desktop`] 放行。
    /// 不为它另起一套配对逻辑，是为了不让两套判断并存。
    #[test]
    fn cowork_session_pairs_by_cwd_without_tty() {
        let now = now_ms();
        let cwd = "/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/o/u/local_a/outputs";
        let procs = vec![cowork_proc(700, cwd, now / 1000 - 300)];
        let sessions = vec![sess("claude", "k1", true, cwd, now - 3_000)];
        let tasks = build(&sessions, &procs);
        assert_eq!(tasks.len(), 1, "不该多出一张未配对进程的占位卡");
        assert_eq!(tasks[0].pid, Some(700));
        assert_eq!(tasks[0].status, TaskStatus::Idle);
        assert_eq!(tasks[0].provider_dsr, "Claude 桌面版");
        // 「哪个客户端」是结构化事实，热路径的 Task 上必须带着它 —— 前端按
        // (provider, desktop) 分组，只给展示名的话它就得去抠中文串
        assert!(tasks[0].desktop, "桌面客户端会话的 desktop 必须为真");
        assert_eq!(tasks[0].ide_dsr, "Claude");
    }

    /// 桌面会话配到共享宿主：不比 cwd（宿主的 cwd 是 `/`，跟谁都对不上），
    /// 一个宿主同时托多条会话，状态不再一律「已结束」。
    #[test]
    fn desktop_sessions_pair_with_shared_host() {
        let now = now_ms();
        let start = now / 1000 - 600;
        let procs = vec![host(900, start)];
        let sessions = vec![
            sess("codex", "d1", true, "/w/a", now - 10_000),
            sess("codex", "d2", true, "/w/b", now - 20_000),
        ];
        let tasks = build(&sessions, &procs);
        assert_eq!(tasks.len(), 2, "共享宿主不额外生成占位卡");
        for t in &tasks {
            assert_eq!(t.pid, Some(900), "{} 该配到桌面宿主", t.id);
            assert_eq!(t.status, TaskStatus::Idle);
            assert_eq!(t.provider_dsr, "ChatGPT 桌面版");
            assert!(t.desktop, "ChatGPT 桌面版的会话 desktop 必须为真");
        }
    }

    /// 上次开 App 时留下、这次没碰过的旧对话不该顶着 pid 显示成「等待输入」。
    /// 判据与 tier④ 一致：会话最后写入必须不早于宿主进程启动。
    #[test]
    fn stale_desktop_sessions_stay_finished() {
        let now = now_ms();
        let start = now / 1000 - 60; // 宿主 1 分钟前才起来
        let procs = vec![host(900, start)];
        let sessions = vec![sess("codex", "old", true, "/w/a", now - 3_600_000)];
        let tasks = build(&sessions, &procs);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].pid, None);
        assert_eq!(tasks[0].status, TaskStatus::Finished);
    }

    /// 回归：桌面客户端没有活动会话时，共享宿主进程一张卡都不该出。
    /// 当初把 `codex app-server` 整个挡掉，就是因为它会冒出一条
    /// 「（会话尚未产生记录）」的空会话卡。
    #[test]
    fn shared_host_never_becomes_placeholder_card() {
        let now = now_ms();
        let procs = vec![host(900, now / 1000 - 60)];
        assert!(build(&[], &procs).is_empty(), "宿主自己不是一条会话");
        // 只有终端 CLI 会话时也一样：宿主不掺和，也不去抢 CLI 会话
        let cli = vec![sess("codex", "c1", false, "/w/a", now - 5_000)];
        let tasks = build(&cli, &procs);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].pid, None, "CLI 会话不能被桌面宿主认领");
        assert_eq!(tasks[0].provider_dsr, "Codex");
        assert!(
            !tasks[0].desktop,
            "终端 CLI 的 desktop 必须为假 —— 同机两个客户端靠它分开"
        );
    }

    /// 桌面宿主只认自己那个 provider 的桌面会话。
    #[test]
    fn shared_host_does_not_cross_providers() {
        let now = now_ms();
        let procs = vec![host(900, now / 1000 - 600)];
        let sessions = vec![sess("claude", "k1", true, "/w/a", now - 5_000)];
        let tasks = build(&sessions, &procs);
        assert_eq!(tasks[0].pid, None, "claude 桌面会话不该配到 codex 宿主");
        assert_eq!(tasks[0].provider_dsr, "Claude 桌面版");
    }

    fn mk_desktop_tree(root: &Path, session: &str, project: &str, jsonl: &str) {
        let dir = root
            .join("org-uuid")
            .join("user-uuid")
            .join(session)
            .join(".claude")
            .join("projects")
            .join(project);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{jsonl}.jsonl")), "").unwrap();
    }

    /// Claude 桌面版本地代理：两层 id 之下的 `local_*/.claude/projects` 能被找出来。
    #[test]
    fn finds_claude_desktop_projects_roots() {
        let root = std::env::temp_dir().join(format!("am-cowork-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        mk_desktop_tree(&root, "local_aaa", "-sessions-x", "s1");
        mk_desktop_tree(&root, "local_bbb", "-sessions-y", "s2");
        // 干扰项：同层的非会话目录、以及没有 .claude/projects 的会话目录
        fs::create_dir_all(root.join("org-uuid").join("user-uuid").join("spaces")).unwrap();
        fs::create_dir_all(root.join("org-uuid").join("user-uuid").join("local_ccc")).unwrap();
        let mut got = claude_desktop_roots(&root);
        got.sort();
        assert_eq!(got.len(), 2, "只收有 .claude/projects 的会话目录: {got:?}");
        assert!(got[0].ends_with("local_aaa/.claude/projects"));
        let _ = fs::remove_dir_all(&root);
    }

    /// 上游把目录层级换掉（或压根没装桌面版）时安静降级：找不到就是空表，不报错。
    #[test]
    fn missing_or_changed_layout_degrades_quietly() {
        let missing = std::env::temp_dir().join(format!("am-cowork-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&missing);
        assert!(
            claude_desktop_roots(&missing).is_empty(),
            "目录不存在 = 什么都不加"
        );

        // 层级变深超出回溯上限 → 找不到，退回改动前的行为
        let deep = std::env::temp_dir().join(format!("am-cowork-deep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&deep);
        let nested = deep.join("a").join("b").join("c").join("d").join("e");
        mk_desktop_tree(&nested, "local_aaa", "-sessions-x", "s1");
        assert!(claude_desktop_roots(&deep).is_empty());
        // 会话目录改名（不再是 local_ 前缀）→ 同样只是找不到
        let renamed = std::env::temp_dir().join(format!("am-cowork-ren-{}", std::process::id()));
        let _ = fs::remove_dir_all(&renamed);
        mk_desktop_tree(&renamed, "agent_aaa", "-sessions-x", "s1");
        assert!(claude_desktop_roots(&renamed).is_empty());
        for d in [&missing, &deep, &renamed] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// 从路径里认会话 id：必须**先**看到本地代理根目录名，才认 `local_` 那一段。
    /// 这个 id 要拿去跟窗口 URL 比对（见 client 的 appinject），认错等于发错会话。
    #[test]
    fn reads_local_agent_session_id_from_path() {
        // 实测取值：进程 cwd 落在会话隔离家目录的 outputs 里
        let cwd = "/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/\
                   dc6589d7-9da7-40ac-8c88-213585132c2c/26eaaf4d-6fa9-4370-9096-75398c87927c/\
                   local_18c6b796-a156-4546-a70e-0dd4da930073/outputs";
        assert_eq!(
            claude_local_agent_session_id(cwd).as_deref(),
            Some("local_18c6b796-a156-4546-a70e-0dd4da930073")
        );
        // 会话文件所在目录同样认得出（同一个 local_ 段）
        let jsonl = "/Users/u/Library/Application Support/Claude/local-agent-mode-sessions/o/u/\
                     local_abc/.claude/projects/-x/s1.jsonl";
        assert_eq!(
            claude_local_agent_session_id(jsonl).as_deref(),
            Some("local_abc")
        );
        // Windows 反斜杠
        assert_eq!(
            claude_local_agent_session_id(
                r"C:\Users\u\AppData\Roaming\Claude\local-agent-mode-sessions\o\u\local_win\outputs"
            )
            .as_deref(),
            Some("local_win")
        );
        // 没有根目录名 → 不认。用户自己有个叫 local_xxx 的项目目录不该被当成桌面会话。
        assert_eq!(
            claude_local_agent_session_id("/Users/u/code/local_something/src"),
            None
        );
        // 根目录名在 local_ 段**之后**出现也不认（顺序是判据的一部分）
        assert_eq!(
            claude_local_agent_session_id("/tmp/local_x/local-agent-mode-sessions"),
            None
        );
        // 终端 CLI 会话的 cwd → 不认，调用方据此退回「宿主上只能有一条会话」
        assert_eq!(claude_local_agent_session_id("/Users/u/code/proj"), None);
        assert_eq!(claude_local_agent_session_id(""), None);
    }

    /// 桌面版本地代理的 jsonl 走的是**同一套** Claude Code 解析器，只是换个扫描根；
    /// 出来的会话带桌面标记与桌面展示名。
    #[test]
    fn desktop_projects_root_reuses_claude_parser() {
        let root = std::env::temp_dir().join(format!("am-cowork-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let proj = root
            .join("org")
            .join("user")
            .join("local_zzz")
            .join(".claude")
            .join("projects")
            .join("-sessions-demo");
        fs::create_dir_all(&proj).unwrap();
        let line = serde_json::json!({
            "type": "user",
            "cwd": "/sessions/demo",
            "sessionId": "sid-1",
            "timestamp": "2026-09-09T10:00:00.000Z",
            "message": { "role": "user", "content": "跑个本地代理" }
        });
        fs::write(proj.join("sid-1.jsonl"), format!("{line}\n")).unwrap();

        let mut sc = SessionScanner::new(root.join("no-cli-projects"));
        let mut out = Vec::new();
        let roots = claude_desktop_roots(&root);
        assert_eq!(roots.len(), 1);
        sc.scan_projects_root(&roots[0], true, &mut out, now_ms());
        assert_eq!(out.len(), 1, "本地代理会话该被同一套解析器读出来");
        assert!(out[0].desktop);
        assert_eq!(out[0].provider, "claude");
        assert_eq!(out[0].cwd, "/sessions/demo");
        assert_eq!(
            crate::model::provider_dsr_desktop(&out[0].provider),
            "Claude 桌面版"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
