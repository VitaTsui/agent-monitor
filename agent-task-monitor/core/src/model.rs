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
    /// 桌面客户端（ChatGPT.app / Claude.app）拉起的代理进程：没有控制终端、
    /// 父链里既没有 IDE 也没有终端模拟器，但有一个 GUI 应用包在托着它。
    Desktop,
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
    /// **共享宿主进程**：一个进程同时托着多条会话，它自己的 cwd 没有会话含义。
    ///
    /// 唯一来源是桌面客户端的服务端进程（实测 ChatGPT 桌面版的
    /// `…/ChatGPT.app/Contents/Resources/codex … app-server`，cwd 恒为 `/`）。
    /// 终端会话是「一进程一会话、cwd 即项目」，这条不是 —— 所以它不参与按 cwd 的
    /// 配对，也绝不单独生成占位任务（那正是当初要把 app-server 整个挡掉的原因）。
    #[serde(default)]
    pub shared_host: bool,
}

/// 一次工具调用。
///
/// 此前一条 assistant 记录里的多次工具调用被 `" | "` 拼成一个字符串塞进
/// [`MessageBrief::content`] —— 一条消息对多个调用，谁也认不出哪一段对应哪一次。
/// 于是执行链上那次派子代理的 `Task` 调用，和它派出来的 [`SubTask`]，
/// 除了「展示名长得像」之外没有任何可靠的对应关系（而且两边截断长度还不一样：
/// 120 vs 80）。要在正文里就地画出子代理卡片，就必须一次调用一个元素、各带自己的 id。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// 这次调用的 `tool_use_id`（Codex 那边是 `call_id`）。
    ///
    /// 它就是 [`SubTask::tool_use_id`] 要对上的那个值。老记录里可能没有，那就是空串 ——
    /// **不要猜**，配不上就当作「这次调用没派出子代理」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// 工具名（Read / Bash / Agent …）
    pub name: String,
    /// 入参摘要（命令 / 文件路径 / 描述，截到 120 字）。没有可展示的入参时是空串。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

impl ToolCall {
    /// 一行人读的说法：`名字: 摘要`。
    ///
    /// 钉钉推送、MCP 会话摘要这类纯文本出口用它 —— 渲染只此一处，
    /// 不在各消费方各拼一遍（那正是当初 `" | "` 拼接扩散开的原因）。
    pub fn line(&self) -> String {
        if self.hint.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, self.hint)
        }
    }
}

/// 会话内一条简要消息（用于详情展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBrief {
    pub role: String,
    /// user 提示词 / assistant 文本 / 工具名
    pub content: String,
    pub timestamp: String,
    /// **这一步跑砸了**：`tool_result` 块上的 `is_error`。
    ///
    /// 原始记录里一直带着（实测本机 `~/.claude/projects` 25222 个块里 997 个为真），
    /// 此前 `entry_to_brief` 只取了文本、把它丢掉 —— 于是前端执行链上每一步长得
    /// 一模一样，跑成的和跑砸的没有任何区别，一轮里到底哪一步出的错看不出来。
    ///
    /// **只在为真时下发**（与 [`Task::live_cwd`] 同一套口径：没什么可说就不出这个键）。
    /// 失败是少数派，把两万多条 `isError: false` 塞进每次轮询纯粹是搬运。
    /// 前端按「有这个键且为真 = 失败」判，缺失即不失败，老客户端上报的数据不受影响。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// **这条记录里的每一次工具调用**，一次一个元素（只有 `role == "tool"` 才有）。
    ///
    /// 取代了原先把多次调用 `" | "` 拼进 [`Self::content`] 的做法 —— 那样拼出来的
    /// 字符串没法反查是哪几次调用，执行链上也就画不出「这一次派了哪个子代理」。
    /// `role == "tool"` 时 `content` 不再下发，纯文本出口走 [`ToolCall::line`]。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolCall>,
    /// **它回应的是哪一次调用**（只有 `role == "tool_result"` 才有）：那次调用的
    /// `tool_use_id`。前端据此把结果贴回执行链上对应的那一步，不必按顺序猜。
    /// 老记录拿不到就是空串。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_use_id: String,
}

impl MessageBrief {
    /// 这条消息的**纯文本形态**：给钉钉推送、MCP 摘要这类只能出文字的出口用。
    ///
    /// `role == "tool"` 的消息正文在 [`Self::tools`] 里而不在 `content` 上，
    /// 直接读 `content` 会拿到空串（那正是「钉钉推送里工具调用变成一行空白」的来源）。
    /// 所有纯文本出口都走这里，渲染只此一处。
    pub fn text(&self) -> String {
        if self.tools.is_empty() {
            self.content.clone()
        } else {
            self.tools
                .iter()
                .map(ToolCall::line)
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }
}

/// 子任务的**结构化收尾归类**。
///
/// 为什么不让前端直接看 [`SubTask::status`]：那是上游 Claude Code 写在
/// `<task-notification>` 里的原文（`completed` / `failed` / `killed` / `stopped`），
/// 语义并不是「成/败」——实测 `killed` 是**父会话被中断或退出时，一次性给当时所有
/// 在跑子代理统一发的收尾通知**（同一时刻三条同状态），子代理自己一点毛病没有；
/// `stopped` 则是有人主动 `TaskStop`。前端若按字面量把这两种一并画成红色失败，
/// 用户看到的就是「我按了 Esc，结果一排子代理全爆红」。
///
/// 归类的判据是**磁盘事实**而不是文案：子会话自己那份 `subagents/agent-<id>.jsonl`
/// 的收尾形态说明它到底有没有把结果交回去（见 scanner 的 `SubAgentTail`）。
/// 上游改文案不会让这个归类失灵。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubTaskOutcome {
    /// 还在跑
    Running,
    /// 正常收尾（交回了结果）
    Completed,
    /// **它自己跑砸了**：卡死、API 报错、退出码非零，原因在 `summary` 里
    Failed,
    /// **被连带终止**：父会话退出/被打断（`killed`），或有人主动停掉（`stopped`）。
    /// 不是这个子任务的错，前端不该画成失败色。
    Interrupted,
}

/// 一个会话名下「在后台跑着（或跑过）的东西」：异步子代理，或后台命令。
///
/// 此前这份清单是序列化成一条 `role:"bgtasks"` 的消息塞在消息流末尾的 —— 前端得先
/// 从对话里把它摘出来再 `JSON.parse`，还得自己滤掉这条不让它出现在聊天记录里。
/// 现在它是 [`Task`] 上的结构化字段，消息流里不再有这条伪消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubTask {
    /// 任务号。`kind == "agent"` 时就是父会话记录里的 `agentId`，也是拉取正文
    /// （`/monitor/tasks/:id/subagents/:agentId/messages`）要用的那个 id；
    /// `kind == "bg"` 时是 `backgroundTaskId`。
    pub id: String,
    /// `agent`（异步子代理，有独立会话记录）| `bg`（后台命令，没有）
    pub kind: String,
    /// 展示名（派活时的 description，截断到 80 字）
    pub label: String,
    /// **上游原文**：running / completed / failed / killed / stopped。
    /// 保留它只为可追溯（排障时要能对上会话记录里那句通知），
    /// 前端配色一律看 [`Self::outcome`]。
    pub status: String,
    /// 结构化归类，见 [`SubTaskOutcome`]
    pub outcome: SubTaskOutcome,
    /// 起跑时刻（会话记录里的 ISO8601 时间戳），拿不到就空串
    pub started_at: String,
    /// 收尾时刻（epoch 毫秒）。0 = 还没结束。
    ///
    /// 此前这个字段是 `#[serde(skip)]` 的纯内部值，于是前端算不出耗时，
    /// 更要命的是**没有任何东西能据以淘汰旧条目**（见 scanner 的保留窗口）。
    pub ended_ms: u64,
    /// 完成通知里的 `<summary>` 原文 —— 唯一说得出「为什么是这个收场」的字段。
    /// 按磁盘改判状态时会清掉（那句话描述的已不是当前状态）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 磁盘上有它自己的会话记录，可以按需拉正文。
    /// 后台命令（`kind == "bg"`）恒为 false —— 它没有独立记录。
    pub has_body: bool,
    /// **起跑那次工具调用的 `tool_use_id`** —— 与 [`ToolCall::id`] 相等即为同一次。
    ///
    /// 前端靠它把执行链上那次 `Agent`/`Bash` 调用，精确对应到这个子任务，
    /// 从而在正文里就地画出子代理卡片。此前两边唯一的交集是展示名字符串，而
    /// 一条消息可能对应多次调用，按名字配根本不可靠。
    ///
    /// 拿不到就是空串（老记录、或起跑记录落在重放窗口之外）。
    /// **空就是空，不要猜** —— 配不上时前端不把这次调用显示成子代理卡。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_use_id: String,
    /// **这是第几次派活**（1 起）。同一个子代理可以被反复叫起来干活：实测本机
    /// `a7f78026084ce8753` 在父记录里有 8 条时刻各不相同的收尾通知
    /// （20:28、21:06、21:10…），每一条都是一次真的重新派活。
    ///
    /// 这个字段存在的理由是**让「它又跑起来了」与「刚才那次判错了」区分得开**：
    /// 本条记录描述的永远是**最近一次**运行（`status` / `ended_ms` / `tool_use_id`
    /// 都跟着换），所以光看 `outcome` 从终态翻回 `running` 是分不清两者的。
    /// `runs` 变了 = 新的一次派活（合法）；`runs` 没变却翻回 `running` = 有 bug。
    ///
    /// 后者现在不该再发生：磁盘推断出来的终态是**吸收态**，只有新的一次派活
    /// （新的 `tool_use_id`）才能把它重新打开，见 scanner 的 `BgTracker::settled`。
    #[serde(default = "one")]
    pub runs: u32,
}

/// `SubTask::runs` 的默认值：老客户端上报的数据里没有这个字段，
/// 按「跑过一次」算 —— 默认 0 会让前端把每条已有记录都当成「还没派过」。
fn one() -> u32 {
    1
}

/// 聚合后的「任务」：一个代理会话 + 可能匹配到的进程
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// 会话 ID（jsonl 文件名）
    pub id: String,
    /// 代理类型：claude / codex …
    pub provider: String,
    /// 项目目录（会话 cwd）。**这是归一化后的「项目根」**，不随会话内 `cd` 漂移 ——
    /// 它要与 `~/.claude/projects` 下的目录名对得上，会话↔进程配对和分组都靠它稳定
    /// （见 scanner 的 `canonical_cwd`）。要「终端此刻在哪」请用 [`Task::live_cwd`]。
    pub project: String,
    /// 项目目录短名
    pub project_name: String,
    /// **会话此刻的工作目录**：jsonl 尾部最后一条记录的 `cwd`。
    ///
    /// 与 `project` 的区别就是这个 bug 的全部：会话内 `cd` 进子目录后，jsonl 里的 cwd
    /// 跟着走，而 `project` 被钉死在项目根。网页拿 `project` 当上传落点、又回填**相对**
    /// 路径 `./tmp/x.png`，终端却按自己当前的 cwd 解析 —— 文件写在 A、终端在 B 找，
    /// 表现为「上传成功但终端说文件不存在」。凡是要与终端的相对路径对齐的地方
    /// （上传落点、目录浏览根、文件夹操作根）都该用它。
    ///
    /// None = 尾窗里一条 cwd 都没读到（极短会话/占位任务），调用方退回 `project`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_cwd: Option<String>,
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
    /// 代理中文/展示名。由 `(provider, desktop)` 这一对经 [`provider_dsr`] /
    /// [`provider_dsr_desktop`] 算出来，**不是客户端自由上报的字符串**。
    pub provider_dsr: String,
    /// **这条会话来自桌面客户端而不是终端 CLI**（Claude 桌面版的本地代理、
    /// ChatGPT 桌面版的 Codex）。
    ///
    /// 同一台机器上，`provider == "codex"` 既可能是 Codex CLI，也可能是
    /// ChatGPT 桌面版 —— 它们是**两个不同的客户端**，只是会话文件格式一样。
    /// 此前这个事实只体现在 `provider_dsr` 那个展示字符串上（scanner 里按它选展示名，
    /// 见 `build_tasks`），Task 本身不带 —— 于是上层要区分「哪个客户端」时手里只有
    /// 一个中文串可抓，按 provider 聚合就会把两个客户端糊成一组（实测本机 33 条 codex
    /// 里 32 条是 CLI、1 条是桌面版，糊在一起后 32 条 CLI 会话被挂在「ChatGPT 桌面版」
    /// 这个组名下）。判据本来就在扫描那一层是个布尔量，带上来即可，不必去猜字符串。
    #[serde(default)]
    pub desktop: bool,
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
    /// 该会话名下的后台子任务（异步子代理 + 后台命令），agent 上报时携带。
    ///
    /// 只带「近期」的：见 scanner 的 `BG_RETAIN_MS` / `BG_MAX_ITEMS` —— 此前这张表
    /// 从会话开头全量重放且**没有任何淘汰**，实测本机单个会话能累到 147 条、
    /// 里头最老的已经是 11 天前的事，且永远不会消失。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_tasks: Vec<SubTask>,
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

/// 一台设备上「有哪一类终端、各有多少条会话」。
///
/// 侧栏要按「设备 × 终端类型」分组，而在此之前没有任何接口能直接回答这个问题 ——
/// 前端只能拉 `/monitor/sessions/history?limit=200`（接口上限）去数最近 200 条倒推。
/// 那是个将就：某个终端最近一条会话一旦排在 200 条之外，这一轮就数不出来，
/// 对应的分组会凭空消失。所以由服务端直接给。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStat {
    /// claude / codex …（与会话上的 `Task::provider` 同一个取值）
    pub provider: String,
    /// 终端 CLI 还是桌面客户端，见 [`Task::desktop`]。
    ///
    /// **同一个 `provider` 会出现两项**（如 `codex/false` 与 `codex/true`）——
    /// 它们是同一台机器上两个不同的客户端，用户要的是「单独显示每个客户端的会话」，
    /// 糊成一项就不满足。分组键是 `(provider, desktop)` 这一对。
    pub desktop: bool,
    /// 展示名：由 `(provider, desktop)` 算出的**规范名**，不是某一条会话上的值。
    /// 取某条会话的值会让组名随最近那条漂（1 条桌面版会话能把 32 条 CLI 会话的组
    /// 改名成「ChatGPT 桌面版」）。
    pub provider_dsr: String,
    /// 该设备该 `(provider, desktop)` 下的会话总数，**含已结束**。
    /// 与 `/monitor/sessions/history?machineId=&provider=&desktop=` 的 `total` 同口径。
    pub session_count: usize,
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
    /// 这台设备上有哪几类终端、各有多少条会话（见 [`ProviderStat`]）。
    ///
    /// 没有任何会话时是**空数组而不是省略字段** —— 前端要区分「这台机器确实没会话」
    /// 和「老版本 hub 不给这个字段」。
    #[serde(default)]
    pub providers: Vec<ProviderStat>,
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
    /// **热列表**：近期（见 scanner 的 `LIVE_WINDOW_MS`）有活动的会话，每轮全量重报。
    pub tasks: Vec<Task>,
    /// **历史会话**：比热列表更老、但仍在回溯窗口（`AM_HISTORY_DAYS`，默认 30 天）内的会话。
    ///
    /// 为什么单开一条而不是并进 `tasks`：客户端 1.5s 一轮全量重报，实测 30 天窗口下
    /// 一轮 94 条、序列化 92 KB —— 每天 5 GB 的上行，只为一批一动不动的已结束会话。
    /// 所以它每 30 秒才带一次（见 client 的 `HISTORY_REPORT_INTERVAL_SECS`），
    /// 而热列表那条路径的开销一点没变（仍是 8 条、11 KB）。
    ///
    /// `None` = 本轮没带（沿用 hub 上一次收到的那份），`Some(空表)` = 确实一条历史都没有。
    /// 两者必须分开：当成空表处理的话，历史列表会每 30 秒闪空一次。
    #[serde(default)]
    pub history_tasks: Option<Vec<Task>>,
    #[serde(default)]
    pub dir_results: Vec<DirResult>,
    /// 上一轮 hub 请求的文件夹操作结果（回传）。旧客户端不带 → 空。
    #[serde(default)]
    pub fs_op_results: Vec<FsOpResult>,
    /// 上一轮 hub 点名现取的文件内容（回传）。旧客户端不带 → 空。
    #[serde(default)]
    pub file_fetch_results: Vec<FileFetchResult>,
    /// 上一轮 hub 点名现读的会话数据（回传）。旧客户端不带 → 空。
    #[serde(default)]
    pub session_fetch_results: Vec<SessionFetchResult>,
    /// 上一轮下发文件的实际落盘路径（回传）。旧客户端不带 → 空，hub 退回自己算的名字。
    #[serde(default)]
    pub file_results: Vec<FileTransferResult>,
    /// 本机 agent 配置清单（只有哈希，没有内容）。旧客户端不带 → None，
    /// hub 据此判定「这台机器还不支持配置同步」，既不索要也不下发。
    #[serde(default)]
    pub config_manifest: Option<ConfigManifest>,
    /// 上一轮 hub 通过 `configPulls` 点名索要的文件内容（回传）。
    #[serde(default)]
    pub config_bodies: Vec<ConfigFileBody>,
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
    /// 会话项目目录（agent 本机路径，作为根，不允许越出）。
    ///
    /// **hub 算的这份必然偏旧**：它来自定期扫描上报的快照，而扫描循环在 macOS 后台会被
    /// App Nap 压到一两分钟一轮。会话期间 `cd` 过之后，拿它当根就会定位到别处 ——
    /// 用户看到的是「上传/选择文件列出来的是另一个目录」。故新客户端改用 [`Self::by_session`]。
    /// 这里仍然填着，纯为旧客户端兜底。
    pub cwd: String,
    /// 相对根的子路径（"" 表示根本身），分隔符统一 '/'
    pub rel: String,
    /// **由 agent 自己按 `task_id` 现读会话记录解析根**，而不是用上面那份 `cwd`。
    ///
    /// 新鲜度因此等同于本次往返本身，扫描循环再慢也不影响。旧客户端不认识这个字段，
    /// 反序列化取 false → 照旧用 `cwd`，行为不变。
    #[serde(default)]
    pub by_session: bool,
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
    /// agent 实际据以列举的**绝对根**（不含 `rel`）。空 = 旧客户端没回报。
    ///
    /// 网页拿它当上传落点与相对路径的基准 —— 只有 agent 知道会话此刻真正在哪，
    /// hub 手里那份是旧的。
    #[serde(default)]
    pub root: String,
}

/// hub → agent：现取一个会话目录内的文件（网页要看 agent 输出里引用的截图）。
///
/// **只为中转，不为存储**：hub 拿到内容后只在内存里放很短一会儿、交给等着的那个
/// 网页请求就丢掉 —— 会话内容不落我方存储是这个项目的既定原则，截图同样算会话内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFetch {
    /// 本次取件的标识，结果按它认领
    pub fetch_id: String,
    /// 会话项目目录（agent 本机路径，作为根，不允许越出）。旧客户端兜底用。
    pub cwd: String,
    /// 相对根的子路径，分隔符统一 '/'
    pub rel: String,
    /// 会话 id（`by_session` 为真时据此解析根）
    #[serde(default)]
    pub task_id: String,
    /// 同 [`DirQuery::by_session`]。会话内容里的相对图片路径也是终端按当前目录写下的，
    /// 用旧快照的根解析，会话 `cd` 过之后就会全变破图。
    #[serde(default)]
    pub by_session: bool,
}

/// agent → hub：现取文件的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFetchResult {
    pub fetch_id: String,
    /// 失败原因（不存在/越界/过大）。非空即失败，此时 content_b64 为空。
    #[serde(default)]
    pub err: String,
    /// 按魔数判定的 MIME（不看扩展名 —— 扩展名是内容里写的，改个名就能让页面按别的类型解析）
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub content_b64: String,
}

/// hub → agent 要的是会话的哪一份数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionWant {
    /// 会话本身的正文
    Messages,
    /// 某个子会话的正文（要 `agent_id`）
    Subagent,
    /// 该会话的**全部**子任务清单（不套保留窗口）
    Subtasks,
}

/// hub → agent：**现读一份会话数据**（历史会话正文 / 子会话正文 / 全量子任务清单）。
///
/// 为什么必须有这条通路：agent 每轮上报只给「活跃会话」（有进程，或 10 分钟内有写入）
/// 带最近 80 条消息与近 24 小时的子任务，hub 的 `/messages` 读的就是这份上报缓存 ——
/// 于是所有已结束的历史会话点开必然是空白；子会话正文更是从来没有被报上来过
/// （本机实测 422 份 `subagents/agent-*.jsonl`，一个字节都没上去过）。
///
/// 全量推是不可行的，所以走「点名现取」：网页要看哪一份，hub 就排一条这个请求，
/// agent 下一轮上报把结果带回来。与 [`FileFetch`] 同一套节奏。
///
/// 三种要求共用一条通路、一个 `want` 判别，不各开各的队列 —— 排队、去重、TTL、
/// 等待窗口这些全是同一套，复制三份只会三处各错一次。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFetch {
    /// 本次取件的标识，结果按它认领
    pub fetch_id: String,
    /// 会话 id（jsonl 文件名）
    pub task_id: String,
    pub want: SessionWant,
    /// 子会话号（= 父记录里的 `agentId`），只有 `want == Subagent` 时有意义
    #[serde(default)]
    pub agent_id: String,
    /// 正文最多返回多少条（agent 侧会再夹一道上限）；子任务清单忽略它
    pub limit: usize,
    /// 这条会话**已经没有进程在跑了**。
    ///
    /// 只有 hub 手里有聚合后的进程状态，解析器没有。据此给会话名下仍挂在「执行中」
    /// 的后台命令收尾 —— 父会话都结束了，它派生的后台命令不可能还在跑。
    #[serde(default)]
    pub parent_ended: bool,
}

/// agent → hub：现读会话数据的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFetchResult {
    pub fetch_id: String,
    /// 失败原因（会话记录不存在 / 读不动）。非空即失败，此时两份内容都为空。
    #[serde(default)]
    pub err: String,
    #[serde(default)]
    pub messages: Vec<MessageBrief>,
    #[serde(default)]
    pub sub_tasks: Vec<SubTask>,
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
    /// 同 [`DirQuery::by_session`]：由 agent 按 `task_id` 现读会话记录解析根。
    /// 必须与目录浏览用同一个根，否则「在网页上看到的目录」和「操作落到的目录」会是两个。
    #[serde(default)]
    pub by_session: bool,
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
    /// 这条 Input 是在回答终端弹出的选择卡（选项序号或自定义答案），不是主动发的任务。
    ///
    /// 客户端据此**跳过「补回车」**（见 client 的 PendingSubmit）：补回车的判据是「会话
    /// 最新用户消息不是刚发的那条 ⇒ 没提交成功」，而选择卡的作答永远不会成为一条用户
    /// 消息，判据恒成立 —— 于是每答一题必补两个回车，正好打在下一题上、替人选了默认项。
    /// 表现是「答完第一题，剩下的题自己就没了」。
    ///
    /// 选择卡本就不需要这道保险：数字键按下即落定，没有「文字进了输入框却没提交」那回事。
    #[serde(default)]
    pub from_select: bool,
}

/// hub → agent 的待写入文件（传输文件到远程设备目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    /// 目标目录（agent 本机绝对路径）。`by_session` 为真时忽略它，改用
    /// `会话当前目录 + rel_dir`；旧客户端不认新字段，仍然只看这里。
    pub dir: String,
    /// 会话 id（`by_session` 为真时据此解析落点根）
    #[serde(default)]
    pub task_id: String,
    /// 相对会话当前目录的子路径（"" = 就落在会话当前目录），分隔符统一 '/'
    #[serde(default)]
    pub rel_dir: String,
    /// **由 agent 在落盘那一刻解析目标目录**，而不是用 hub 事先算好的 `dir`。
    ///
    /// 这一步把「定位」推到了最晚的时刻：hub 排队、网络往返期间会话若又 `cd` 了，
    /// 事先算的绝对路径就已经过时。agent 落盘时现算，再把实际路径回报回去
    /// （见 [`FileTransferResult::path`]），网页据此回填，两端永远说的是同一个位置。
    #[serde(default)]
    pub by_session: bool,
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
    /// 本次传输的标识：非空表示 hub 要求 agent 回报**实际落盘路径**（见 [`FileTransferResult`]）。
    ///
    /// 名字的最终决定权在 agent 手里 —— 目标已存在时它会改名成 `图片 (1).jpg`（不覆盖）。
    /// 此前这个新名字没有回程，hub 拼进任务正文的路径仍是自己算的原名，指向目录里那个
    /// **旧文件**：agent 照着读得到内容、不报错，只是读的是上一版。本字段就是那条回程。
    ///
    /// 分片传输只认第 0 片定下的名字，回报在最后一片落完时发一次。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transfer_id: String,
}

/// agent → hub：文件实际落到了哪里。
///
/// 只在 [`FileTransfer::transfer_id`] 非空时回报。失败也必须回报（`ok=false`）——
/// hub 那边有个等结果的窗口，不回报它只能干等到超时，再拿自己算的名字去拼路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferResult {
    pub transfer_id: String,
    /// 实际落盘的绝对路径（agent 本机）。失败时为空。
    #[serde(default)]
    pub path: String,
    pub ok: bool,
    /// 失败原因（解码失败/目标越界/写盘失败）。
    #[serde(default)]
    pub err: String,
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

/// 桌面客户端会话的展示名。
///
/// 同一个 provider 既能从终端跑（Claude Code / Codex CLI），也能从桌面客户端跑
/// （Claude 桌面版的本地代理、ChatGPT 桌面版的 Codex）。两者的会话文件格式一样、
/// 控制方式却完全不同，列表里必须一眼看得出这条是从哪儿来的。
pub fn provider_dsr_desktop(provider: &str) -> String {
    match provider {
        "claude" => "Claude 桌面版".into(),
        "codex" => "ChatGPT 桌面版".into(),
        other => format!("{} 桌面版", provider_dsr(other)),
    }
}
