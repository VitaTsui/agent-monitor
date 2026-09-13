/**************************************
 * Module Name : 前台-任务监控对话页
 **************************************/

import { get, post, del } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

import { inDesktopClient } from "@/utils/clientAuth";

export interface PortalTaskProcess {
  pid: number;
  agent: string;
  cwd: string;
  ide: string;
  ideName: string;
  startTime: number;
  cpuUsage: number;
  memory: number;
  command: string;
}

/** 执行链上的一次工具调用 */
export interface ToolCall {
  /**
   * 这次调用的 `tool_use_id`（Codex 那边是 `call_id`）。
   *
   * 它就是 {@link SubTask.toolUseId} 要对上的那个值：`tool.id === subTask.toolUseId`
   * 即为同一次派活，据此在正文的执行链里就地画出子代理卡片。
   * **老记录可能没有（空串/缺失）——配不上就别显示成智能体卡，不要退回按名字猜**：
   * 一条消息可能对应多次调用，展示名两边截断长度还不一样（120 vs 80）。
   */
  id?: string;
  /** 工具名（Read / Bash / Agent …） */
  name: string;
  /** 入参摘要（命令 / 文件路径 / 描述，最长 120 字）；没有可展示入参时不下发 */
  hint?: string;
}

export interface PortalMessage {
  role: string;
  /**
   * 正文。**`role === "tool"` 时是空的** —— 那类消息的内容在 {@link tools} 里。
   *
   * 此前一条记录里的多次工具调用被 `" | "` 拼进这里，拼完就再也认不出哪一段是哪一次
   * 调用；那条旧路径已经删掉，不存在「两个字段都能用」的过渡期。
   */
  content: string;
  timestamp: string;
  /** 本地乐观回显（发送后立即上屏，终端同步回同内容后被替换） */
  local?: boolean;
  /** 回显对应的队列指令 id（撤回用） */
  cmdId?: string;
  /** 仍在 hub 队列排队、还没被客户端取走 */
  queued?: boolean;
  /** 已送达终端（写入其输入队列），等待终端执行；真实消息同步回来后回显被替换 */
  delivered?: boolean;
  /**
   * 这条是在回答终端弹出的选择卡（选项序号或自定义答案）。
   * 单看内容说明不了什么（孤零零一个「1」），问题本身又不在流里 —— 不进对话流。
   */
  fromSelect?: boolean;
  /**
   * **这一步跑砸了**（只出现在 `role === "tool_result"` 上）。
   *
   * 后端只在为真时下发这个键（`am-core` `model.rs` 的 `MessageBrief::is_error`），
   * 所以「缺失」就是「没出错」，不必和 `false` 区分。老版客户端上报的数据里没有
   * 这个键，执行链退回改前的样子（每一步都不标失败），不会报错。
   */
  isError?: boolean;
  /**
   * **这条记录里的每一次工具调用**，一次一个元素（只有 `role === "tool"` 才有）。
   * 渲染执行链请遍历它，不要再读 `content`。
   */
  tools?: ToolCall[];
  /**
   * **它回应的是哪一次调用**（只有 `role === "tool_result"` 才有）：那次调用的
   * `tool_use_id`。据此把结果贴回执行链上对应的那一步，不必按先后顺序猜。
   * 老记录拿不到时不下发。
   */
  toolUseId?: string;
}

/** AskUserQuestion 的一道题 */
export interface SelectQuestion {
  question?: string;
  header?: string;
  /**
   * 这道题可多选。终端里的交互也随之不同：单选按一下数字键即落定，多选要逐个
   * 数字键勾选、最后回车提交底部的 Submit —— 远端注入必须照着这个节奏来，
   * 否则终端停在选择卡上不动（选项卡不消失、其他端也跟着不消失）。
   *
   * 数据一直都在：pendingSelect 存的是 AskUserQuestion 的整份 tool_input，
   * 只是此前这里没声明、UI 便一律当单选处理。
   */
  multiSelect?: boolean;
  options?: { label?: string; description?: string }[];
}

/** AskUserQuestion 的整份 input：终端此刻在等你回答的东西 */
export interface SelectPayload {
  questions?: SelectQuestion[];
}

/**
 * 子任务的**结构化收尾归类**——前端配色一律看它，不要去认 `status` 里那几个英文词。
 *
 * - `running` 还在跑
 * - `completed` 正常收尾（把结果交回去了）
 * - `failed` **它自己跑砸了**（卡死 / API 报错 / 退出码非零），原因在 `summary` 里
 * - `interrupted` **被连带终止**：父会话退出或被打断（后端原文 `killed`），
 *   或有人主动停掉（原文 `stopped`），又或者父会话都结束了它还挂着
 *   （原文 `orphaned`，这一个是后端合成的：父会话没了，它派生的后台命令不可能还在跑）。
 *   不是这个子任务的错，别画成失败色。
 *
 * 为什么不能看 `status`：实测 `killed` 是父会话被中断时**一次性发给当时所有在跑子代理
 * 的统一通知**（同一时刻三条同状态），子代理本身一点毛病没有。按字面量配色的结果就是
 * 「按了一下 Esc，一排子代理全爆红」。归类由后端按磁盘事实（子会话记录有没有交回结果）
 * 算好，上游改文案不会让它失灵。
 */
export type SubTaskOutcome = "running" | "completed" | "failed" | "interrupted";

/** 一个会话名下「在后台跑着（或跑过）的东西」：异步子代理，或后台命令 */
export interface SubTask {
  /**
   * 任务号。`kind === "agent"` 时就是 agentId，也是拉子会话正文
   * （{@link getPortalSubAgentMessages}）要传的那个 id；`kind === "bg"` 时是后台命令号。
   */
  id: string;
  /** `agent` = 异步子代理（有独立会话记录，可展开看正文）｜`bg` = 后台命令（没有） */
  kind: "agent" | "bg";
  /** 展示名（派活时的说明，后端已截到 80 字） */
  label: string;
  /**
   * 上游原文：`running` / `completed` / `failed` / `killed` / `stopped`；
   * 外加一个后端合成的 `orphaned`（父会话已结束、后台命令不可能还在跑）。
   * **只用于排障展示**（要能和会话记录里那句通知对上），配色看 {@link outcome}。
   */
  status: string;
  outcome: SubTaskOutcome;
  /** 起跑时刻 ISO8601；拿不到是空串 */
  startedAt: string;
  /** 收尾时刻（epoch 毫秒）。0 = 还没结束，可据此与 startedAt 算耗时 */
  endedMs: number;
  /** 「为什么是这个收场」的原文一句话。只有拿到完成通知才有；按磁盘改判状态时会清掉 */
  summary?: string;
  /** 磁盘上有它自己的会话记录，可以展开拉正文。`kind === "bg"` 恒为 false */
  hasBody: boolean;
  /**
   * **起跑那次工具调用的 `tool_use_id`** —— 与 {@link ToolCall.id} 相等即为同一次。
   *
   * 执行链里就地画子代理卡片靠的就是它：`tool.id === subTask.toolUseId`。
   * 拿不到时不下发（老记录、或起跑记录落在重放窗口之外）；
   * **空就是空，别拿 label 去凑**。热路径的 `subTasks` 与按需读盘的
   * {@link getPortalSubTasks} 都带这个字段。
   */
  toolUseId?: string;
  /**
   * **这是第几次派活**（1 起）。同一个子代理可以被反复叫起来干活 ——
   * 实测本机 `a7f78026084ce8753` 在父会话记录里有 8 条时刻各不相同的收尾通知。
   *
   * 本条记录描述的永远是**最近一次**运行（`status`/`outcome`/`endedMs`/`toolUseId`
   * 都跟着换），所以光看 `outcome` 从终态翻回 `running` 分不清两种情况。判据是这个字段：
   * - `runs` 变了 → **它又跑起来了**，卡片该翻回「执行中」、该重新起轮询；
   * - `runs` 没变却翻回 `running` → 那是后端 bug，不是真相。现在不该再出现
   *   （磁盘推断出来的终态是吸收态，只有新的一次派活才能重新打开它）。
   */
  runs?: number;
}

interface IPortalTaskData {
  id: string;
  title: string;
  usedTokens5h: number;
  tokenLimit: number;
  autoPaused: boolean;
  provider: string;
  providerDsr: string;
  /**
   * 这条会话属于**终端 CLI（`false`）还是桌面客户端（`true`）** ——
   * 与 {@link DeviceProvider.desktop}、{@link HistorySession.desktop} 是同一个键。
   *
   * 侧栏分组直接用 `(provider, desktop)` 这一对判，**不要**再去「拿历史快照反查」或
   * 「按该设备该 provider 只有唯一一项」推断：同机同时跑 Codex CLI 与 ChatGPT 桌面版、
   * 且这条会话还没进历史列表时，反查会把它落到隔壁组。
   *
   * 进程占位任务（会话记录还没生成）没有会话文件可判，一律按 CLI 算（`false`）。
   */
  desktop: boolean;
  /** 项目根（归一化，不随会话内 cd 漂移）——分组、标题用 */
  project: string;
  projectName: string;
  /**
   * 会话**此刻**的工作目录（会话内 cd 后跟着走；与 project 相同时后端不下发）。
   *
   * 凡是要和终端的相对路径对齐的地方都用它：上传落点、目录浏览根、`./x` 回填。
   * 用 project 的话，会话 cd 进子目录后文件会写到项目根、终端却在子目录里找。
   */
  liveCwd?: string;
  prompt: string;
  lastAction: string;
  status: string;
  statusDsr: string;
  ideDsr: string;
  pid: number | null;
  machineId: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  startedAt: string | null;
  lastActiveAt: string | null;
  mtimeMs: number;
  lineCount: number;
  version: string | null;
  gitBranch: string | null;
  process: PortalTaskProcess | null;
  /**
   * 号位：钉钉里「@N 内容」用的编号，由 hub 按终端锚分配、跨重启保持不变。
   * 网页/客户端显示同一个号，用户才能在手机上照着网页说「@3 继续」。
   * 未参与编号（如占位任务）时为空。
   */
  slot: number | null;
  /** 终端里 claude 原生排队、尚未被接受执行的输入（按入队顺序） */
  queuedInputs: string[];
  /**
   * 终端**此刻正等你选**：AskUserQuestion 的整份 input（questions/options）。
   *
   * 由 PreToolUse hook 在选项弹给终端用户**之前**报上来，所以远端能同步弹出选项框、
   * 替终端做决定。（对话流里的 select 消息是事后从 jsonl 读到的 —— 等它出现时，
   * 人早在终端上选完了，那份只能当记录看。）用户选完即由后续 hook 清除。
   */
  pendingSelect?: SelectPayload;
  /**
   * 用户给这个会话起的名字。有它就盖过自动标题（见 _utils/sessionNote 的 sessionTitle）。
   *
   * 挂在**终端窗口**上而不是会话 id 上，`/clear`、`--resume`、hub 重启都不丢；
   * 没起过名字时后端下发 null。
   */
  note?: string | null;
  /**
   * 该会话名下的后台子任务（异步子代理 + 后台命令），可直接当作这条会话的**子节点**渲染。
   *
   * 以前这份数据是伪装成一条 `role: "bgtasks"` 的消息塞在消息流末尾的，消费方得自己
   * 从对话里摘出来再 `JSON.parse`、还得记着别把它渲染成聊天气泡。**那条消息已经没有了**，
   * 改读这个字段。
   *
   * 只带「近期」的：后端按收尾时刻保留 24 小时、最多 50 条终态条目（还在跑的不淘汰）。
   * 此前从会话开头全量累积且永不淘汰，实测单条会话能挂到 147 条、最老的是 11 天前的。
   */
  subTasks: SubTask[];
}
export type PortalTaskData = Partial<IPortalTaskData>;

export type PortalControlAction = "pause" | "resume" | "interrupt" | "stop" | "kill";

/** 当前登录用户信息（含实时 isSuper）；前端每次加载调一次刷新本地缓存，改权限无需重登 */
export interface MeInfo {
  id: string;
  username: string;
  nickname: string;
  isSuper: boolean;
}
export const getMe = async () => {
  return await get<MeInfo>("/monitor/me");
};

// 会话列表（平铺参数过滤）
export const getPortalTaskList = async (params?: {
  status?: string;
  keyword?: string;
}) => {
  return await get<ListRes<PortalTaskData>>("/monitor/tasks", { params });
};

/** 一条远程交互记录：我发的指令，或它给回的结果 */
export interface SessionHistoryItem {
  id: string;
  owner: string;
  /** 所属会话（jsonl id），同一会话的往来会连成一串 */
  sessionId: string;
  /** user = 我下发的；assistant = 它给回的 */
  role: "user" | "assistant";
  content: string;
  /** 发生时刻（epoch 秒） */
  at: number;
  /** 下发来源：dingtalk / web / mcp；assistant 条为空 */
  source: string;
  /** 会话在钉钉里的号位（@N 的 N），终端关太久被回收则为 null */
  slot: number | null;
  hostname: string;
  project: string;
  title: string;
  provider: string;
  /**
   * 用户给这个终端起的名字。**没起过名字时后端连字段都不下发**（不是 null）——
   * 备注不落盘，是读取时按 anchor 现 join 上去的（见 hub 的 history::with_note）。
   * 所以判空只能靠 falsy，不能靠 `"note" in e`。
   */
  note?: string;
  /**
   * 备注的 join 键（machineId|终端锚）。纯内部字段，不渲染 ——
   * 存量记录没有它，读回来是空串，于是匹配不到任何备注、回落到 title。
   */
  anchor?: string;
}

/**
 * 远程交互历史：把「我发了什么 → 它回了什么」按时间排成一条对话流。
 * 返回最近 limit 条，且保持正序（旧 → 新），直接从上往下渲染即是聊天记录的读法。
 *
 * @param session 只看某个会话的往来；不传则返回该账号的全部
 */
export const getSessionHistory = async (limit?: number, session?: string) => {
  return await get<ListRes<SessionHistoryItem>>("/monitor/history", {
    params: { limit, session },
  });
};

/**
 * 会话正文的返回形状。
 *
 * `pending === true` 表示「这台机器还没把正文送回来」——不是出错，也不是空会话：
 * 历史会话的正文是 hub 点名让那台机器现读磁盘取回来的，一次往返要两轮上报
 * （客户端约 1.5s 一轮），hub 最多等 8 秒。前端此时该显示「读取中」并过一会儿再问一次，
 * **不要**把它渲染成空对话。
 */
export interface MessagesRes {
  list: PortalMessage[];
  pending: boolean;
}

/**
 * 会话正文。活跃会话读的是客户端随上报捎带的缓存（秒级新鲜）；
 * **已结束的历史会话**改由后端现去那台机器读磁盘 —— 此前这种会话点开永远是空白。
 */
export const getPortalTaskMessages = async (id: string, limit?: number) => {
  return await get<MessagesRes>(`/monitor/tasks/${id}/messages`, {
    params: { limit },
  });
};

/** {@link getPortalSubTasks} 的返回形状；`pending` 语义同 {@link MessagesRes} */
export interface SubTasksRes {
  list: SubTask[];
  pending: boolean;
}

/**
 * 一条会话的**全部**子任务清单 —— 左侧列表里把主会话展开成子会话节点用这个。
 *
 * 与 `PortalTaskData.subTasks` 是同一个 {@link SubTask} 结构、两种口径：
 * - `subTasks` 随会话快照下发，是**当前状态面板**（只留近 24 小时、最多 50 条终态，
 *   而且只有活跃会话才带）；
 * - 这条是**全量视角**，按需读盘、不套保留窗口，历史会话照样展得开。
 *   活跃会话调它同样成立，结果是 `subTasks` 的超集。
 *
 * 会话不存在 → 404（与 {@link getPortalSubAgentMessages} 对齐）。
 * 同样要处理 `pending`（后端现去那台机器读盘，最多等 8 秒）。
 */
export const getPortalSubTasks = async (id: string) => {
  return await get<SubTasksRes>(`/monitor/tasks/${id}/subtasks`);
};

/**
 * **子会话正文**：把某个子代理展开成一串对话。
 *
 * @param id 父会话 id
 * @param agentId 取自 `PortalTaskData.subTasks[].id`（只有 `kind === "agent"` 且
 *   `hasBody` 为真的才拉得到；后台命令没有独立记录）
 *
 * 返回结构与 {@link getPortalTaskMessages} 完全一致（同一套 {@link PortalMessage}），
 * 可以直接复用现有的消息渲染。一律现读磁盘、不缓存，所以同样要处理 `pending`。
 */
export const getPortalSubAgentMessages = async (
  id: string,
  agentId: string,
  limit?: number,
) => {
  return await get<MessagesRes>(
    `/monitor/tasks/${id}/subagents/${agentId}/messages`,
    { params: { limit } },
  );
};

/**
 * 设置或清除会话备注。传空串 = 清除，恢复自动标题。
 *
 * 超长（>100 字）后端报 400 而不是静默截断，msg 里带实际字数，直接展示即可。
 */
export const setPortalTaskNote = async (id: string, note: string) => {
  return await post<{ note: string | null }>(`/monitor/tasks/${id}/note`, { note });
};

export interface SlashCommand {
  name: string;
  desc: string;
  source: string;
}

// 该会话模型可用的斜杠命令
export const getPortalSlashCommands = async (id: string) => {
  return await get<ListRes<SlashCommand>>(`/monitor/tasks/${id}/slash-commands`);
};

// 任务控制
export const controlPortalTask = async (
  id: string,
  action: PortalControlAction,
  pid?: number | null
) => {
  return await post<{ pid: number; result: string }>(
    `/monitor/tasks/${id}/control`,
    { action, pid }
  );
};

// 向会话发布任务（注入一行输入）
/**
 * 分片粒度（5MB）。与 hub 的 UPLOAD_BODY_LIMIT（12MB）配套 —— 单片加上 multipart
 * 边界与其它字段留足富余。
 *
 * 不切得更小是因为每片都是一次完整往返（鉴权、multipart 解析、base64、入队），
 * 片太多时这些固定开销会盖过传输本身。
 */
export const UPLOAD_CHUNK_SIZE = 5 * 1024 * 1024;

export const sendPortalInput = async (
  id: string,
  text: string,
  pid?: number | null,
  /** 这条是在回答选择卡（选项序号/自定义答案）：hub 据此不推钉钉、不进交互历史 */
  fromSelect?: boolean
) => {
  return await post<{ pid: number; result: string; cmdId?: string }>(
    `/monitor/tasks/${id}/input`,
    // source 让 hub 区分「客户端 / 网页」下发来源，用于钉钉推送正文标注
    { text, pid, source: inDesktopClient() ? "client" : "web", fromSelect }
  );
};

// ---------- 历史会话（含已结束的） ----------

interface IHistorySession {
  /** 会话 id（jsonl 文件名），拉正文时当 taskId 用 */
  id: string;
  /** 会话标题 = 首个用户提示词 */
  title: string;
  /** 最近一条真实用户提示词 */
  prompt: string;
  /** running / idle / paused / finished */
  status: string;
  statusDsr: string;
  provider: string;
  providerDsr: string;
  /** 这条会话属于终端 CLI 还是桌面客户端 —— 与 {@link DeviceProvider.desktop} 同一个键 */
  desktop: boolean;
  /** 项目根（归一化，不随会话内 cd 漂移） */
  project: string;
  projectName: string;
  machineId: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  startedAt: string | null;
  lastActiveAt: string | null;
  /** 会话文件最近修改时刻（epoch 毫秒）——列表的排序键，也是翻页游标 */
  mtimeMs: number;
  lineCount: number;
  gitBranch: string | null;
  /** 用户给这个终端起的名字；没起过是 null */
  note: string | null;
}
export type HistorySession = Partial<IHistorySession>;

export interface HistorySessionRes {
  list: HistorySession[];
  /** 过滤后的总条数（不受分页影响） */
  total: number;
  /** 还有更旧的：把它原样当下一页的 `before` 传回来。null = 到底了 */
  nextCursor: number | null;
}

/**
 * 历史会话列表（**不过滤已结束的**，这正是它与 {@link getPortalTaskList} 的区别）。
 *
 * 回溯多久由客户端的 `AM_HISTORY_DAYS` 决定，默认 30 天。
 *
 * 翻页用**时间游标**而不是页码：这份列表的底料是每轮上报刷新的内存快照，翻页期间
 * 新会话会插进头部，用 offset 会让某条被跳过或看两遍。要下一页就把上一次返回的
 * `nextCursor` 原样填进 `before`。
 */
export const getHistorySessionList = async (params?: {
  /** 模糊过滤：项目 / 标题 / 提示词 / 主机名 */
  keyword?: string;
  /** 只看某台机器 */
  machineId?: string;
  /** 只看某个 provider（claude / codex …） */
  provider?: string;
  /**
   * 只看终端 CLI（`false`）或只看桌面客户端（`true`）；不传 = 两者都要。
   *
   * 点开某个侧栏分组拉它的历史时，**必须把 {@link DeviceProvider.desktop} 一起传**：
   * 光传 `provider` 会把 Codex CLI 与 ChatGPT 桌面版的会话混在一起拉回来。
   */
  desktop?: boolean;
  /** 上一页返回的 nextCursor */
  before?: number;
  /** 每页条数，1~200，默认 50 */
  limit?: number;
}) => {
  return await get<HistorySessionRes>("/monitor/sessions/history", { params });
};

// ---------- 设备管理（信任设备）----------

/** 一台设备上「有哪几个客户端、各有多少条会话」——侧栏按「设备 × 客户端」分组用 */
export interface DeviceProvider {
  /** `claude` / `codex`，与 {@link HistorySession.provider} 同一套取值 */
  provider: string;
  /**
   * 终端 CLI（`false`）还是桌面客户端（`true`）。
   *
   * **同一个 `provider` 会出现两项**（如 `codex/false` = Codex CLI、
   * `codex/true` = ChatGPT 桌面版）：它们是同一台机器上两个不同的客户端，
   * 只是会话文件格式一样。分组键是 `(provider, desktop)` 这一对，别只按 provider 分。
   */
  desktop: boolean;
  /**
   * 组名：由 `(provider, desktop)` 算出的**规范名**（服务端两个固定枚举），
   * 不是某一条会话上的值 —— 组名不会随最近那条会话漂。直接当分组标题用。
   */
  providerDsr: string;
  /**
   * 该设备该 `(provider, desktop)` 下的会话总数，**含已结束**。
   * 与 `getHistorySessionList({ machineId, provider, desktop })` 的 `total` 同口径。
   */
  sessionCount: number;
}

export interface PortalDevice {
  id: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  version: string;
  online: boolean;
  isHub: boolean;
  sessionCount: number;
  runningCount: number;
  owner: string | null;
  trusted: boolean;
  /** 是否是「他人协助码共享给我」的设备 */
  shared: boolean;
  /**
   * 这台设备上有哪几个客户端、各有多少条会话。按会话数降序（同数按 provider 名、
   * CLI 在桌面版之前），
   * 顺序稳定，可直接照序渲染分组。
   *
   * 别再用 `getHistorySessionList({ limit: 200 })` 数最近 200 条倒推 —— 那是将就：
   * 某个终端最近一条会话一旦排到 200 条之外，对应分组就会凭空消失。
   *
   * **没有会话时是空数组**（不是缺字段）。设备离线仍返回上次已知的那份 ——
   * 否则笔记本一合盖，侧栏分组就全没了；本 hub 生命周期内从未上报过的设备才是空数组。
   */
  providers: DeviceProvider[];
}

export const getPortalDevices = async () => {
  return await get<ListRes<PortalDevice>>("/monitor/devices");
};

export const trustPortalDevice = async (id: string) => {
  return await post(`/monitor/devices/${id}/trust`, {});
};

export const untrustPortalDevice = async (id: string) => {
  return await post(`/monitor/devices/${id}/untrust`, {});
};

export const deletePortalDevice = async (id: string) => {
  return await del(`/monitor/devices/${id}`);
};

// ---------- 协助共享（跨用户接入） ----------

export interface ShareInfo {
  code: string;
  temporary: boolean;
  expiresAt: number;
}

export interface ShareCreated extends ShareInfo {
  password: string;
}

/** 查看本设备当前协助码（主人） */
export const getShareInfo = async (id: string) => {
  return await get<ShareInfo | null>(`/monitor/share/${id}`);
};

/** 生成/刷新协助码（主人）；temporary=true 用系统生成的临时密码 */
export const createShare = async (
  id: string,
  temporary: boolean,
  password?: string,
) => {
  return await post<ShareCreated>(`/monitor/share/${id}`, { temporary, password });
};

/** 停止共享（主人） */
export const revokeShare = async (id: string) => {
  return await del(`/monitor/share/${id}`);
};

/** 当前接入的访客（主人） */
export const getShareGuests = async (id: string) => {
  return await get<ListRes<string>>(`/monitor/share/${id}/guests`);
};

/** 踢掉访客（主人） */
export const kickShareGuest = async (id: string, user: string) => {
  return await post(`/monitor/share/${id}/kick`, { user });
};

/** 访客用连接码 + 密码接入他人设备 */
export const connectShare = async (code: string, password: string) => {
  return await post<{ machineId: string }>("/monitor/share/connect", {
    code,
    password,
  });
};

/** 访客断开自己的接入 */
export const disconnectShare = async (machineId: string) => {
  return await post("/monitor/share/disconnect", { machineId });
};



// ---------- 额度（5h token 上限）----------



// ---------- 文件传输到指定设备目录 ----------

/**
 * 上传一个文件（超过分片粒度的自动切片顺序上传）。
 *
 * 切片是必需的而非优化：hub 会把收到的这一片整个读进内存再 base64（膨胀 1/3），
 * 还要在下发队列里驻留到 agent 来取。不切的话，一个上百 MB 的文件能直接把 hub 顶爆，
 * 而且请求体也会撞上服务端的体积上限。
 *
 * 顺序而非并发：agent 侧按 FIFO 收到分片后依次追加写入，乱序会写出错乱的文件。
 *
 * @param onProgress 已发送字节数 / 总字节数，用于展示进度
 */
export const uploadPortalFile = async (
  id: string,
  dir: string,
  file: File,
  onProgress?: (sent: number, total: number) => void,
  /**
   * 指定落盘文件名（默认用 file.name）。
   *
   * 调用方先查过目标目录、算出了不撞名的名字时传它 —— 这样「实际落盘的名字」与
   * 「回填进输入框的名字」出自同一处，不会各说各话。
   */
  asName?: string,
  /**
   * 会话 id + 相对该会话**当前**目录的子路径。
   *
   * 带上它们，落点就由 agent 在写盘那一刻现算（它现读会话记录拿到当前 cwd），
   * 而不是用 `dir` 那份 hub 事先算好的绝对路径 —— 后者来自定期扫描的快照，
   * 会话期间 `cd` 过就已经指向别处。`dir` 仍然要传，作旧客户端的兜底。
   */
  session?: { taskId: string; relDir: string },
) => {
  const url = `/monitor/devices/${id}/upload`;
  const total = file.size;
  const name = asName || file.name;
  // 小文件不切：多带两个字段没意义，也省得旧版 hub/agent 走到分片分支上
  if (total <= UPLOAD_CHUNK_SIZE) {
    const form = new FormData();
    form.append("dir", dir);
    // 显式传文件名（UTF-8 文本字段）：multipart 的 Content-Disposition filename 对非 ASCII
    // （如粘贴图片的「粘贴-xxx.png」）编码在服务端会被解歪，导致落盘名与回填名对不上。
    form.append("name", name);
    if (session?.taskId) {
      form.append("taskId", session.taskId);
      form.append("relDir", session.relDir);
    }
    form.append("file", file);
    const res = await post<{ path?: string; result?: string; size: number }>(url, form);
    onProgress?.(total, total);
    return res;
  }

  const chunkTotal = Math.ceil(total / UPLOAD_CHUNK_SIZE);
  let last!: Awaited<ReturnType<typeof post<{ path?: string; result?: string; size: number }>>>;
  for (let i = 0; i < chunkTotal; i += 1) {
    const start = i * UPLOAD_CHUNK_SIZE;
    const blob = file.slice(start, Math.min(start + UPLOAD_CHUNK_SIZE, total));
    const form = new FormData();
    form.append("dir", dir);
    form.append("name", name);
    form.append("chunkIndex", String(i));
    form.append("chunkTotal", String(chunkTotal));
    if (session?.taskId) {
      form.append("taskId", session.taskId);
      form.append("relDir", session.relDir);
    }
    form.append("file", blob, name);
    last = await post<{ path?: string; result?: string; size: number }>(url, form);
    // 任一片失败即中止：继续传后面的只会在 agent 那边拼出一个残缺却"看着成功"的文件
    if (last.code !== 0) {
      return last;
    }
    onProgress?.(Math.min(start + UPLOAD_CHUNK_SIZE, total), total);
  }
  return last;
};

/** 最新版本信息（更新推送用；desktop = hub 版本，android 来自打包 manifest） */
export interface VersionInfo {
  desktop: string;
  /** 桌面端强制更新下限（低于它必须更新才能继续使用） */
  desktopMin: string | null;
  android: string | null;
  /** 移动端强制更新下限 */
  androidMin: string | null;
}

export const getVersionInfo = () => {
  return get<VersionInfo>("/monitor/version");
};

/** 设备配对认领：把客户端展示的配对码绑定到当前登录账号（绑定即信任） */
export const claimPairDevice = (code: string) => {
  return post<{ machineId: string; hostname: string; platform: string }>(
    "/monitor/pair/claim",
    { code }
  );
};

// ---------- 用户自助机器人集成 ----------

export interface IntegrationsInfo {
  /** 自己的钉钉机器人（一个账号一个，谁配的就服务谁） */
  dingtalk?: {
    appKey: string;
    hasSecret: boolean;
    /** 已经跟机器人说过话 = 它知道该把推送发给谁了 */
    linked: boolean;
  };
  /** 管理员配的公共机器人：没配自己机器人的账号，绑个钉钉号就能用 */
  globalBot?: {
    available: boolean;
    boundIds: DingtalkBoundId[];
  };
  /** 机器人文件接收目录（所有渠道通用）：按「设备 → 项目」层级列出 */
  recvDirDevices?: {
    machineId: string;
    hostname: string;
    projects: {
      cwd: string;
      name: string;
      dir?: string | null;
      taskId?: string | null;
    }[];
  }[];
}

/** 已绑到本账号的钉钉号 */
export interface DingtalkBoundId {
  staffId: string;
  nick: string;
}

export const getIntegrations = async () => {
  return await get<IntegrationsInfo>("/monitor/integrations");
};

/** 设置某项目的钉钉文件接收目录（dir 空 = 清除，回落默认 tmp） */
export const setDingtalkRecvDir = async (project: string, dir: string) => {
  return await post<{ result: string }>(
    "/monitor/integrations/dingtalk-recv-dir",
    { project, dir }
  );
};

/** 配置自己的钉钉机器人；appKey 传空 = 解绑。appSecret 留空表示沿用已存的 */
export const setDingtalkApp = async (data: { appKey: string; appSecret?: string }) => {
  return await post<{ result: string }>(
    "/monitor/integrations/dingtalk-app",
    data,
  );
};

/**
 * 取扫码绑定的授权地址：把 url 画成二维码，用钉钉扫一下即可把该钉钉号绑到本账号。
 * command 是同一个码的另一种用法（扫不了时手动发给机器人）。
 */
export const getDingtalkQr = async () => {
  return await get<{
    url: string;
    code: string;
    expiresIn: number;
    command: string;
  }>("/monitor/integrations/dingtalk-qr");
};

/** 本账号已绑定的钉钉号 */
export const getDingtalkIds = async () => {
  return await get<ListRes<DingtalkBoundId>>("/monitor/integrations/dingtalk-ids");
};

/** 解绑自己的某个钉钉号 */
export const unbindDingtalkId = async (staffId: string) => {
  return await post<{ result: string }>("/monitor/integrations/dingtalk-unbind", {
    staffId,
  });
};

/** 认领机器人回发的绑定链接（URL 上的 ?dtbind= token） */
export const claimDingtalkBind = async (token: string) => {
  return await post<{ result: string; staffId: string; nick: string }>(
    "/monitor/integrations/dingtalk-bind",
    { token },
  );
};

/**
 * 现取会话目录里的一个文件（显示 agent 输出里引用的截图）。
 *
 * 异步：hub 向那台机器现要一次，所以第一次多半回 `pending`，隔一会儿再调即可。
 * hub 只在内存中转、交件即删，不落盘。
 */
export const getTaskFile = async (id: string, rel: string) => {
  return await get<{ pending: boolean; mime?: string; contentB64?: string }>(
    `/monitor/tasks/${id}/file`,
    { params: { rel } }
  );
};

/** 仍在排队（未被客户端取走）的输入 */
export const getQueuedInputs = async (id: string) => {
  return await get<ListRes<{ cmdId: string; text: string }>>(
    `/monitor/tasks/${id}/queued`,
  );
};

/** 撤回还在排队的输入（已被终端接收则失败） */
export const recallPortalInput = async (id: string, cmdId: string) => {
  return await post<boolean>(`/monitor/tasks/${id}/recall`, { cmdId });
};

/** 向终端注入按键：撤回终端原生排队(up，按 count 次) / 插入排队到会话(esc)。
 *  仅 iTerm2(mac) 与 Windows 控制台可干净注入。 */
export const termKeyTask = async (
  id: string,
  key: "up" | "esc",
  count = 1,
) => {
  return await post<boolean>(`/monitor/tasks/${id}/termkey`, { key, count });
};

/** 会话目录下的子目录与文件（异步：pending=true 时轮询重试） */
export const getTaskDirs = async (id: string, rel: string) => {
  return await get<{ dirs: string[]; files: string[]; cwd: string; pending: boolean }>(
    `/monitor/tasks/${id}/dirs`,
    { params: { rel } },
  );
};

/** 会话目录内文件夹操作（新建/删除/重命名）：下发给 agent，返回 opId 后轮询结果 */
export const fsopTask = async (
  id: string,
  body: { op: "mkdir" | "delete" | "rename"; rel: string; name: string; newName?: string },
) => {
  return await post<{ opId: string; pending: boolean }>(
    `/monitor/tasks/${id}/fsop`,
    body,
  );
};

/** 取文件夹操作结果（agent 回传前 pending=true，需轮询） */
export const getFsopResult = async (id: string, opId: string) => {
  return await get<{ ok?: boolean; msg?: string; pending: boolean }>(
    `/monitor/tasks/${id}/fsop/${opId}`,
  );
};

/** 配置同步：单台设备的同步状态 */
export interface ConfigSyncDevice {
  machineId: string;
  hostname: string;
  platform: string;
  trusted: boolean;
  online: boolean;
  /** 是否为配置源（其余设备向它看齐） */
  isSource: boolean;
  /** 客户端是否已支持配置同步（没上报过清单则为 false，通常是版本旧或刚上线） */
  supported: boolean;
  /** 本机在管配置份数 */
  fileCount: number;
  /** 还差多少份：源机 = 服务端基线尚未收全，镜像机 = 本机尚缺 */
  behind: number;
  /** 最近一次扫描时刻（unix 秒，0 = 没扫过） */
  scannedAt: number;
}

/** 配置同步总览 */
export interface ConfigSyncInfo {
  enabled: boolean;
  /** 配置源设备 id；未开启时为 null */
  source: string | null;
  /** 服务端基线里的份数 */
  baselineCount: number;
  devices: ConfigSyncDevice[];
}

/** 配置同步状态：谁是配置源、各设备还差多少份 */
export const getConfigSync = async () => {
  return await get<ConfigSyncInfo>("/monitor/config/sync");
};

/** 指定配置源设备；machineId 传空 = 关闭配置同步 */
export const setConfigSource = async (machineId: string) => {
  return await post<{ result: string }>("/monitor/config/source", { machineId });
};
