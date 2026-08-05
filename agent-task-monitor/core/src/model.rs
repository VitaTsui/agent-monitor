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
    /// 终端锚：该 agent 最近的 shell 祖先 pid（powershell/bash…）。终端 shell 的 pid 跨
    /// claude 的 /clear、--resume、重启都不变，比易变的 claude pid 更适合做「会话↔进程」
    /// 配对的稳定锚。扫描时算好，供持久化/配对复用（见 process::ProcessScanner::nearest_shell）。
    #[serde(default)]
    pub shell_pid: Option<u32>,
    /// 终端锚 shell 的启动时间（epoch 秒）。与 shell_pid 一起唯一确定「同一个 shell」——
    /// Windows 会重用 pid：关掉终端再开一个可能拿到同一个 shell pid，光比 pid 会把新终端
    /// 错配到旧会话。配对恢复时须 pid + start 都对上才算同一 shell。
    #[serde(default)]
    pub shell_start: Option<u64>,
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
    /// 终端里 claude 原生排队、尚未被接受执行的输入（按入队顺序，供前端底部挂载显示）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued_inputs: Vec<String>,
    /// **终端此刻正等你选**：AskUserQuestion 的整份 input（questions/options）。
    ///
    /// 来自 PreToolUse hook，即在选项弹给终端用户**之前**就已知道 —— 因此远端能同步
    /// 弹出选项框、替终端做决定。（从 jsonl 读到的 select 消息是事后的，等它出现时
    /// 人早在终端上选完了。）用户选完即由后续 hook 覆盖清除，见 client/hookrec.rs。
    ///
    /// 存 `Value` 而不是「字符串里塞一份 JSON」：后者每经一层就再转义一次，
    /// 前端还得自己 parse 并兜住解析失败。结构化之后各层都能直接读它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_select: Option<serde_json::Value>,
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
    /// 向终端注入按键（不提交）：text 形如 "up:3"（按 3 次上键撤回排队）/ "esc"（插入排队）。
    /// 仅 iTerm2(mac) 与 Windows 控制台可干净注入；Terminal.app 不支持（前端走提示）。
    TermKey,
}

#[derive(Debug, Deserialize)]
pub struct ControlReq {
    pub action: ControlAction,
    /// 可选：明确指定 pid（同目录多进程时消歧）
    pub pid: Option<u32>,
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
    /// 本设备是否是「他人通过协助码共享给我」的（非本人设备）
    #[serde(default)]
    pub shared: bool,
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
    #[serde(default)]
    pub dir_results: Vec<DirResult>,
    /// 上一轮 hub 请求的文件夹操作结果（回传）。旧客户端不带 → 空。
    #[serde(default)]
    pub fs_op_results: Vec<FsOpResult>,
    /// 本机 agent 配置清单（只有哈希，没有内容）。旧客户端不带 → None，
    /// hub 据此判定「这台机器还不支持配置同步」，既不索要也不下发。
    #[serde(default)]
    pub config_manifest: Option<ConfigManifest>,
    /// 上一轮 hub 通过 `configPulls` 点名索要的文件内容（回传）。
    #[serde(default)]
    pub config_bodies: Vec<ConfigFileBody>,
    /// 结构化配置（settings.json / config.toml）的**字段普查**：只有键路径与类型，没有值。
    /// 二期字段级同步的准备工作，见 `ConfigProbe`。
    #[serde(default)]
    pub config_probe: Vec<ConfigProbe>,
    /// 本机结构化配置里**白名单字段**的当前值（字段级同步用）。
    /// 只有配置源机器上报的会进基线；镜像机上报的仅用于判断它是否已同步到位。
    #[serde(default)]
    pub config_patches: Vec<ConfigPatch>,
}

/// 结构化配置里的一个字段（**不含值**）。
///
/// 二期要做 settings.json 的字段级合并，而白名单必须建立在「用户实际用了哪些字段」之上——
/// 凭空猜一份白名单，等于拿猜测去改用户的配置文件。所以先做这一步只读普查。
///
/// 刻意不传值：settings.json 里混着 API key helper 路径之类的东西，
/// 上传值等于把它们复制进 hub，而普查根本不需要值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigKeyInfo {
    /// 点分键路径，如 `permissions.allow`
    pub path: String,
    /// 值类型：string / number / bool / array / object / null
    pub ty: String,
    /// 数组元素个数或对象键数（标量恒为 0）。用于判断字段规模，仍不涉及内容。
    #[serde(default)]
    pub len: usize,
    /// 值里出现了绝对路径或家目录变量 —— 机器相关，跨机同步会把另一台机器指向不存在的位置。
    #[serde(default)]
    pub machine_specific: bool,
}

/// 一份结构化配置文件的普查结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProbe {
    /// 文件标识：`claude/settings.json` 或 `codex/config.toml`
    pub file: String,
    pub keys: Vec<ConfigKeyInfo>,
}

/// 结构化配置的**字段级**同步单元（二期）。
///
/// 与 md 类配置的 `ConfigPush` 是两条完全不同的路径，不要混用：
/// 那边整份覆盖，这边只带白名单内的几个顶层字段，客户端读到后**合并**进本机文件，
/// 用户手写的其余字段（尤其 `hooks`）原样保留。
///
/// hub 上因此永远不存在一份完整的用户 settings.json 副本 —— 基线里只有
/// `SETTINGS_SYNC_KEYS` 圈定的那几个值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPatch {
    /// 文件标识：`claude/settings.json` 或 `codex/config.toml`
    pub file: String,
    /// 顶层字段名 → 值。只会出现白名单内、且值不含本机路径的字段。
    pub fields: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ConfigPatch {
    /// 内容指纹：字段集合一致则一致。用于判断某台设备是否已经与基线同步。
    ///
    /// 用 BTreeMap 是为了这个：HashMap 的迭代顺序不稳定，同样的内容会算出不同的指纹，
    /// 于是每一轮都判定「不一致」，无休止地重复下发。
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(&self.fields).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// 配置同步：单个文件的指纹。
///
/// `path` 一律是**归一化相对路径**（`claude/agents/x.md`、`codex/AGENTS.md`），
/// 绝不放绝对路径 —— mac 的 `/Users/vita/...` 推到 Windows 机器上既拼不出目标位置，
/// 又把本机用户名泄露给同账号的其它设备。目标绝对路径由客户端自己用本机 home 拼。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileMeta {
    pub path: String,
    /// 内容的 sha256（小写十六进制）。差异判定只看它，不看 mtime ——
    /// 各机器时钟不一定同步，mtime 比大小会把「时钟慢的那台」永远判成落后。
    pub sha256: String,
    pub size: u64,
    /// 本机修改时间（unix 秒）。仅用于客户端自己的扫描缓存与界面展示。
    #[serde(default)]
    pub mtime: u64,
}

/// 配置同步：一台机器的全部在管配置文件指纹
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigManifest {
    pub files: Vec<ConfigFileMeta>,
    /// 扫描完成时刻（unix 秒）
    #[serde(default)]
    pub scanned_at: u64,
}

/// agent → hub：被点名索要的配置文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileBody {
    pub path: String,
    pub content_b64: String,
    /// 内容哈希，hub 落基线前复验，防止传输途中截断
    pub sha256: String,
}

/// hub → agent：待写入的配置文件。
///
/// 不复用 `FileTransfer`：那条链路的 `dir` 是**绝对路径**（hub 并不知道对端 home 在哪），
/// 且 `write_transfer` 是直接整份覆盖、不备份 —— 用户的 CLAUDE.md / agents 被无声盖掉
/// 是不可接受的。这里只给相对路径，客户端拼本机 home 并走「备份 + 原子写」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPush {
    /// 归一化相对路径，同 `ConfigFileMeta::path`
    pub path: String,
    pub content_b64: String,
    /// 内容哈希，客户端落盘前复验
    pub sha256: String,
}

/// hub → agent：列出会话目录下某相对子路径的子目录（上传选目录用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirQuery {
    pub task_id: String,
    /// 会话项目目录（agent 本机路径，作为根，不允许越出）
    pub cwd: String,
    /// 相对根的子路径（"" 表示根本身），分隔符统一 '/'
    pub rel: String,
}

/// agent → hub：目录列表结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirResult {
    pub task_id: String,
    pub rel: String,
    /// 子目录名（仅目录；已排序，隐藏目录靠后）
    pub dirs: Vec<String>,
    /// 该目录下的文件名（已排序，隐藏文件靠后）。用于「选择文件回填相对路径」。
    /// 旧客户端不带该字段 → 反序列化为空。
    #[serde(default)]
    pub files: Vec<String>,
}

/// hub → agent：会话目录内的文件夹操作（上传选目录弹窗里新建/删除/重命名）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOp {
    /// 操作 id：网页据此轮询结果
    pub op_id: String,
    pub task_id: String,
    /// 会话项目目录（agent 本机路径，作为根，不允许越出）
    pub cwd: String,
    /// 目标所在相对目录（"" = 根），分隔符统一 '/'
    pub rel: String,
    /// 操作类型：mkdir / delete / rename
    pub op: String,
    /// 目标名（rel 下的目录/文件名）
    pub name: String,
    /// rename 的新名（其余操作忽略）
    #[serde(default)]
    pub new_name: String,
}

/// agent → hub：文件夹操作结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpResult {
    pub op_id: String,
    pub ok: bool,
    #[serde(default)]
    pub msg: String,
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
    /// 队列指令 id：网页据此查询「还在排队」与撤回（旧客户端忽略该字段）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// hub → agent 的待写入文件（传输文件到远程设备目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    /// 目标目录（agent 本机）
    pub dir: String,
    pub filename: String,
    /// base64 编码的文件内容（分片传输时是这一片的内容）
    pub content_b64: String,
    /// 分片序号（0 起）。非分片传输恒为 0。
    ///
    /// 大文件必须切片：整份读进内存再 base64 会膨胀 1/3，还要在 hub 的下发队列里
    /// 驻留到 agent 来取 —— 一个 100MB 的文件就能让 hub 吃掉 130MB+。
    #[serde(default)]
    pub chunk_index: u32,
    /// 分片总数。0 或 1 都表示「不是分片，就这一份」。
    ///
    /// 旧版 agent 不认识这两个字段，反序列化时按 default 取 0，于是走原来的整份覆盖
    /// 写入路径 —— 语义正好落在「非分片」上，不会把某一片当成完整文件写坏。
    /// 但也因此，hub 必须确认对端版本够新才允许分片（见 server 的上传处理）。
    #[serde(default)]
    pub chunk_total: u32,
}

pub fn platform_dsr(platform: &str) -> String {
    match platform {
        "macos" => "Mac".into(),
        "windows" => "Windows".into(),
        "linux" => "Linux".into(),
        other => other.into(),
    }
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
        "gemini" => "Gemini CLI".into(),
        "aider" => "Aider".into(),
        "opencode" => "OpenCode".into(),
        other => other.into(),
    }
}
