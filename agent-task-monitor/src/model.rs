use serde::{Deserialize, Serialize};

/// 任务（会话）状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// 正在执行（进程存活且会话文件近期有写入）
    Running,
    /// 进程存活但等待用户输入（近期无写入）
    Idle,
    /// 已被暂停（SIGSTOP）
    Paused,
    /// 进程已退出（历史会话）
    Finished,
}

/// 宿主 IDE / 终端类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdeKind {
    Cursor,
    Vscode,
    Terminal,
    Other,
}

/// 匹配到的 AI 代理进程信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    /// 代理类型：claude / codex …
    pub agent: String,
    /// 控制终端设备（如 /dev/ttys004），用于终端级排除
    #[serde(default)]
    pub tty: String,
    /// 进程工作目录
    pub cwd: String,
    /// 宿主 IDE / 终端
    pub ide: IdeKind,
    /// 宿主应用名（如 Cursor / Code / Terminal）
    pub ide_name: String,
    /// 进程启动时间（epoch 秒）
    pub start_time: u64,
    /// 累计 CPU 使用率（瞬时）
    pub cpu_usage: f32,
    /// 常驻内存（字节）
    pub memory: u64,
    /// 命令行
    pub command: String,
}

/// 会话内一条简要消息（用于详情展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBrief {
    pub role: String,
    /// user 提示词 / assistant 文本 / 工具名
    pub content: String,
    pub timestamp: String,
}

/// 聚合后的「任务」：一个代理会话 + 可能匹配到的进程
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// 会话 ID（jsonl 文件名）
    pub id: String,
    /// 代理类型：claude / codex …
    pub provider: String,
    /// 项目目录（会话 cwd）
    pub project: String,
    /// 项目目录短名
    pub project_name: String,
    /// 会话标题：会话的首个用户提示词（原始任务），更像标题
    #[serde(default)]
    pub title: String,
    /// 当前任务描述：最近一条真实用户提示词
    pub prompt: String,
    /// 最近一次活动的动作描述（如正在调用的工具）
    pub last_action: String,
    pub status: TaskStatus,
    /// 状态中文描述（前端表格直接展示）
    pub status_dsr: String,
    /// 代理中文/展示名
    pub provider_dsr: String,
    /// 宿主 IDE 展示名（无进程为 "—"）
    pub ide_dsr: String,
    /// 进程 pid（无进程为 null）
    pub pid: Option<u32>,
    /// 所属机器 ID
    #[serde(default)]
    pub machine_id: String,
    /// 所属机器主机名
    #[serde(default)]
    pub hostname: String,
    /// 机器系统：macos / windows / linux
    #[serde(default)]
    pub platform: String,
    /// 机器系统展示名：Mac / Windows / Linux
    #[serde(default)]
    pub platform_dsr: String,
    /// 会话开始时间 ISO8601
    pub started_at: Option<String>,
    /// 最近活动时间 ISO8601
    pub last_active_at: Option<String>,
    /// 会话文件最近修改的 epoch 毫秒
    pub mtime_ms: u64,
    /// 会话消息条数（近似，取自解析尾部前的行数统计）
    pub line_count: u64,
    /// 近 5 小时滚动窗口 token 用量
    #[serde(default)]
    pub used_tokens_5h: u64,
    /// 当前生效的 token 上限（0 = 不限制）
    #[serde(default)]
    pub token_limit: u64,
    /// 是否因超额被自动暂停
    #[serde(default)]
    pub auto_paused: bool,
    /// 代理 CLI 版本
    pub version: Option<String>,
    /// git 分支
    pub git_branch: Option<String>,
    /// 匹配到的进程（无进程 = 历史会话）
    pub process: Option<ProcessInfo>,
    /// 最近若干条消息摘要（agent 上报时携带，供 hub 缓存）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_messages: Vec<MessageBrief>,
}

/// 控制动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlAction {
    /// 暂停（SIGSTOP）
    Pause,
    /// 恢复（SIGCONT）
    Resume,
    /// 中断当前任务（SIGINT，相当于按 Esc/Ctrl+C 一次）
    Interrupt,
    /// 终止进程（SIGTERM）
    Stop,
    /// 强杀（SIGKILL）
    Kill,
    /// 向会话注入输入（发布任务）
    Input,
}

#[derive(Debug, Deserialize)]
pub struct ControlReq {
    pub action: ControlAction,
    /// 可选：明确指定 pid（同目录多进程时消歧）
    pub pid: Option<u32>,
    /// action=Input 时要注入的文本
    #[serde(default)]
    pub text: Option<String>,
}

/// 一台被监控的电脑
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInfo {
    pub id: String,
    pub hostname: String,
    /// macos / windows / linux
    pub platform: String,
    /// Mac / Windows / Linux
    pub platform_dsr: String,
    /// agent 版本
    pub version: String,
    pub online: bool,
    /// 是否 hub 本机
    pub is_hub: bool,
    pub last_report_at: Option<String>,
    pub session_count: usize,
    pub running_count: usize,
    /// 归属用户名
    pub owner: Option<String>,
    /// 是否已信任（未信任设备不监控其会话）
    pub trusted: bool,
}

/// agent → hub 的快照上报
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPayload {
    pub machine_id: String,
    pub hostname: String,
    pub platform: String,
    pub version: String,
    /// 认领该设备的用户名（agent 侧 AM_USER）
    #[serde(default)]
    pub owner: Option<String>,
    pub tasks: Vec<Task>,
}

/// hub → agent 的待执行控制命令
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlCmd {
    pub task_id: String,
    pub pid: Option<u32>,
    pub action: ControlAction,
    #[serde(default)]
    pub text: Option<String>,
}

/// hub → agent 的待写入文件（传输文件到远程设备目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    /// 目标目录（agent 本机）
    pub dir: String,
    pub filename: String,
    /// base64 编码的文件内容
    pub content_b64: String,
}

pub fn platform_dsr(platform: &str) -> String {
    match platform {
        "macos" => "Mac".into(),
        "windows" => "Windows".into(),
        "linux" => "Linux".into(),
        other => other.into(),
    }
}

/// 监控端自身状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub hostname: String,
    pub platform: String,
    pub version: String,
    pub started_at: String,
    pub projects_dir: String,
    pub session_count: usize,
    pub running_count: usize,
    pub process_count: usize,
}

impl TaskStatus {
    pub fn dsr(&self) -> &'static str {
        match self {
            TaskStatus::Running => "执行中",
            TaskStatus::Idle => "等待输入",
            TaskStatus::Paused => "已暂停",
            TaskStatus::Finished => "已结束",
        }
    }
}

pub fn provider_dsr(provider: &str) -> String {
    match provider {
        "claude" => "Claude Code".into(),
        "codex" => "Codex".into(),
        other => other.into(),
    }
}
