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
    pub session_id: String,
    /// 项目目录编码名（~/.claude/projects 下的目录名），配对进程用
    pub project_key: String,
    pub cwd: String,
    /// 会话标题（首个用户提示词）
    pub title: String,
    pub prompt: String,
    pub last_action: String,
    /// 最后一条有效条目是否表示「回合结束」（助手纯文本收尾）
    pub turn_ended: bool,
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

impl SessionScanner {
    pub fn new(projects_dir: PathBuf) -> Self {
        let codex_dir = dirs::home_dir()
            .map(|h| h.join(".codex/sessions"))
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        Self {
            projects_dir,
            codex_dir,
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
        let Ok(projects) = fs::read_dir(&self.projects_dir) else {
            return out;
        };
        for project in projects.flatten() {
            let pdir = project.path();
            if !pdir.is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(&pdir) else { continue };
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
                    out.push(summary);
                }
            }
        }
        // Codex CLI 会话（~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl）
        self.scan_codex_into(&mut out, now_ms);
        out.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
        out
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
            let Ok(meta) = fs::metadata(&path) else { continue };
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
        for line in head_txt.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
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
            path.file_stem()?.to_str()?.rsplitn(6, '-').next().map(String::from)
        })?;

        // 尾部：最近动作 + 回合是否结束（最后一条有效项是否助手文本）
        let tail = read_tail(path, TAIL_BYTES).ok()?;
        let mut last_action = String::new();
        let mut turn_ended = false;
        let mut last_active = None;
        for line in tail.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
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
            session_id,
            project_key: encode_path(&cwd),
            cwd,
            title: prompt.clone(),
            prompt,
            last_action,
            turn_ended,
            started_at,
            last_active_at: last_active,
            version: None,
            git_branch: None,
            mtime_ms,
            created_ms: 0,
            line_count: 0,
            used_tokens_5h: 0,
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
        // 会话标题 = 头部首个用户提示词（原始任务）；缺失时回退当前提示词
        summary.title = head
            .prompt
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| summary.prompt.clone());
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
        self.cache.insert(
            path.to_path_buf(),
            CacheEntry { size, mtime_ms, line_count, summary: summary.clone(), head },
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
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let brief = if is_codex { codex_entry_to_brief(&v) } else { entry_to_brief(&v) };
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
            out.push(MessageBrief { role: "todos".into(), content: m, timestamp: ts.clone() });
        }
        if let Some(m) = bgtasks {
            out.push(MessageBrief { role: "bgtasks".into(), content: m, timestamp: ts });
        }
        Ok(out)
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
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                st.todos.observe(&v);
                st.bg.observe(&v);
            }
            st.offset += end as u64;
        }
        // 强制产出当前态（dirty 只用于增量期间的去重，这里要的是全量快照）
        st.todos.dirty = true;
        st.bg.dirty = true;
        Ok((
            st.todos.take_snapshot("").map(|m| m.content),
            st.bg.take_snapshot("").map(|m| m.content),
        ))
    }

    fn find_session_file(&self, session_id: &str) -> Result<PathBuf> {
        // 防路径穿越：session_id 只允许 uuid 字符
        if !session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
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
    active_ids: &HashSet<String>,
) -> Vec<Task> {
    // 按 cwd 分组进程（已按启动时间升序）
    // 进程按「编码后的 cwd」分组，与会话的项目目录名对齐（会话内 cwd 会漂移，目录名不会）
    // 按 (provider, cwd-key) 分组：各 provider 的会话只与同类进程配对
    let mut proc_by_key: HashMap<(String, String), Vec<&ProcessInfo>> = HashMap::new();
    for p in processes {
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
        list.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    }

    let mut pid_of_session: HashMap<&str, &ProcessInfo> = HashMap::new();
    let mut paired_pids: HashSet<u32> = HashSet::new();

    // 第一优先：按「进程打开着哪个会话文件」得出的确定配对（pinned）。
    // 这能解决「关闭的会话 mtime 反而更新、抢走了活进程」——因为已关闭会话的文件
    // 没有活进程占着，压根不会出现在 pinned 里；闲置但仍开着的会话则会被正确配上。
    if !pinned.is_empty() {
        let proc_by_pid: HashMap<u32, &ProcessInfo> =
            processes.iter().map(|p| (p.pid, p)).collect();
        for (pid, sid) in pinned {
            if let (Some(p), Some(s)) = (
                proc_by_pid.get(pid),
                sessions.iter().find(|s| &s.session_id == sid),
            ) {
                pid_of_session.insert(s.session_id.as_str(), *p);
                paired_pids.insert(*pid);
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
                .map(|p| *p)
                .collect();
            let mut free_sess: Vec<&SessionSummary> = sess
                .iter()
                .filter(|s| !pid_of_session.contains_key(s.session_id.as_str()))
                .map(|s| *s)
                .collect();

            // ① 命令行 --resume <id>：恢复的会话 started_at 很旧，只能靠命令行认出来。
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

            // ② 会话文件创建时刻≈进程启动时刻：新起/空白的会话在终端打开（=进程启动）
            // 那刻创建，created_ms 与进程 start_time 只差几秒；旧的有内容会话创建于
            // 很久以前，差很远、被窗口挡在外面。这才是「空白新会话正确配上、关闭/闲置的
            // 旧会话不再冒充活进程」的关键（空白会话没 started_at、长跑会话 mtime 也不
            // 等于启动时刻，都不可靠，唯 created_ms 稳）。贪心取窗口内时间差最小的对。
            const CREATE_WINDOW_MS: i64 = 30 * 60 * 1000;
            loop {
                let mut best: Option<(usize, usize, i64)> = None;
                for (pi, p) in free_procs.iter().enumerate() {
                    if p.start_time == 0 {
                        continue;
                    }
                    let p_ms = (p.start_time as i64) * 1000;
                    for (si, s) in free_sess.iter().enumerate() {
                        if s.created_ms == 0 {
                            continue;
                        }
                        let d = (s.created_ms as i64 - p_ms).abs();
                        if d <= CREATE_WINDOW_MS && best.map_or(true, |(_, _, bd)| d < bd) {
                            best = Some((pi, si, d));
                        }
                    }
                }
                match best {
                    Some((pi, si, _)) => {
                        let p = free_procs.remove(pi);
                        let s = free_sess.remove(si);
                        pid_of_session.insert(s.session_id.as_str(), p);
                        paired_pids.insert(p.pid);
                    }
                    None => break,
                }
            }

            // ③ 剩余进程配会话：优先配「此刻正在被写」的活跃会话（active_ids = 本轮 mtime
            // 相比上轮有推进），其余按 mtime 最近者兜底。兜底必须保留 —— 否则闲置但真实的
            // 当前会话（助手刚回复、等待输入，mtime 已冻结）会配不上、显示空白。
            // free_sess 本就是 mtime 降序，稳定排序把活跃的提到前面即可。
            let mut ordered: Vec<&SessionSummary> = free_sess;
            ordered.sort_by_key(|s| !active_ids.contains(&s.session_id));
            let mut it = ordered.into_iter();
            for p in free_procs {
                match it.next() {
                    Some(s) => {
                        pid_of_session.insert(s.session_id.as_str(), p);
                        paired_pids.insert(p.pid);
                    }
                    None => break,
                }
            }
        }
    }

    let mut tasks = Vec::new();
    for s in sessions {
        let proc_info = pid_of_session.get(s.session_id.as_str()).map(|p| (*p).clone());
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
            provider_dsr: crate::model::provider_dsr(&s.provider),
            title: if s.title.is_empty() { s.prompt.clone() } else { s.title.clone() },
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
    let mut prompt = String::new();
    let mut last_action = String::new();
    let mut turn_ended = false;
    let mut started_at: Option<String> = None;
    let mut last_active_at: Option<String> = None;
    let mut version: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut used_tokens_5h: u64 = 0;
    let window_start_ms = now_ms().saturating_sub(5 * 3600 * 1000);

    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(c) = v.get("cwd").and_then(Value::as_str) {
            if cwd.is_empty() {
                cwd = c.to_string();
            }
            if canonical_cwd.is_empty() && encode_path(c) == project_key {
                canonical_cwd = c.to_string();
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
                    || v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)
                {
                    continue;
                }
                let content = v.pointer("/message/content");
                if let Some(text) = user_text(content) {
                    prompt = text;
                    last_action = "等待助手响应".into();
                    turn_ended = false;
                } else if content_has_tool_result(content) {
                    turn_ended = false;
                }
            }
            "queue-operation" => {
                if v.get("operation").and_then(Value::as_str) == Some("enqueue") {
                    if let Some(text) = queued_user_text(&v) {
                        prompt = text;
                        last_action = "等待助手响应".into();
                        turn_ended = false;
                    }
                }
            }
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

    Some(SessionSummary {
        provider: "claude".into(),
        session_id: session_id.to_string(),
        project_key,
        cwd: if canonical_cwd.is_empty() { cwd } else { canonical_cwd },
        title: String::new(),
        prompt,
        last_action,
        turn_ended,
        started_at,
        last_active_at,
        version,
        git_branch,
        mtime_ms: 0,
        created_ms: 0,
        line_count: 0,
        used_tokens_5h,
    })
}

/// 从 claude 进程命令行里取被 `--resume <id>` / `--resume=<id>` / `-r <id>` 指定的
/// 会话号。恢复的会话 started_at 很旧，靠时间配不上，只能从命令行认出来。
/// `--continue`（无显式 id）返回 None，交给 started_at/mtime 兜底。
fn resume_session_id(command: &str) -> Option<&str> {
    let looks_id = |s: &str| s.len() >= 8 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
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

/// ISO8601 → epoch 毫秒
fn iso_to_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
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
    {
        return None;
    }
    Some(truncate(trimmed, 500))
}

/// queue-operation enqueue 的 content：用户排队输入（过滤系统通知包装）
fn queued_user_text(v: &Value) -> Option<String> {
    let text = v.get("content").and_then(Value::as_str)?.trim().to_string();
    if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
        return None;
    }
    Some(truncate(&text, 500))
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
    Some(truncate(t, 500))
}

/// 把一行 Codex 会话记录转为简要消息（与 Claude 的 entry_to_brief 对应）。
/// 覆盖：用户/助手消息、function_call / custom_tool_call（工具行）及其输出（结果行）。
fn codex_entry_to_brief(v: &Value) -> Option<MessageBrief> {
    if v.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string();
    let p = v.get("payload")?;
    match p.get("type").and_then(Value::as_str)? {
        "message" => match p.get("role").and_then(Value::as_str)? {
            "user" => codex_user_text(v).map(|t| MessageBrief {
                role: "user".into(),
                content: t,
                timestamp: ts,
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
                    content: truncate(t, 2000),
                    timestamp: ts,
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
            Some(MessageBrief { role: "tool".into(), content, timestamp: ts })
        }
        "function_call_output" | "custom_tool_call_output" => {
            let out = p.get("output").and_then(Value::as_str).unwrap_or("");
            (!out.trim().is_empty()).then(|| MessageBrief {
                role: "tool_result".into(),
                content: truncate(out.trim(), 400),
                timestamp: ts,
            })
        }
        _ => None,
    }
}

fn entry_to_brief(v: &Value) -> Option<MessageBrief> {
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string();
    let ty = v.get("type").and_then(Value::as_str)?;
    if v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    match ty {
        "user" => {
            if v.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
                return None;
            }
            let content = v.pointer("/message/content");
            if let Some(text) = user_text(content) {
                return Some(MessageBrief { role: "user".into(), content: text, timestamp: ts });
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
                        });
                    }
                }
            }
            None
        }
        "queue-operation" => {
            if v.get("operation").and_then(Value::as_str) == Some("enqueue") {
                let text = queued_user_text(v)?;
                return Some(MessageBrief { role: "user".into(), content: text, timestamp: ts });
            }
            None
        }
        "assistant" => {
            let items = v.pointer("/message/content")?.as_array()?;
            let mut text_buf = String::new();
            let mut tools = Vec::new();
            let mut plan: Option<&str> = None;
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
                    content: truncate(p.trim(), 4000),
                    timestamp: ts,
                });
            }
            if !text_buf.trim().is_empty() {
                Some(MessageBrief {
                    role: "assistant".into(),
                    content: truncate(text_buf.trim(), 2000),
                    timestamp: ts,
                })
            } else if !tools.is_empty() {
                Some(MessageBrief {
                    role: "tool".into(),
                    content: truncate(&tools.join(" | "), 400),
                    timestamp: ts,
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
    let Some(input) = input else { return String::new() };
    for key in ["command", "file_path", "path", "pattern", "description", "prompt", "url"] {
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

/// 后台运行的任务（run_in_background 的命令 / 异步子代理）
#[derive(Debug, Clone, Serialize)]
struct BgTask {
    id: String,
    label: String,
    /// running | completed | failed | killed | stopped
    status: String,
}

/// 追踪会话里「在后台跑着」的任务。
///
/// 同样是跨记录的状态：
/// - 启动：tool_use 带 run_in_background=true，任务号要等 tool_result 里的
///   "…with ID: xxx"（子代理则是 "agentId: xxx"）才拿得到；
/// - 结束：后续某条 user 记录里的 <task-notification> 带 <task-id> 与 <status>。
#[derive(Default)]
struct BgTracker {
    /// tool_use_id -> 展示名（等 tool_result 回填任务号）
    pending: HashMap<String, String>,
    items: Vec<BgTask>,
    dirty: bool,
}

impl BgTracker {
    fn observe(&mut self, v: &Value) {
        // 完成通知不止一种落法：子代理/后台命令跑完时是一条 queue-operation，
        // 通知文本直接挂在顶层 content（字符串）上，不在 /message/content 里。
        // 只看 /message/content 的话，任务只进不出，永远停在「运行中」。
        if let Some(s) = v.get("content").and_then(Value::as_str) {
            self.on_notification(s);
        }
        // 挂在消息体上的：内容可能是纯字符串，也可能是分块数组
        if let Some(c) = v.pointer("/message/content") {
            match c {
                Value::String(s) => self.on_notification(s),
                Value::Array(items) => {
                    for item in items {
                        match item.get("type").and_then(Value::as_str) {
                            Some("tool_use") => self.on_tool_use(item),
                            Some("tool_result") => self.on_tool_result(item),
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(Value::as_str) {
                                    self.on_notification(t);
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

    fn on_tool_use(&mut self, item: &Value) {
        let input = item.get("input");
        let is_bg = input
            .and_then(|i| i.get("run_in_background"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !is_bg {
            return;
        }
        let Some(use_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let name = item.get("name").and_then(Value::as_str).unwrap_or("任务");
        // description 最贴近人看的说明，没有再退回工具名
        let label = input
            .and_then(|i| i.get("description"))
            .and_then(Value::as_str)
            .map(|s| truncate(s, 80))
            .unwrap_or_else(|| name.to_string());
        self.pending.insert(use_id.to_string(), label);
    }

    fn on_tool_result(&mut self, item: &Value) {
        let Some(use_id) = item.get("tool_use_id").and_then(Value::as_str) else {
            return;
        };
        let Some(label) = self.pending.remove(use_id) else {
            return;
        };
        let text = tool_result_text(item);
        let Some(id) = parse_bg_id(&text) else { return };
        self.items.push(BgTask { id, label, status: "running".into() });
        self.dirty = true;
    }

    /// 解析 <task-notification>：一条通知可能带多个 task-id，共用一个 status
    fn on_notification(&mut self, text: &str) {
        if !text.contains("<task-notification>") {
            return;
        }
        let Some(status) = tag_value(text, "status") else { return };
        // "__orphan_summary__:*" 是会话续跑/压缩恢复时的孤儿汇总标记：语义是「此前所有
        // 后台 shell/子代理都已不在」。它往往只枚举部分 id，漏网的若只按枚举清，会永远
        // 卡在「运行中」（run_in_background 起的 dev server / 测试 hub 尤其常见）。
        // 故一旦出现该标记，就把当时所有仍在跑的后台任务统一落到该终态，
        // 只留下最近一次恢复之后新起、当前真在跑的后台任务 —— 即「实时」语义。
        let is_orphan_summary = text.contains("__orphan_summary__");
        let mut rest = text;
        while let Some(id) = tag_value(rest, "task-id") {
            // "__orphan_summary__:*" 是内部扫描标记，不是真任务
            if !id.starts_with("__") {
                if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
                    if t.status != status {
                        t.status = status.clone();
                        self.dirty = true;
                    }
                }
            }
            let Some(pos) = rest.find("</task-id>") else { break };
            rest = &rest[pos + "</task-id>".len()..];
        }
        if is_orphan_summary {
            for t in self.items.iter_mut() {
                if t.status == "running" {
                    t.status = status.clone();
                    self.dirty = true;
                }
            }
        }
    }

    fn take_snapshot(&mut self, ts: &str) -> Option<MessageBrief> {
        if !self.dirty || self.items.is_empty() {
            return None;
        }
        self.dirty = false;
        Some(MessageBrief {
            role: "bgtasks".into(),
            content: serde_json::to_string(&self.items).ok()?,
            timestamp: ts.to_string(),
        })
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

/// 从后台任务的 tool_result 文本里取任务号：
/// 命令是 "…background with ID: xxx"，子代理是 "agentId: xxx"
fn parse_bg_id(text: &str) -> Option<String> {
    for marker in ["with ID: ", "agentId: "] {
        if let Some(p) = text.find(marker) {
            let rest = &text[p + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
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
        })
    }
}

/// 解析文件头部：初始 cwd、第一条真实用户提示词、会话开始时间
fn parse_head(path: &Path) -> HeadInfo {
    let mut info = HeadInfo::default();
    let Ok(mut f) = fs::File::open(path) else { return info };
    let mut buf = vec![0u8; HEAD_BYTES];
    let Ok(n) = f.read(&mut buf) else { return info };
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
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
            && !v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)
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
        t.observe(&assistant_tool("u1", "TaskCreate", serde_json::json!({ "subject": "甲" })));
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
        t.observe(&assistant_tool("u1", "TaskCreate", serde_json::json!({ "subject": "甲" })));
        t.observe(&tool_result("u1", "Task #1 created successfully: 甲"));
        t.observe(&assistant_tool("u2", "TaskCreate", serde_json::json!({ "subject": "乙" })));
        t.observe(&tool_result("u2", "Task #2 created successfully: 乙"));
        let _ = t.take_snapshot("ts");

        t.observe(&assistant_tool(
            "u3",
            "TaskUpdate",
            serde_json::json!({ "taskId": "2", "status": "completed" }),
        ));
        assert_eq!(t.items.iter().find(|i| i.id == "2").unwrap().status, "completed");
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

    fn bg_use(id: &str, name: &str, desc: &str) -> Value {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-17T10:00:00Z",
            "message": { "content": [
                { "type": "tool_use", "id": id, "name": name,
                  "input": { "command": "yarn start", "description": desc, "run_in_background": true } }
            ]}
        })
    }

    fn result(use_id: &str, text: &str) -> Value {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-07-17T10:00:01Z",
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": use_id, "content": text }
            ]}
        })
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

        t.observe(&result("u1", "Command running in background with ID: bhb69r9ff. Output..."));
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
        t.observe(&result("u1", "Async agent launched successfully.\nagentId: a21278fd478be0810 (internal)"));
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].id, "a21278fd478be0810");
    }

    /// 一条通知可带多个 task-id 共用一个 status；__orphan_summary__ 是内部标记要跳过
    #[test]
    fn notification_with_multiple_ids_skips_internal_markers() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&result("u1", "Command running in background with ID: b5eauqs4i."));
        t.observe(&bg_use("u2", "Bash", "乙"));
        t.observe(&result("u2", "Command running in background with ID: bmojunb33."));
        let _ = t.take_snapshot("ts");

        t.observe(&notification(
            "<task-notification>\n<task-id>b5eauqs4i</task-id>\n<task-id>bmojunb33</task-id>\n<task-id>__orphan_summary__:shell</task-id>\n<status>stopped</status>\n</task-notification>",
        ));
        assert!(t.items.iter().all(|i| i.status == "stopped"));
        assert_eq!(t.items.len(), 2, "内部标记不该混进清单");
    }

    /// 孤儿汇总只枚举了部分 id 时，漏网的运行中任务也应一并落终态：
    /// 会话续跑/压缩恢复后，此前所有后台 shell 都已不在，不能永远卡在「运行中」。
    #[test]
    fn orphan_summary_stops_unlisted_running_tasks() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "被枚举的"));
        t.observe(&result("u1", "Command running in background with ID: b5eauqs4i."));
        t.observe(&bg_use("u2", "Bash", "漏网的 dev server"));
        t.observe(&result("u2", "Command running in background with ID: bvvgsfndf."));
        let _ = t.take_snapshot("ts");

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
        t.observe(&result("u1", "Command running in background with ID: b1."));
        t.observe(&bg_use("u2", "Bash", "乙"));
        t.observe(&result("u2", "Command running in background with ID: b2."));

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
        assert_eq!(encode_path("D:\\Program\\Foo"), encode_path("d:\\program\\foo"));
        assert_eq!(encode_path("D:\\Program\\Foo"), "d--program-foo");
    }

    /// 非后台的普通命令不该被收进来
    #[test]
    fn foreground_command_is_ignored() {
        let mut t = BgTracker::default();
        let v = serde_json::json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "tool_use", "id": "u1", "name": "Bash",
                  "input": { "command": "ls", "description": "列目录" } }
            ]}
        });
        t.observe(&v);
        t.observe(&result("u1", "Command running in background with ID: zzz."));
        assert!(t.items.is_empty());
    }

    /// take_snapshot 反映的始终是当前状态，且取走后不重复产出
    #[test]
    fn snapshot_reflects_current_state_once() {
        let mut t = BgTracker::default();
        t.observe(&bg_use("u1", "Bash", "甲"));
        t.observe(&result("u1", "Command running in background with ID: b1."));

        let first = t.take_snapshot("ts").expect("首次应有快照");
        assert!(first.content.contains("running"));
        assert!(t.take_snapshot("ts").is_none(), "无变化不该重复产出");

        t.observe(&notification(
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
        ));
        let second = t.take_snapshot("ts").expect("状态变了该有新快照");
        assert!(second.content.contains("completed"));
        assert!(!second.content.contains("\"running\""), "应是原地更新而非追加一条");
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
            "message": { "content": [
                { "type": "tool_result", "tool_use_id": "u1",
                  "content": "Async agent launched successfully.\nagentId: a21278fd478be0810" }
            ]}
        }));
        assert_eq!(t.items[0].status, "running");

        // queue-operation：通知在顶层 content，message 整个不存在
        t.observe(&serde_json::json!({
            "type": "queue-operation",
            "operation": "enqueue",
            "content": "<task-notification>\n<task-id>a21278fd478be0810</task-id>\n<status>completed</status>\n</task-notification>"
        }));
        assert_eq!(t.items[0].status, "completed", "顶层 content 的通知必须被接住");
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
        f.write_all((create("u2", "乙") + &created("u2", 2, "乙")).as_bytes()).unwrap();
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
mod pairing_tests {
    use super::*;

    fn sess(id: &str, started: &str, mtime_ms: u64) -> SessionSummary {
        SessionSummary {
            provider: "claude".into(),
            session_id: id.into(),
            project_key: "-proj".into(),
            cwd: "/proj".into(),
            title: id.into(),
            prompt: String::new(),
            last_action: String::new(),
            turn_ended: true,
            started_at: Some(started.into()),
            last_active_at: None,
            version: None,
            git_branch: None,
            mtime_ms,
            created_ms: 0,
            line_count: 1,
            used_tokens_5h: 0,
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
        }
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

        let tasks = build_tasks(&[blank, old], &[p], &|_| false, &HashMap::new(), &HashSet::new());
        let b = tasks.iter().find(|t| t.id == "blank").unwrap();
        let o = tasks.iter().find(|t| t.id == "old").unwrap();
        assert_eq!(b.pid, Some(200), "进程应配给创建时刻≈启动的空白会话");
        assert_eq!(o.pid, None, "旧会话不该抢到进程（尽管 mtime 更新）");
        assert_eq!(o.status, TaskStatus::Finished);
    }

    /// 命令行 --resume <id>：恢复的会话 created_ms/started_at 都很旧，只能靠命令行认出。
    #[test]
    fn resume_command_pairs_old_session() {
        assert_eq!(resume_session_id("claude --resume abc12345-ef"), Some("abc12345-ef"));
        assert_eq!(resume_session_id("node x/claude.js -r sess-9999"), Some("sess-9999"));
        assert_eq!(resume_session_id("claude --continue"), None);

        let now = now_ms();
        let mut resumed = sess("resumed-xyz", "2026-06-01T00:00:00Z", now - 2_000);
        resumed.created_ms = now - 20 * 24 * 3600 * 1000; // 20 天前创建
        let mut p = proc(300, now / 1000 - 30);
        p.command = "claude --resume resumed-xyz".into();

        let tasks = build_tasks(&[resumed], &[p], &|_| false, &HashMap::new(), &HashSet::new());
        assert_eq!(tasks.iter().find(|t| t.id == "resumed-xyz").unwrap().pid, Some(300));
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

        let tasks = build_tasks(&[closed, cursor], &[p], &|_| false, &pinned, &HashSet::new());
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
        assert_eq!(encode_path("D:\\proj\\"), "D--proj");

        let now = now_ms();
        let mut s = sess("live", "2026-07-20T00:00:00Z", now - 30_000);
        s.project_key = "D--proj".into(); // 会话目录名（无尾随）
        s.cwd = "D:\\proj".into();
        let mut p = proc(4242, now);
        p.cwd = "D:\\proj\\".into(); // 进程 cwd 带尾随反斜杠
        p.tty = String::new();

        let tasks = build_tasks(&[s], &[p], &|_| false, &HashMap::new(), &HashSet::from(["live".to_string()]));
        // 配对成功 = 恰好一条任务、带 pid、状态非 Finished（不是占位进程）
        assert_eq!(tasks.len(), 1, "应配成一条，而非会话+占位进程两条");
        assert_eq!(tasks[0].pid, Some(4242));
        assert_ne!(tasks[0].status, TaskStatus::Finished);
        assert!(!tasks[0].prompt.contains("尚未产生记录"), "不该是占位进程");
    }

    /// 真实踩到的坑：`claude --resume` 起来的长会话，起始时间是几周前，
    /// 但此刻正在被写入；另一个会话起始更晚却早就没人管了。
    /// 按起始时间配对会让活着的那个配不到进程 → 判为 Finished → 从列表消失，
    /// 死的那个反倒顶着 pid 常驻显示「等待输入」。必须按最后活动时间配。
    #[test]
    fn alive_resumed_session_wins_over_recently_started_dead_one() {
        let now = now_ms();
        // 活着：起始很老（resume），但刚刚还在写
        let alive = sess("alive", "2026-06-26T02:29:18Z", now - 60_000);
        // 已死：起始更晚，但 4 小时没动静了
        let dead = sess("dead", "2026-07-16T16:16:19Z", now - 4 * 3600 * 1000);
        let sessions = vec![alive, dead];
        // 只有一个进程 → 只能有一个会话是活的
        let procs = vec![proc(3191, 1000)];

        let tasks = build_tasks(&sessions, &procs, &|_| false, &HashMap::new(), &HashSet::from(["alive".to_string()]));
        let by_id = |id: &str| tasks.iter().find(|t| t.id == id).unwrap().clone();

        assert_eq!(by_id("alive").pid, Some(3191), "正在写入的会话必须拿到进程");
        assert_eq!(by_id("dead").pid, None, "四小时没动静的会话不该顶着进程");
        assert_eq!(
            by_id("dead").status,
            TaskStatus::Finished,
            "配不到进程即已结束（列表会过滤掉）"
        );
    }

    /// 多进程时：最新的进程配最近活动的会话，多余的老会话落到 Finished
    #[test]
    fn pairs_most_recent_sessions_with_processes() {
        let now = now_ms();
        let sessions = vec![
            sess("newest", "2026-07-01T00:00:00Z", now - 10_000),
            sess("middle", "2026-07-02T00:00:00Z", now - 20_000),
            sess("stale", "2026-07-03T00:00:00Z", now - 3 * 3600 * 1000),
        ];
        let procs = vec![proc(100, 1000), proc(200, 2000)];

        let tasks = build_tasks(&sessions, &procs, &|_| false, &HashMap::new(), &HashSet::from(["newest".to_string(), "middle".to_string()]));
        let pid = |id: &str| tasks.iter().find(|t| t.id == id).unwrap().pid;

        // 两个进程 → 最近活动的两个会话拿到 pid
        assert_eq!(pid("newest"), Some(200), "最新进程配最近活动的会话");
        assert_eq!(pid("middle"), Some(100));
        assert_eq!(pid("stale"), None, "起始最晚但最久没活动的，不该拿到进程");
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
        let m = entry_to_brief(&user_entry("把服务启动，然后打开前台")).expect("真实用户消息必须保留");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "把服务启动，然后打开前台");
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
        assert!(codex_entry_to_brief(&injected).is_none(), "注入块不该冒充用户消息");

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
            session_id: id.into(),
            project_key: key.into(),
            cwd: format!("/w/{key}"),
            title: id.into(),
            prompt: String::new(),
            last_action: String::new(),
            turn_ended: true,
            started_at: None,
            last_active_at: None,
            version: None,
            git_branch: None,
            mtime_ms: now - 5_000,
            created_ms: 0,
            line_count: 1,
            used_tokens_5h: 0,
        };
        let proc = |agent: &str, pid: u32, key: &str| ProcessInfo {
            pid,
            agent: agent.into(),
            tty: format!("/dev/ttys{pid}"),
            cwd: format!("/w/{key}"),
            ide: crate::model::IdeKind::Terminal,
            ide_name: "Terminal".into(),
            start_time: 1000,
            cpu_usage: 0.0,
            memory: 0,
            command: agent.into(),
        };
        let sessions = vec![mk("claude", "c1", "-w-app"), mk("codex", "x1", "-w-app")];
        let procs = vec![proc("claude", 11, "app"), proc("codex", 22, "app"), proc("gemini", 33, "app")];

        let tasks = build_tasks(&sessions, &procs, &|_| false, &HashMap::new(), &HashSet::from(["c1".to_string(), "x1".to_string()]));
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
        }];
        let tasks = build_tasks(&[], &procs, &|_| false, &HashMap::new(), &HashSet::new());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Claude Code", "空目录名不该带「 · 」尾巴");
        assert!(!tasks[0].title.contains('·'));
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
