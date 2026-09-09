use crate::model::{MessageBrief, ProcessInfo, Task, TaskStatus};
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
    /// 本会话是刚 `/clear` 出来、还没输入的「全新空会话」：内容只有 /clear 命令块、无真实
    /// prompt。claude 执行 /clear 会另起这样一个会话，同一进程从旧会话转到它。build_tasks
    /// 据此把进程从「被清空取代的旧会话」迁到本会话（clear-follow），否则进程会被 tier②
    /// 「认领 created 最早的会话」粘回旧会话 → 网页内容/标题定格在清空前。
    pub cleared: bool,
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

/// 会话列表最多回溯的时长（毫秒）：7 天
const HISTORY_WINDOW_MS: u64 = 7 * 24 * 3600 * 1000;
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
        .map(|c| c.join("Claude").join("local-agent-mode-sessions"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
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

    /// 扫描全部项目目录，返回近 7 天内有活动的会话摘要
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
        out.sort_by_key(|b| std::cmp::Reverse(b.mtime_ms));
        out
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
                if now_ms.saturating_sub(mtime_ms) > HISTORY_WINDOW_MS {
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

    /// 递归收集 Codex 会话摘要（7 天窗口，带同一套 mtime/size 缓存）
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
            if now_ms.saturating_sub(mtime_ms) > HISTORY_WINDOW_MS {
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

    /// 解析一个会话的对话消息（供前台对话流展示），返回最后 limit 条。
    /// 末尾附带两条「当前状态」快照：任务清单（todos）与后台任务（bgtasks）。
    pub fn messages(&mut self, session_id: &str, limit: usize) -> Result<Vec<MessageBrief>> {
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
        if is_codex {
            // Codex 没有任务清单/后台任务语义，直接裁剪返回
            let skip = msgs.len().saturating_sub(limit);
            return Ok(msgs.into_iter().skip(skip).collect());
        }
        let skip = msgs.len().saturating_sub(limit);
        let mut out: Vec<MessageBrief> = msgs.into_iter().skip(skip).collect();

        // 状态快照：从会话开头增量重放得来，不受上面 limit 窗口影响，
        // 一律追加在末尾（前端会把它们摘出去单独渲染，位置无所谓）。
        let ts = out.last().map(|m| m.timestamp.clone()).unwrap_or_default();
        let (todos, bgtasks) = self.replay_state(&path)?;
        if let Some(m) = todos {
            out.push(MessageBrief {
                role: "todos".into(),
                content: m,
                timestamp: ts.clone(),
                is_error: false,
            });
        }
        if let Some(m) = bgtasks {
            out.push(MessageBrief {
                role: "bgtasks".into(),
                content: m,
                timestamp: ts,
                is_error: false,
            });
        }
        Ok(out)
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

    /// 增量重放任务清单与后台任务，返回两者的 JSON 快照。
    /// 只解析上次之后新增的字节；文件被截断/轮转时从头重来。
    fn replay_state(&mut self, path: &Path) -> Result<(Option<String>, Option<String>)> {
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
        let bg = st
            .bg
            .reconciled(&subs.last_write, now_ms, &|id| subs.tail(id));
        let bg = if bg.is_empty() {
            None
        } else {
            serde_json::to_string(&bg).ok()
        };
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
        if now.saturating_sub(s.mtime_ms) < 7 * 24 * 3600 * 1000 {
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

    // 第一优先：按「进程打开着哪个会话文件」得出的确定配对（pinned）。
    // 这能解决「关闭的会话 mtime 反而更新、抢走了活进程」——因为已关闭会话的文件
    // 没有活进程占着，压根不会出现在 pinned 里；闲置但仍开着的会话则会被正确配上。
    if !pinned.is_empty() {
        for (pid, sid) in pinned {
            if let (Some(p), Some(s)) = (proc_by_pid.get(pid), sid_index.get(sid.as_str()).copied())
            {
                pid_of_session.insert(s.session_id.as_str(), *p);
                paired_pids.insert(*pid);
            }
        }
    }

    // clear-follow：claude 执行 /clear 会另起一个「只含 /clear 命令、还没输入」的全新会话
    // （cleared=true），同一进程从旧会话转到它。但下面 tier② 会按「created 最早」把进程粘回
    // 它启动时创建的旧会话 → 网页内容/标题定格在清空前。这里据配对缓存把「上一轮配在同项目、
    // 现被清空取代」的进程迁到新会话（进程还是同一个，只是会话号变了）。缓存是权威信号
    //（上一轮该 pid 确实配在旧会话），单会话场景下无歧义；多进程同时 /clear 时按「旧会话
    // mtime 最新」挑最可能刚清空的那个，至少不会更差。被迁走的旧会话记入 released，本轮不
    // 再被其它 tier 抢配（它已结束）。
    let mut released: HashSet<&str> = HashSet::new();
    // clear-follow (a) 保持：上一轮已迁到 cleared 空会话的进程，本轮继续粘住它，抢在 tier②
    // 之前。否则——缓存本轮已指向新会话、(b) 不会再迁，空闲的进程会被 tier② 按「created 最早」
    // 又拽回旧会话；下一轮缓存又变回旧会话、(b) 再迁到新…… 于是进程在 旧↔新 间每轮抖动，
    // 表现为卡片标题/内容闪烁（旧会话有标题 ↔ 新空会话只剩项目名）。粘住即止住抖动。
    for (pid, sid) in cached {
        if paired_pids.contains(pid) || pid_of_session.contains_key(sid.as_str()) {
            continue;
        }
        let Some(s) = sid_index.get(sid.as_str()).copied() else {
            continue;
        };
        if s.cleared {
            if let Some(p) = proc_by_pid.get(pid) {
                pid_of_session.insert(s.session_id.as_str(), *p);
                paired_pids.insert(*pid);
            }
        }
    }
    // clear-follow (b) 迁移：收集本轮全新清空会话；没有就整层跳过，额外开销只落在真有 /clear 的轮次
    let mut fresh: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.cleared && !pid_of_session.contains_key(s.session_id.as_str()))
        .collect();
    if !fresh.is_empty() {
        fresh.sort_by_key(|s| s.created_ms); // 按 created 升序稳定处理
        for s_new in fresh {
            // 候选：缓存里配在「同项目、更旧会话」的存活且未配对进程
            let mut best: Option<(u32, &SessionSummary)> = None; // (pid, 被取代的旧会话)
            for (pid, old_sid) in cached {
                if old_sid == &s_new.session_id || paired_pids.contains(pid) {
                    continue;
                }
                let Some(p) = proc_by_pid.get(pid) else {
                    continue;
                };
                if p.agent != s_new.provider || encode_path(&p.cwd) != s_new.project_key {
                    continue;
                }
                // 旧会话须存在、且比新会话更早创建（确是被取代的前身）
                let Some(old) = sid_index.get(old_sid.as_str()).copied() else {
                    continue;
                };
                if old.created_ms >= s_new.created_ms {
                    continue;
                }
                if best.is_none_or(|(_, b)| old.mtime_ms > b.mtime_ms) {
                    best = Some((*pid, old));
                }
            }
            if let Some((pid, old)) = best {
                pid_of_session.insert(s_new.session_id.as_str(), proc_by_pid[&pid]);
                paired_pids.insert(pid);
                // 该 pid 的旧会话已被取代 → 释放，避免其它 tier 又把它配给别的进程
                released.insert(old.session_id.as_str());
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
                .filter(|s| {
                    !pid_of_session.contains_key(s.session_id.as_str())
                        && !released.contains(s.session_id.as_str())
                })
                .copied()
                .collect();

            // ① 命令行 --resume <id>：恢复指定会话（创建于很久前，靠命令行认出）。
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
            free_procs.retain(|p| {
                if wants_continue(&p.command) && !free_sess.is_empty() {
                    let s = free_sess.remove(0);
                    pid_of_session.insert(s.session_id.as_str(), p);
                    paired_pids.insert(p.pid);
                    return false;
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
            // 安全性完全由上面的 mtime>=启动 约束保证，与活跃间隔无关；会话集合本身已卡 7 天窗口。
            let mut free_sess: Vec<&SessionSummary> = free_sess.into_iter().collect();
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
            if let (Some(p), Some(s)) = (
                proc_by_pid.get(pid),
                sid_index
                    .get(sid.as_str())
                    .copied()
                    .filter(|s| now.saturating_sub(s.mtime_ms) < 7 * 24 * 3600 * 1000),
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
            if pid_of_session.contains_key(s.session_id.as_str()) {
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
            title,
            used_tokens_5h: 0,
            token_limit: 0,
            auto_paused: false,
            status_dsr: status.dsr().to_string(),
            ide_dsr: p.ide_name.clone(),
            pid: Some(p.pid),
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
    // 是否见过 /clear 命令块（尾窗内）。与「无真实 prompt」合起来 → cleared：刚清空、未输入。
    let mut saw_clear = false;
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
                    prompt = text;
                    last_action = "等待助手响应".into();
                    turn_ended = false;
                    saw_clear = false; // 清空后又有真实输入 → 不再是「刚清空的空会话」
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
            let content = if args.is_empty() {
                name.to_string()
            } else {
                format!("{name}: {}", truncate(args, 120))
            };
            Some(MessageBrief {
                role: "tool".into(),
                content,
                timestamp: ts,
                is_error: false,
            })
        }
        "function_call_output" | "custom_tool_call_output" => {
            let out = p.get("output").and_then(Value::as_str).unwrap_or("");
            (!out.trim().is_empty()).then(|| MessageBrief {
                role: "tool_result".into(),
                content: truncate(out.trim(), 400),
                timestamp: ts,
                is_error: false,
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

fn entry_to_brief(v: &Value) -> Option<MessageBrief> {
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ty = v.get("type").and_then(Value::as_str)?;
    if v.get("isSidechain")
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
            let mut tools = Vec::new();
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
                        let hint = tool_input_hint(item.get("input"));
                        tools.push(if hint.is_empty() {
                            name.to_string()
                        } else {
                            format!("{name}: {hint}")
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
                });
            }
            // 交互式选择卡片：整份 input（questions/options）序列化给前端
            if let Some(inp) = select_input {
                return Some(MessageBrief {
                    role: "select".into(),
                    content: truncate(&inp.to_string(), 4000),
                    timestamp: ts,
                    is_error: false,
                });
            }
            if !text_buf.trim().is_empty() {
                Some(MessageBrief {
                    role: "assistant".into(),
                    content: truncate(text_buf.trim(), FLOW_TEXT_MAX),
                    timestamp: ts,
                    is_error: false,
                })
            } else if !tools.is_empty() {
                Some(MessageBrief {
                    role: "tool".into(),
                    content: truncate(&tools.join(" | "), 400),
                    timestamp: ts,
                    is_error: false,
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

/// 后台运行的任务（后台命令 / 异步子代理）
#[derive(Debug, Clone, Serialize)]
struct BgTask {
    id: String,
    label: String,
    /// running | completed | failed | killed | stopped
    status: String,
    /// "agent"（异步子代理）| "bg"（后台命令）—— 前端据此拆成独立的子代理列表
    kind: String,
    /// 起跑时刻（会话记录里的 ISO8601 时间戳），前端据此算耗时；拿不到就空串
    #[serde(rename = "startedAt")]
    started_at: String,
    /// 完成通知里的 `<summary>` 原文 —— 这是**唯一**说得出「为什么是这个收场」的字段。
    ///
    /// 实测本机 466 份会话记录、1431 条去重后的 `<task-notification>`：
    /// `<summary>` 出现 1412 次，非 completed 的 141 条里 138 条有它，缺的 3 条
    /// 全是 `__orphan_summary__` 那种合成通知（本就没有单条任务的收尾信息）。
    /// 形态固定为一行，长度中位数 75、p90 243、最长 926 字符，例如：
    /// - `Background command "…" failed with exit code 137`
    /// - `Agent "…" failed: Agent stalled: no progress for 600s (stream watchdog did not recover)`
    /// - `Agent "…" failed: Agent terminated early due to an API error: …（error type rate_limit, HTTP 429, request id …）`
    /// - `Agent "…" was stopped by Claude` / `Background command "…" was stopped`
    ///
    /// **原样下发，不做任何解析**：退出码、限流原因、卡死时长都嵌在这句话里，
    /// 而这句话是上游随时会改的英文文案。去里面抠 `exit code (\d+)` 就是拿字面量
    /// 当接口用，上游改一版就整条哑掉；下发原文则最多是措辞变了，信息不会丢。
    ///
    /// 只有通知带过来才有值；[`Self::reconciled`] 靠磁盘改判状态时会清掉它 ——
    /// 那时这句话描述的已经不是当前状态了。
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    /// 最近一条完成通知的时刻（epoch 毫秒，0 = 还没收到）。只用于与磁盘对齐时
    /// 判「这条通知是不是已经过期」（子会话被唤醒续跑了），不外发。
    #[serde(skip)]
    ended_ms: u64,
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
    items: Vec<BgTask>,
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
            t.label = label;
            t.status = status;
            t.summary = summary;
            t.started_at = started_at;
            t.ended_ms = ended_ms;
            return;
        }
        self.items.push(BgTask {
            id,
            label,
            status,
            kind: kind.to_string(),
            started_at,
            summary,
            ended_ms,
        });
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
                if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
                    t.status = status.clone();
                    t.summary = summary.clone();
                    t.ended_ms = ended_ms;
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

    /// 把父会话记录重放出的清单与磁盘上的子会话记录对齐，得到当前真实状态。
    ///
    /// 父记录里的 `<task-notification>` 是唯一的收尾信号，可它并不保证写得下来
    /// （父进程被打断/退出、机器重启时就没了）——一旦缺席，条目会永远停在「执行中」。
    /// 而子会话自己那份记录是硬事实，收尾形态还是固定的，见 [`SubAgentTail`]。
    ///
    /// 两种对齐，都只针对子代理（`kind == "agent"`），后台命令没有这份记录 ——
    /// 也不需要：后台命令的两条收尾信号（完成通知、`TaskStop` 结果）都在父记录里，
    /// 只要两条都认全就没有无上界的条目（实测详见 [`BgTracker`] 与 [`stopped_task_id`]）。
    /// - 清单说在跑、它自己却早已收尾 → 落终态（父记录漏了那条通知）；
    /// - 清单说已结束、它却在通知很久之后还在写 → 那条通知过期了（子会话被唤醒续跑，
    ///   通知正文自己写着 "the same task-id may notify more than once"），改按记录判。
    ///
    /// 「很久」不能取小：实测本机 340 个子会话，「最后一条通知 → 记录最后写入」的间隔
    /// p99 = 0.1 秒（被 kill 的那几个也只差 0.0~0.1 秒，纯写入竞争），唯一的例外
    /// `a7bca81f9e85bfd33` 是 +1042 秒 —— 正是一个被唤醒续跑的。取
    /// [`SUBAGENT_SETTLE_MS`]（5 分钟）作界，比那个写入竞争大三个数量级。
    /// 第一版按「晚于通知即算复活」判，把 30 小时前 failed 的 `aa8f4211d424433a4`
    /// 重新点亮成执行中，就是栽在这 0.1 秒上。
    ///
    /// 不改 `self.items`：对齐结果每轮现算，父记录后来补上真状态时以父记录为准。
    ///
    /// `last_write` 为空（没派过子会话、或旧版 Claude Code 不建这个目录）时原样返回。
    /// `tail_of` 只在真需要时才调用（它要读文件尾），绝大多数条目走不到。
    fn reconciled(
        &self,
        last_write: &HashMap<String, u64>,
        now_ms: u64,
        tail_of: &dyn Fn(&str) -> Option<SubAgentTail>,
    ) -> Vec<BgTask> {
        let mut items = self.items.clone();
        if last_write.is_empty() {
            return items;
        }
        for t in items.iter_mut().filter(|t| t.kind == "agent") {
            let Some(&wrote_ms) = last_write.get(&t.id) else {
                continue;
            };
            // 已是终态、且通知之后没再动过 → 通知说了算，不必读文件
            let resumed = wrote_ms > t.ended_ms.saturating_add(SUBAGENT_SETTLE_MS);
            if t.status != "running" && !resumed {
                continue;
            }
            let idle_ms = now_ms.saturating_sub(wrote_ms);
            match tail_of(&t.id) {
                // 结果已经交回去了，静置够久就是真跑完了 —— 父记录漏了那条通知而已
                Some(SubAgentTail::Finished) if idle_ms > SUBAGENT_SETTLE_MS => {
                    t.status = "completed".into();
                    t.summary = None;
                }
                // 停在半路：正常是在等一个慢工具，久到不像话就是被 kill 在半路了
                Some(SubAgentTail::Midflight) if idle_ms > SUBAGENT_ABANDON_MS => {
                    t.status = "stopped".into();
                    t.summary = None;
                }
                // 还在写：在跑（对续跑的条目就是从终态翻回来）
                Some(_) => {
                    t.status = "running".into();
                    t.summary = None;
                }
                None => {}
            }
        }
        items
    }
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

    /// 目录里所有子会话记录的最新写入时刻（没有就 0）。供上层做缓存键。
    fn newest_ms(&self) -> u64 {
        self.last_write.values().copied().max().unwrap_or(0)
    }

    /// 读某个子会话记录的最后一条完整记录，判形态。
    ///
    /// 只读尾部 256KB：一条记录再大也进得来，而整份记录可达数 MB，每轮全读吃不消。
    /// 末尾那行可能正被写入（只有半截）——解析不了就说明它此刻正在写，算半路。
    fn tail(&self, id: &str) -> Option<SubAgentTail> {
        let text = read_tail(&self.dir.join(format!("agent-{id}.jsonl")), 256 * 1024).ok()?;
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

    fn agent(id: &str, status: &str, ended_ms: u64) -> BgTask {
        BgTask {
            id: id.into(),
            label: "子会话".into(),
            status: status.into(),
            kind: "agent".into(),
            started_at: "2026-09-08T02:55:52.284Z".into(),
            summary: Some("Agent \"子会话\" failed: Agent stalled".into()),
            ended_ms,
        }
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
        let out = t.reconciled(&writes("a320f1242950d09d0", now - 8 * HOUR), now, &|_| {
            Some(SubAgentTail::Finished)
        });
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
        // 通知之后很久还在写 → 是被唤醒续跑了，终态与原因都作废
        let out = t.reconciled(&writes("a1", now - 60_000), now, &|_| {
            Some(SubAgentTail::Midflight)
        });
        assert_eq!(out[0].status, "running");
        assert_eq!(out[0].summary, None, "翻回执行中就不该再挂着失败原因");

        // 父记录漏了通知、子会话自己早已交回结果 → 判 completed，旧原因同样作废
        let mut t = BgTracker::default();
        t.items.push(agent("a2", "running", 0));
        t.items[0].summary = Some("Agent \"x\" failed: 过期的原因".into());
        let out = t.reconciled(&writes("a2", now - 8 * HOUR), now, &|_| {
            Some(SubAgentTail::Finished)
        });
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
        let out = t.reconciled(&writes("a1", now - 60_000), now, &|_| {
            Some(SubAgentTail::Finished)
        });
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
        let out = t.reconciled(&writes("a1", now - 30 * 60 * 1000), now, &|_| {
            Some(SubAgentTail::Midflight)
        });
        assert_eq!(out[0].status, "running", "慢工具的合法空窗不该判死");
    }

    /// 半路被 kill、父会话又没写下通知：只剩时间能兜底
    #[test]
    fn abandoned_midflight_subagent_is_stopped() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let now = 12 * HOUR;
        let out = t.reconciled(&writes("a1", now - 3 * HOUR), now, &|_| {
            Some(SubAgentTail::Midflight)
        });
        assert_eq!(out[0].status, "stopped");
    }

    /// 被 kill 的子会话，最后一笔落盘比通知晚 0.0~0.1 秒（纯写入竞争）。据此判「复活」
    /// 会把 30 小时前 failed 的 `aa8f4211d424433a4` 重新点亮成执行中 —— 实测 340 个
    /// 子会话里，这个间隔的 p99 只有 0.1 秒。
    #[test]
    fn write_race_after_notification_is_not_a_resume() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        // 通知在 10 小时前，记录最后写入只比它晚 100 毫秒
        t.items.push(agent("a1", "failed", now - 10 * HOUR));
        let out = t.reconciled(&writes("a1", now - 10 * HOUR + 100), now, &|_| {
            unreachable!("通知之后没动静就不该去读文件")
        });
        assert_eq!(out[0].status, "failed");
    }

    /// 子会话可以在「完成通知」之后被唤醒续跑（通知正文自己写着可能通知多次）。
    /// 实测本机 `a7bca81f9e85bfd33` 就是这样：通知之后记录又长了 1042 秒。
    /// 不认这一条的话，正干着活的子会话在清单里是 completed，头部一个胶囊都不显示。
    #[test]
    fn resumed_after_notification_comes_back_to_running() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "completed", now - HOUR));
        let out = t.reconciled(&writes("a1", now - 5_000), now, &|_| {
            Some(SubAgentTail::Midflight)
        });
        assert_eq!(out[0].status, "running");
    }

    /// 续跑之后又跑完了：还是按记录判，落回终态
    #[test]
    fn resumed_then_finished_settles_back() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "completed", now - 2 * HOUR));
        let out = t.reconciled(&writes("a1", now - HOUR), now, &|_| {
            Some(SubAgentTail::Finished)
        });
        assert_eq!(out[0].status, "completed");
    }

    /// 两道时间判定都是**严格大于**，边界值那一刻还不算数
    #[test]
    fn thresholds_are_exclusive_at_the_boundary() {
        let now = 12 * HOUR;
        let run = |idle: u64, tail: SubAgentTail| {
            let mut t = BgTracker::default();
            t.items.push(agent("a1", "running", 0));
            t.reconciled(&writes("a1", now - idle), now, &|_| Some(tail))[0]
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

    /// 续跑判定同样是严格大于：只比通知晚 SETTLE 那一刻不算续跑
    #[test]
    fn resume_threshold_is_exclusive_at_the_boundary() {
        let now = 12 * HOUR;
        let ended = now - HOUR;
        let run = |wrote: u64| {
            let mut t = BgTracker::default();
            t.items.push(agent("a1", "failed", ended));
            t.reconciled(&writes("a1", wrote), now, &|_| {
                Some(SubAgentTail::Midflight)
            })[0]
                .status
                .clone()
        };
        assert_eq!(
            run(ended + SUBAGENT_SETTLE_MS),
            "failed",
            "边界上还不算续跑"
        );
        assert_eq!(run(ended + SUBAGENT_SETTLE_MS + 1), "running");
    }

    /// 终态覆盖：failed 的条目被唤醒续跑、这回跑完了 → 改写成 completed
    /// （光测 completed→completed 覆盖不到「终态被换成另一个终态」这条路）
    #[test]
    fn resumed_failed_item_settles_to_completed() {
        let mut t = BgTracker::default();
        let now = 12 * HOUR;
        t.items.push(agent("a1", "failed", now - 3 * HOUR));
        // 通知之后又写了两小时，且已静置够久
        let out = t.reconciled(&writes("a1", now - HOUR), now, &|_| {
            Some(SubAgentTail::Finished)
        });
        assert_eq!(out[0].status, "completed", "续跑跑完了就不该还挂着 failed");
    }

    /// 后台命令（kind=bg）没有子会话记录，不参与对齐
    #[test]
    fn background_commands_are_untouched() {
        let mut t = BgTracker::default();
        let mut cmd = agent("b1", "running", 0);
        cmd.kind = "bg".into();
        t.items.push(cmd);
        let now = 12 * HOUR;
        let out = t.reconciled(&writes("b1", now - 8 * HOUR), now, &|_| {
            Some(SubAgentTail::Finished)
        });
        assert_eq!(out[0].status, "running");
    }

    /// 没派过子会话 / 旧版 Claude Code 不建这个目录时原样返回，不改任何判定
    #[test]
    fn without_subagent_dir_nothing_changes() {
        let mut t = BgTracker::default();
        t.items.push(agent("a1", "running", 0));
        let out = t.reconciled(&HashMap::new(), 12 * HOUR, &|_| unreachable!());
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
        let (todos, _) = sc.replay_state(&path).unwrap();
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
        let (todos, _) = sc.replay_state(&path).unwrap();
        assert!(todos.unwrap().contains("甲"));
        let after_first = sc.state_cache.get(&path).unwrap().offset;
        assert!(after_first > 0);

        // 追加第二个任务，只该解析新增部分
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all((create("u2", "乙") + &created("u2", 2, "乙")).as_bytes())
            .unwrap();
        f.flush().unwrap();

        let (todos, _) = sc.replay_state(&path).unwrap();
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
        let (todos, _) = sc.replay_state(&path).unwrap();
        assert!(todos.unwrap().contains("甲"));
        let off = sc.state_cache.get(&path).unwrap().offset;

        // 补全那半行
        let rest = create("u2", "乙");
        let rest = &rest[rest.len() / 2..];
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(rest.as_bytes()).unwrap();
        f.write_all(created("u2", 2, "乙").as_bytes()).unwrap();
        f.flush().unwrap();

        let (todos, _) = sc.replay_state(&path).unwrap();
        assert!(
            todos.unwrap().contains("乙"),
            "补全后该行必须被完整解析（偏移没有停在行中间）"
        );
        assert!(sc.state_cache.get(&path).unwrap().offset > off);

        let _ = fs::remove_dir_all(&dir);
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
            "type": "function_call", "name": "spawn_agent",
            "arguments": "{\"task\":\"x\"}"
        }));
        let m = codex_entry_to_brief(&f).unwrap();
        assert_eq!(m.role, "tool");
        assert!(m.content.starts_with("spawn_agent"));

        let o = line(serde_json::json!({
            "type": "custom_tool_call_output", "output": "done"
        }));
        assert_eq!(codex_entry_to_brief(&o).unwrap().role, "tool_result");

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
