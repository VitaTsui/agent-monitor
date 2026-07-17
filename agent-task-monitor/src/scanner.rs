use crate::model::{MessageBrief, ProcessInfo, Task, TaskStatus};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个会话 jsonl 解析出的摘要（缓存单元）
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub path: PathBuf,
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
    cache: HashMap<PathBuf, CacheEntry>,
}

/// 会话列表最多回溯的时长（毫秒）：7 天
const HISTORY_WINDOW_MS: u64 = 7 * 24 * 3600 * 1000;
/// 摘要解析时读取的文件尾部大小
const TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// 头部读取大小（拿初始 cwd / 提示词 / 开始时间）
const HEAD_BYTES: usize = 256 * 1024;

impl SessionScanner {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self { projects_dir, cache: HashMap::new() }
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
                if let Some(summary) = self.summarize(&path, meta.len(), mtime_ms) {
                    out.push(summary);
                }
            }
        }
        out.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
        out
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

    /// 解析一个会话的对话消息（供前台对话流展示），返回最后 limit 条
    pub fn messages(&self, session_id: &str, limit: usize) -> Result<Vec<MessageBrief>> {
        let path = self.find_session_file(session_id)?;
        // 大文件只读尾部 8MB，足够渲染最近对话
        let tail = read_tail(&path, 8 * 1024 * 1024)?;
        let mut msgs = Vec::new();
        for line in tail.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if let Some(m) = entry_to_brief(&v) {
                msgs.push(m);
            }
        }
        let skip = msgs.len().saturating_sub(limit);
        Ok(msgs.into_iter().skip(skip).collect())
    }

    fn find_session_file(&self, session_id: &str) -> Result<PathBuf> {
        // 防路径穿越：session_id 只允许 uuid 字符
        if !session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            anyhow::bail!("非法会话 ID");
        }
        let projects = fs::read_dir(&self.projects_dir)?;
        for project in projects.flatten() {
            let candidate = project.path().join(format!("{session_id}.jsonl"));
            if candidate.is_file() {
                return Ok(candidate);
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
) -> Vec<Task> {
    // 按 cwd 分组进程（已按启动时间升序）
    // 进程按「编码后的 cwd」分组，与会话的项目目录名对齐（会话内 cwd 会漂移，目录名不会）
    let mut proc_by_key: HashMap<String, Vec<&ProcessInfo>> = HashMap::new();
    for p in processes {
        if p.agent == "claude" {
            proc_by_key.entry(encode_path(&p.cwd)).or_default().push(p);
        }
    }

    // 同一项目下：活跃会话按开始时间升序，与进程按启动时间升序一一配对
    let mut sess_by_key: HashMap<&str, Vec<&SessionSummary>> = HashMap::new();
    // 只有「近期活跃」的会话才参与进程配对（老会话大概率已结束）
    let now = now_ms();
    for s in sessions {
        if now.saturating_sub(s.mtime_ms) < 6 * 3600 * 1000 {
            sess_by_key.entry(s.project_key.as_str()).or_default().push(s);
        }
    }
    for list in sess_by_key.values_mut() {
        list.sort_by(|a, b| {
            a.started_at.cmp(&b.started_at) // ISO8601 字符串可直接比
        });
    }

    let mut pid_of_session: HashMap<&str, &ProcessInfo> = HashMap::new();
    for (key, procs) in &proc_by_key {
        if let Some(sess) = sess_by_key.get(key.as_str()) {
            // 倒序配对：最新的会话配最新的进程（resume 场景下老会话文件已停更）
            for (p, s) in procs.iter().rev().zip(sess.iter().rev()) {
                pid_of_session.insert(s.session_id.as_str(), p);
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
            provider: "claude".into(),
            provider_dsr: crate::model::provider_dsr("claude"),
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

    // 有进程但没配到任何会话（刚启动还没写文件）→ 生成占位任务
    let matched: std::collections::HashSet<u32> =
        pid_of_session.values().map(|p| p.pid).collect();
    for p in processes {
        if !matched.contains(&p.pid) {
            // 被系统挂起（非我方暂停）的占位进程判为孤儿，前台会过滤掉
            if crate::process::is_stopped(p.pid) && !manual_paused(p.pid) {
                continue;
            }
            let status = if manual_paused(p.pid) {
                TaskStatus::Paused
            } else {
                TaskStatus::Idle
            };
            tasks.push(Task {
                id: format!("pid-{}", p.pid),
                machine_id: String::new(),
                hostname: String::new(),
                platform: String::new(),
                platform_dsr: String::new(),
                provider: p.agent.clone(),
                provider_dsr: crate::model::provider_dsr(&p.agent),
                title: String::new(),
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
    let project_key = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
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
        session_id: session_id.to_string(),
        path: path.to_path_buf(),
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
        line_count: 0,
        used_tokens_5h,
    })
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
    if trimmed.is_empty()
        || trimmed.starts_with("<local-command")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<system-reminder>")
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
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            text_buf.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
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
    p.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn short_name(cwd: &str) -> String {
    cwd.rsplit(['/', '\\']).next().unwrap_or(cwd).to_string()
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
