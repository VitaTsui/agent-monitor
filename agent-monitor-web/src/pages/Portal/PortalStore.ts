import {
  getQueuedInputs,
  recallPortalInput,
  termKeyTask,
  HistorySession,
  PortalControlAction,
  PortalDevice,
  PortalMessage,
  PortalTaskData,
  SubTask,
  controlPortalTask,
  deletePortalDevice,
  getHistorySessionList,
  getPortalDevices,
  getPortalSubAgentMessages,
  getPortalSubTasks,
  getPortalTaskList,
  getPortalTaskMessages,
  sendPortalInput,
  setPortalTaskNote,
  trustPortalDevice,
  untrustPortalDevice,
} from "@/services/apis/portal";

import { isSlashCommand } from "./_utils/slashCommand";
import { isStateSnapshot } from "./_utils/sessionState";

import { makeAutoObservable } from "mobx";
import { message as antdMessage } from "@hsu-react/ui";
import { getAccessToken } from "@/utils/auth";

/** 拆分视图最多同时打开的会话数 */
const MAX_PANES = 4;

/**
 * 右栏（会话状态）**按格**记一份：`{ 会话 id: { open } }`。
 *
 * 从前是全局一个布尔（`am.portal.rightPane.open`）＋ 全局一个比例
 * （`am.portal.rightPane.ratio`）：拆分成 2~4 格时，任何一格的开关都在拨同一个值
 * —— 那不是「一格的右栏」，是「整页一条右栏」。这两个旧键已废弃，
 * 加载时顺手删掉，不留第二套状态在磁盘上。
 *
 * **只剩「开/关」这一个维度**：右栏宽度已改为固定 320（见 `RightPane`），
 * 比例不再是可调项。存量记录里带着的 `ratio` 字段读的时候直接忽略 —— 多出来的
 * 字段不影响 `open`，不必为它留第二套解析。
 */
const RIGHT_PANE_KEY = "am.portal.rightPane.byId";

/** 侧栏顶部选中的那台设备。刷新后要停在同一台上，否则每次进来都跳回本机 */
const SELECTED_MACHINE_KEY = "am.portal.sidebar.machine";

/** 上一版的两个全局键。只在加载时清一次，不再有任何代码读它们 */
const RIGHT_PANE_LEGACY_KEYS = [
  "am.portal.rightPane.open",
  "am.portal.rightPane.ratio",
];

/** 一格右栏的状态。**只有开关**，宽度是固定的（见 `RightPane`） */
interface RightPaneState {
  /** 开着没有 */
  open: boolean;
}

/** WS 断开后的重连退避（毫秒），逐次递增，封顶 10s */
const WS_RETRY_MS = [1000, 2000, 5000, 10000];

/**
 * WS 掉线时的轮询兜底间隔。
 * WS 正常时不轮询任务列表（推送即时且省流），断了才退回轮询，
 * 保证浏览器/代理不支持 WS 时功能不残废。
 */
const POLL_MS = 2000;

/**
 * 一个「客户端」= 一台机器上的一个终端程序。
 *
 * **`provider` 一个字段不够**：Codex CLI 与 ChatGPT 桌面版同属 `provider === "codex"`，
 * 但它们是两个各跑各的客户端，会话也分别存在两处。只按 provider 分的话两边的会话
 * 糊成一组，组名还只能二选一。所以键是 `(machineId, provider, desktop)` 这一组。
 */
export interface ClientKey {
  machineId: string;
  provider: string;
  /** 终端 CLI（false）还是桌面客户端（true） */
  desktop: boolean;
}

/** 侧栏里一个客户端分组下的一个时间桶 */
export interface SessionBucket {
  label: string;
  items: PortalTaskData[];
}

/**
 * CLI 分组里的一个**项目**（一个 cwd 一组）。
 *
 * 只有 CLI 有这一层：终端会话天然长在某个工作目录下，「我在哪个项目上开着哪几个
 * 终端」才是那一列要回答的问题。桌面客户端的对话没有 cwd，硬塞一层「未知项目」
 * 是凭空多一级缩进。
 */
export interface SessionProject {
  /** 归一化后的目录键（与后端 `encode_path` 同规则，见 `normProject`） */
  key: string;
  /** 展示名：备注过的项目名 > 目录名 > 「未知项目」 */
  title: string;
  items: PortalTaskData[];
}

/** 侧栏里的一个客户端分组（可折叠，内含该客户端的全部会话） */
export interface ClientSection extends ClientKey {
  /**
   * `machineId|provider|desktop`，折叠态与历史分页都按它记。
   *
   * **第三段是 `desktop` 这个布尔量，不是展示名**：展示名是中文串，既不能当契约级的键，
   * 也不能当筛选参数回传给 `/monitor/sessions/history`。
   */
  key: string;
  hostname: string;
  providerDsr: string;
  platformDsr: string;
  /** 此刻有几条在执行 */
  running: number;
  /** 服务端说的历史总条数（不受分页影响）；还没拉过是 0 */
  total: number;
  buckets: SessionBucket[];
  /**
   * **CLI 专有**：组内按项目再分一层。桌面分组恒为空数组，走 `buckets`。
   *
   * 两者互斥而不是并存：一个 CLI 分组永远只走 `projects`，一个桌面分组永远只走
   * `buckets`。渲染层据此二选一，不做「两个都铺」这种事。
   */
  projects: SessionProject[];
  /** 历史正在拉 */
  loading: boolean;
  /** 拉过至少一页了 */
  loaded: boolean;
  /** 还有更旧的可以翻 */
  hasMore: boolean;
}

/** 一个客户端的历史分页状态 */
interface ClientHistoryState {
  list: HistorySession[];
  total: number;
  /** 下一页的时间游标；null = 到底了 */
  nextCursor: number | null;
  loading: boolean;
  loaded: boolean;
  /** 这份列表是按哪个关键字拉的 —— 关键字一变就整份作废重拉 */
  keyword: string;
}

/** 每页历史条数。50 是后端默认值，够铺满一屏又不至于一次拉太多 */
const HISTORY_PAGE = 50;

/**
 * 正文「读取中」的重试节奏。
 *
 * 历史会话与子会话的正文都不在上报缓存里 —— hub 要点名让那台机器现读磁盘再送回来，
 * 一次往返两轮上报（客户端约 1.5s 一轮），hub 最多等 8 秒就先回一个 `pending: true`。
 * 那**不是空会话**，过一会儿再问一次就有了；不重试的话界面会一直停在「暂无内容」。
 */
const PENDING_RETRY_MS = 2500;
/** 最多再问几次。问到第 6 次（约 15 秒）还没有，多半是那台机器离线了 */
const PENDING_RETRY_MAX = 6;

/**
 * 「现读磁盘」这类请求没拿到东西的**原因**。三种要分开说，别合成一句兜底。
 *
 * - `offline` 设备离线 —— 历史会话的正文与子任务都在**那台机器的磁盘上**，hub 手里
 *   没有。机器一离线就取不到，但这是**可恢复**的：机器回来点一下重试就有了。
 * - `missing` 会话/子会话真的不存在（记录被删了、id 不对）。重试也不会变。
 * - `network` 请求压根没发出去 / 没回来。
 */
export type FetchFailKind = "offline" | "missing" | "network";

/**
 * 归一化目录键：与后端 `encode_path`（core/scanner）逐字对齐 —— 去尾随分隔符后，
 * 把每个非字母数字字符一律替换成 `-`，再小写。
 *
 * **必须与配对键同规则**：同一目录下的空会话（占位任务用进程 cwd）与真实会话
 * （用 jsonl 里的 cwd），以及 cursor / 非 cursor 终端，cwd 字符串常在分隔符、
 * 盘符冒号、标点处有细微差异；只做「斜杠 / 大小写」归一挡不住，两条本该同组的
 * 会话会分成两个项目。
 *
 * 这一份是从旧的 `selectedGroups` 里原样搬过来的，不另写一套。
 */
const normProject = (p: string | undefined | null) =>
  (p ?? "")
    .replace(/[/\\]+$/, "")
    .replace(/[^a-zA-Z0-9]/g, "-")
    .toLowerCase();

/** 项目的展示名：备注过的项目名 > 目录名 > 兜底。同样沿用旧 `selectedGroups` 的口径 */
const projectTitleOf = (t: PortalTaskData): string => {
  const dirName =
    (t.project ?? "").split(/[\\/]/).filter(Boolean).pop() ?? "";
  return t.projectName || dirName || "未知项目";
};

/** 时间桶的标签与顺序（照 VitaAgent 侧栏：今天 / 昨天 / 过去 7 天 / 过去 30 天 / 更早） */
const BUCKET_ORDER = ["今天", "昨天", "过去 7 天", "过去 30 天", "更早"];

/** 这条会话的最近活动时刻落在哪个桶里 */
const bucketOf = (ms: number, now = Date.now()): string => {
  const day = (t: number) => {
    const d = new Date(t);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  };
  if (!ms) {
    return "更早";
  }
  const days = Math.round((day(now) - day(ms)) / 86400000);
  if (days <= 0) return "今天";
  if (days === 1) return "昨天";
  if (days <= 7) return "过去 7 天";
  if (days <= 30) return "过去 30 天";
  return "更早";
};

/** 单会话在前端保留的最大消息数（合并是只增不减的，须有上限） */
const MAX_MESSAGES_PER_TASK = 500;

/**
 * 无缓存时统一返回这一个空数组。
 * 每次返回新的 [] 会让消费方 useEffect 的 [messages] 依赖永远比不相等，
 * 空会话下每帧都重跑副作用。
 */
const EMPTY_MESSAGES: PortalMessage[] = [];

/** 同 EMPTY_MESSAGES：无队列时统一返回这一个空数组，别每次新建 */
const EMPTY_HUB_QUEUED: { cmdId: string; text: string }[] = [];

/** 同 EMPTY_MESSAGES：没有子任务时统一返回这一个空数组 */
const EMPTY_SUB_TASKS: SubTask[] = [];

/**
 * 读出每一格右栏的开合。读不到（隐私模式 / 头一回来 / 存的是脏数据）
 * 就返回空表 —— 没记过的格一律按「开着」算。
 *
 * **只认 `open` 一个字段**：上一版每条还存着 `ratio`（可拖调的宽度比例），
 * 那个设计已经整条撤销。存量记录原样读得进来，多出来的 `ratio` 在这里被丢掉，
 * 下一次落盘就没了 —— 不为它留兼容分支。
 *
 * 顺手删掉上一版的两个全局键：它们已经没有任何读取方，留着只会让人以为还有人用。
 */
const readRightPaneState = (): Record<string, RightPaneState> => {
  try {
    RIGHT_PANE_LEGACY_KEYS.forEach((k) => localStorage.removeItem(k));
    const raw = localStorage.getItem(RIGHT_PANE_KEY);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as Record<string, Partial<RightPaneState>>;
    const out: Record<string, RightPaneState> = {};
    Object.entries(parsed ?? {}).forEach(([id, v]) => {
      if (!id || typeof v !== "object" || v === null) {
        return;
      }
      out[id] = { open: v.open !== false };
    });
    return out;
  } catch {
    return {};
  }
};

class PortalStore {
  private _tasks: PortalTaskData[] = [];
  private _devices: PortalDevice[] = [];
  /**
   * 侧栏顶部选中的设备（手动选过才有值）。
   *
   * 初值从 localStorage 读：刷新后要停在同一台机器上 —— 否则远程看另一台机器的
   * 人每刷一次页面就被扔回本机。存的是 machineId，那台机器暂时不在列表里时
   * 这个值仍然留着（见 `selectedMachineId` 的兜底），机器回来就自动选回去。
   */
  private _selectedMachineId = ((): string => {
    try {
      return localStorage.getItem(SELECTED_MACHINE_KEY) ?? "";
    } catch {
      // 隐私模式读不到就当没选过，退回默认（本机优先）
      return "";
    }
  })();
  /** 拆分视图中打开的会话（有序，全局跨设备） */
  private _openIds: string[] = [];
  /**
   * 「放大」模式下占据主区的那个会话。为空＝普通网格模式。
   *
   * 会话多了以后网格里每格都不够看，尤其在看某一个的长输出时。放大模式把它撑满主区，
   * 其余缩成右侧一列只读卡片 —— 仍能瞥见它们的动静，又不抢地方。
   */
  private _focusedId = "";
  /**
   * 每一格右栏（会话状态）的开合，按会话 id 各记一份。**默认开**。
   *
   * 这块内容原先钉在每一格对话流的末尾、一直看得见；搬进右栏后若默认收起，
   * 「这个会话正在办什么」就退回到「先点一下才看得见」—— 那正是把它从悬浮胶囊
   * 里挪出来时要解决的问题。默认开着、用户关掉才记一笔，才是不丢东西的换法。
   *
   * 表里没有的会话＝没动过，走默认值；所以这张表只装「用户真的改过的那几格」。
   */
  private _rightPaneById: Record<string, RightPaneState> = readRightPaneState();
  private _messagesById: Record<string, PortalMessage[]> = {};
  /**
   * hub 队列里待下发的输入（会话 id → 条目）。
   *
   * 这是**跨端可见**的那一份：本地回显只活在发起方自己的浏览器里，别的端无从知道。
   * 手机上发一条任务，桌面客户端得靠这份数据才看得见「有条任务正排着队」，
   * 而不是干等到终端执行完、真实消息回来才突然冒出来。
   */
  private _hubQueuedById: Record<string, { cmdId: string; text: string }[]> =
    {};
  private _loadingIds: string[] = [];
  /**
   * 正文**还没送回来**的会话（含子会话）。不是空会话、也不是出错。
   *
   * 历史会话与子会话的正文都要 hub 点名让那台机器现读磁盘，一次往返两轮上报。
   * 把这个状态单独记一份，界面才说得出「读取中」而不是「暂无可展示的对话内容」。
   */
  private _pendingIds: string[] = [];
  /** 每条会话已经为「读取中」重试过几次（到 PENDING_RETRY_MAX 就不再问） */
  private _pendingTries: Record<string, number> = {};
  /** 正文没取到的原因（会话 id / 子会话复合 id → 原因）。取到了就清掉 */
  private _bodyFailById: Record<string, FetchFailKind> = {};
  /** 子任务清单没取到的原因（会话 id → 原因）。取到了就清掉 */
  private _subTasksFailById: Record<string, FetchFailKind> = {};
  /**
   * 历史会话：按客户端（`machineId|provider`）各存一份分页状态。
   *
   * 不做成一张大列表再前端分组 —— 翻页游标是**按查询**的，每个客户端各翻各的页，
   * 混在一起就没法说清「这一页是哪一组的下一页」。
   */
  private _historyByClient: Record<string, ClientHistoryState> = {};
  /** 已知会话的静态资料（历史接口回来的那些），供 taskOf 在 `_tasks` 里找不到时兜底 */
  private _histById: Record<string, HistorySession> = {};
  /** 侧栏搜索框里的关键字。既筛活跃会话，也作为历史接口的 keyword 参数 */
  private _keyword = "";
  /**
   * 按需拉回来的**全量**子任务清单（会话 id → 清单）。
   *
   * `Task.subTasks` 只覆盖活跃会话的近 24 小时 / 50 条，历史会话压根没有这个字段 ——
   * 侧栏要把任意一条会话展开成子会话树，就得有这条按需通路（见 getPortalSubTasks）。
   */
  private _subTasksById: Record<string, SubTask[]> = {};
  /** 正在拉子任务清单的会话 */
  private _subTasksLoading: string[] = [];
  /**
   * 清单**还没送回来**的会话（`/subtasks` 一路 `pending: true` 问到头）。
   *
   * 这不是「没有子会话」。那个接口是 hub 点名让那台机器现读磁盘，机器慢半拍、
   * 上报节律没跟上都会一直 pending —— 从前问到第 6 次就把空清单写进缓存，
   * 界面于是说「该会话没有子会话」，一句假话，而且缓存住了再也不会自己纠正。
   * 单独记一份，侧栏才说得出「读取中」并给一条重试的出路。
   */
  private _subTasksPending: string[] = [];
  /**
   * 已经问过 `/subtasks` 的会话（无论成没成）。
   *
   * 与 `_subTasksById` 分开记，是为了让**失败**也算「问过」，却不覆盖已有数据：
   * 那个接口 404 的正常情形是「这条会话不在 hub 的清单里」，而活跃会话手上
   * 本来就有一份 `Task.subTasks`。失败时往 `_subTasksById` 写个空数组，等于用
   * 一次失败把已经拿到的子任务擦掉。
   */
  private _subTasksTried: string[] = [];
  /**
   * **子代理正文**（`父会话 id|agentId` → 消息列表）。
   *
   * 这一份只喂执行链里的智能体卡：点开一张子代理小卡，它自己走过的链就地接在
   * 卡片下面。从前是把子会话合成一条只读会话塞进 `_messagesById` 与 `openIds`，
   * 于是「子代理」在拆分视图、右栏、备注、号位每一处都要被当成会话特判一次 ——
   * 那套连同复合 id 一并撤了，这里只保留「按 agentId 拉一份正文」这一个能力。
   */
  private _subMsgsById: Record<string, PortalMessage[]> = {};
  /** 正在拉正文的子代理（同一个 key） */
  private _subMsgsLoading: string[] = [];
  /** 正文还没送回来的子代理：显示「读取中」并自动重试，**不是空** */
  private _subMsgsPending: string[] = [];
  /** 已经为「读取中」重试过几次 */
  private _subMsgsTries: Record<string, number> = {};
  /** 正文没取到的原因（离线 / 不存在 / 网络）。取到了就清掉 */
  private _subMsgsFail: Record<string, FetchFailKind> = {};
  /**
   * 撤回后把原文回填给对应会话的对话框：Composer 用 reaction 监听，命中自己的
   * taskId 就把 text 填进输入框再消费掉。带 nonce 是为了「撤回同一段文本」也能
   * 重新触发（否则相同对象引用不变、reaction 不响应）。
   */
  composerRefill: { taskId: string; text: string; nonce: number } | null = null;
  private _refillNonce = 0;

  /**
   * 「把执行链滚到这个子代理那张卡上」的一次请求。
   *
   * 侧栏的子会话行只负责**定位**：点它 = 打开父会话 ＋ 让正文滚到派出它的那一步。
   * 内容一律在执行链里看（见 `AgentCard`）—— 不再有「子会话当成一条独立会话打开」
   * 那条复合 id 路径，那套的毛病记在 `SessionTree` 的注释里。
   *
   * 做成一次性的请求（带 `seq`）而不是一个选中态：定位是个**动作**，
   * 做完就该消失；留成状态的话，用户手动滚开之后它还会把视图抢回去。
   * 同一个子代理连点两次也要能再滚一次，所以 `seq` 每次递增。
   */
  focusAgent: { taskId: string; agentId: string; seq: number } | null = null;
  private _focusSeq = 0;
  /**
   * 上一次**没定位到**的那个子代理。
   *
   * 定位不到是有原因的（派出它的那次工具调用不在已加载的正文里），但**原因要说在
   * 用户点的那个地方** —— 侧栏那一行上。弹一条飘过去的全局提示等于让人回头找
   * 刚才点的是哪条；而什么都不说就成了「点了没反应」，那是最让人反复戳的一种。
   */
  private _focusMissId = "";

  get focusMissId() {
    return this._focusMissId;
  }

  /** 请求把某条会话的执行链滚到某个子代理的卡片上并展开它 */
  public focusAgentCard = (taskId: string, agentId: string) => {
    this._focusSeq += 1;
    this.focusAgent = { taskId, agentId, seq: this._focusSeq };
    // 新的一次定位开始，上一次的「定位不到」就该收回去
    this._focusMissId = "";
  };

  /** 这次定位落地了（真的滚过去了），把请求消费掉 */
  public clearFocusAgent = (seq: number) => {
    if (this.focusAgent?.seq === seq) {
      this.focusAgent = null;
    }
  };

  /** 这次定位**找不到**目标：把原因记在那一行上，并把请求消费掉 */
  public reportFocusMiss = (seq: number) => {
    if (this.focusAgent?.seq !== seq) {
      return;
    }
    this._focusMissId = this.focusAgent.agentId;
    this.focusAgent = null;
  };

  constructor() {
    // 连接管理字段是命令式状态（WS 句柄、定时器句柄、世代号、关闭标志），
    // 没有任何视图观测它们，纳入 observable 既多余、又会在 WS 回调（裸闭包、
    // 非 action）里写入时留下隐患：一旦日后开启 enforceActions 就会抛错。
    // 这里显式排除，让它们保持普通字段。
    makeAutoObservable<
      PortalStore,
      | "_ws"
      | "_wsRetry"
      | "_wsRetryTimer"
      | "_pollTimer"
      | "_deviceTimer"
      | "_wsClosing"
      | "_wsGen"
    >(this, {
      _ws: false,
      _wsRetry: false,
      _wsRetryTimer: false,
      _pollTimer: false,
      _deviceTimer: false,
      _wsClosing: false,
      _wsGen: false,
    });
  }

  /**
   * 侧栏顶部那个设备选择器的列表，**数据源只有 `/monitor/devices` 一处**。
   *
   * 从前这份是拿 `_tasks` 去重现拼的 —— `_tasks` 只含**此刻有活进程**的会话，
   * 于是「我有哪几台机器」被偷换成「哪几台机器此刻有东西在跑」：一台合上盖的
   * 笔记本、一台只剩历史会话的机器，都会从选择器里整台消失，而它们的会话在
   * 下面的分组里明明还列着。分组（`clientSections`）用的就是 `_devices`，
   * 两处必须同源，否则「选不到但列得出」这种自相矛盾的状态一定会再出现。
   *
   * 顺序 = `_sortedDevices`：本机优先、其次主机名。选择器与分组同一个顺序。
   */
  /* **不带会话数**：两个消费方（设备选择器、命令面板）都不印了 ——
     `providers[].sessionCount` 是客户端回溯窗口内的条数，与侧栏实际列出多少
     不是一回事，不带限定词就会被读成「列表里有这么多」。字段连同那次求和
     一并去掉，不留着白算。 */
  get deviceList(): {
    machineId: string;
    hostname: string;
    platform: string;
    platformDsr: string;
    /** 此刻有几条在执行 */
    running: number;
    /** 此刻在不在线。离线设备**照样列出来**（历史会话仍然看得到） */
    online: boolean;
    /** 是不是客户端窗口所在的这台机器 */
    isLocal: boolean;
  }[] {
    return this._sortedDevices.map((d) => ({
      machineId: d.id,
      hostname: d.hostname || d.id,
      platform: d.platform,
      platformDsr: d.platformDsr,
      running: d.runningCount,
      online: d.online,
      isLocal: !!this._localMachineId && d.id === this._localMachineId,
    }));
  }

  /**
   * 设备的固定顺序：**本机优先、其次主机名**。
   *
   * 抽成一处的理由：选择器与分组都要照它排，各排各的就是两套排序并存，
   * 后端调了口径这边不跟，每轮还可能抖。
   */
  private get _sortedDevices(): PortalDevice[] {
    return [...this._devices].sort((a, b) => {
      const local =
        Number(b.id === this._localMachineId) -
        Number(a.id === this._localMachineId);
      if (local !== 0) {
        return local;
      }
      return (a.hostname || a.id).localeCompare(b.hostname || b.id, "zh");
    });
  }

  /** 客户端窗口里由页面注入的本机 machineId（浏览器为空） */
  private _localMachineId = "";

  public setLocalMachineId = (id: string) => {
    this._localMachineId = id;
  };

  /**
   * 侧栏此刻在看哪一台设备。
   *
   * 优先级：**手动选过的 > 本机 > 列表里第一台**。
   *
   * 最后那一档是这一版新加的，它不是兜底而是必需：侧栏的列表现在按设备**分**了，
   * 选不中任何一台就等于整列空着。上一版之所以能返回空串，是因为那时列表铺的是
   * 全部设备的全部客户端，选中与否只影响一处高亮。浏览器/远程端没有本机 id，
   * 头一回进来必须落在某一台上，才有东西可看。
   */
  get selectedMachineId() {
    const list = this._sortedDevices;
    if (list.length === 0) {
      return "";
    }
    if (
      this._selectedMachineId &&
      list.some((d) => d.id === this._selectedMachineId)
    ) {
      return this._selectedMachineId;
    }
    if (this._localMachineId && list.some((d) => d.id === this._localMachineId)) {
      return this._localMachineId;
    }
    return list[0].id;
  }

  public selectMachine = (id: string) => {
    if (id === this._selectedMachineId) {
      return;
    }
    // 切设备只改左侧列表看的是哪一台，不动已打开的会话：打开的会话是全局的
    //（可跨多设备），配合拆分可同时查看多设备的多个会话；也不默认选中任何会话。
    // 各会话属于哪台设备由内容区标题下方的设备名标识（见 ChatPane）。
    this._selectedMachineId = id;
    try {
      localStorage.setItem(SELECTED_MACHINE_KEY, id);
    } catch {
      // 隐私模式下写不进去也无妨，本次会话内仍然生效，下次回到默认
    }
  };

  /**
   * 侧栏的**客户端分组**：一台机器上的一种终端（Claude / Codex 各算一个）一组。
   *
   * **CLI 与桌面客户端铺的东西不一样**，判据是现成的 `desktop` 布尔量：
   *
   *   - **`desktop === false`（Claude Code、Codex CLI）→ 只列「当前打开的」会话。**
   *     CLI 的一条会话就是一个终端窗口：窗口一关会话就结束了，磁盘上那些 jsonl
   *     只是残留记录，不是用户心智里「还在的那段对话」。所以这一列不拉历史、
   *     不分页、也不按时间分桶（都是开着的窗口，「昨天」这种标签只是噪音）。
   *     「打开」的判据是 **`PortalTaskData.process != null`** —— 那个终端的进程
   *     还在。**不是 `status`**：`status` 说的是「在不在跑任务」，一个空闲但没关的
   *     终端照样是打开的；也不是时间阈值，那是字面量式的猜测。
   *     （同一判据在 `am-client` 里就是 `t.process.is_none()` = 父会话已结束。）
   *
   *   - **`desktop === true`（Claude 桌面版、ChatGPT 桌面版）→ 列全部历史**，
   *     按时间分桶（今天 / 昨天 / 过去 7 天 / 过去 30 天 / 更早）、游标翻页、
   *     搜索照旧。它的会话是持久的对话列表，回去翻、接着聊都成立。
   *
   * 桌面那一侧两份数据合成一列：
   *
   *   - **活跃会话**来自 `_tasks`（WS 秒级推送）—— 它带 `status`、`lastAction`、
   *     `slot`、`subTasks` 这些「此刻在干什么」的字段；
   *   - **历史会话**来自 `/monitor/sessions/history`（按需翻页）—— 它不过滤已结束的，
   *     但只有静态字段。
   *
   * 同一条会话两边都有时**以活跃那份为准**：反过来的话，一条正在跑的会话会因为
   * 历史快照里写着 `finished` 而显示成已结束。
   *
   * **组从哪来**：`/monitor/devices` 每台设备下发的 `providers`（这台机器上有哪几类
   * 终端）。从前是「客户端集合取自 `_tasks`」—— `_tasks` 只含**此刻有活进程**的会话，
   * 于是「这台机器上有哪些终端」被偷换成了「此刻有哪些终端在跑」：本机 33 条 Codex
   * 历史会话因为没有一条活着，Codex 组压根建不出来，那一组的历史请求也就永远不会
   * 发出（历史那一侧只能往**已存在**的组里填，建不出新组）。中间过渡过一版「数最近
   * 200 条历史倒推」，也一并删掉了 —— 某个终端最近一条会话排到 200 条之外就会凭空
   * 消失，那是将就不是答案。两套来源不并存。
   *
   * **只铺 `selectedMachineId` 那一台设备的客户端。** 设备这一维被提到了侧栏顶部的
   * 选择器上（见 `DeviceSelect`），组标题里因此不再带主机名 —— 组名就是客户端名
   * （`Claude Code` / `Codex` / `ChatGPT 桌面版`）。上一版是「设备 × 客户端」二合一，
   * 组标题形如 `MacBook Pro · Claude Code`：机器一多，同一个客户端名在一列里重复
   * 出现好几遍，而「我现在在看哪台机器」没有任何一处说得清。两套分组判断不并存 ——
   * 顶部选设备之后，这里就只按客户端分。
   *
   * **组的顺序就是这里的建组顺序**（Map 保序），末尾不再排一次：
   *   该设备的 `providers`（后端已按会话数降序给好）。
   * 前端再排一遍就是两套排序并存，每轮还可能抖。
   *
   * `_tasks` 与历史仍然参与，但只负责**补**：一种终端第一次跑起来时它的会话会先出现在
   * 热路径上，不必等设备列表下一轮（5 秒）才在侧栏冒出来。
   *
   * 设备离线时后端照常返回上次已知的那份 `providers`，所以离线设备的分组继续显示 ——
   * 笔记本一合盖侧栏就空掉是更糟的那一种；组内点开会走已有的「设备离线」提示。
   */
  get clientSections(): ClientSection[] {
    const kw = this._keyword.trim().toLowerCase();
    const hit = (...vals: (string | null | undefined)[]) =>
      !kw || vals.some((v) => (v ?? "").toLowerCase().includes(kw));

    const byClient = new Map<string, ClientSection>();
    const rowsByClient = new Map<string, Map<string, PortalTaskData>>();
    const ensure = (
      machineId: string,
      provider: string,
      desktop: boolean,
      meta: { hostname?: string; providerDsr?: string; platformDsr?: string },
    ): string => {
      const key = `${machineId}|${provider}|${desktop}`;
      if (!byClient.has(key)) {
        byClient.set(key, {
          key,
          machineId,
          provider,
          desktop,
          hostname: meta.hostname || machineId,
          providerDsr: meta.providerDsr || provider,
          platformDsr: meta.platformDsr || "",
          running: 0,
          total: 0,
          buckets: [],
          projects: [],
          loading: false,
          loaded: false,
          hasMore: false,
        });
        rowsByClient.set(key, new Map());
      }
      return key;
    };
    /** 会话（活跃的 / 历史的）那一侧的入口：字段名一样，缺省口径也一样 */
    const ensureOf = (t: PortalTaskData | HistorySession): string =>
      ensure(
        t.machineId || t.hostname || "unknown",
        t.provider || "unknown",
        // `desktop` 是契约的一部分，热路径与历史两边都下发。**不给兜底** ——
        // 缺了就是后端的 bug，该暴露出来，不该在这儿遮成「按 CLI 算」
        !!t.desktop,
        t,
      );

    /* 先把**选中那台设备**的客户端全集建出来：**有没有会话可铺是另一回事**，
       组本身必须先在。组内顺序照后端给的 `providers` 原样来 ——
       末尾因此不需要再 sort 一次。 */
    const mid = this.selectedMachineId;
    const device = this._sortedDevices.find((d) => d.id === mid);
    if (device) {
      for (const p of device.providers) {
        ensure(device.id, p.provider, p.desktop, {
          hostname: device.hostname,
          providerDsr: p.providerDsr,
          platformDsr: device.platformDsr,
        });
      }
    }

    /** 这条会话属于选中的那台设备吗。不属于就不进这一列 */
    const mine = (t: PortalTaskData | HistorySession) =>
      (t.machineId || t.hostname || "unknown") === mid;

    /**
     * 这条 CLI 会话的终端**还开着吗**。
     *
     * `process` 是后端每轮上报捎带的进程快照，进程没了就是 `null` ——
     * 与 `am-client` 里 `t.process.is_none()`（父会话已结束）同一个信号。
     * 不拿 `status` 凑：`status === "idle"` 的终端可能开着也可能早就关了，
     * 那说的是「在不在跑任务」，不是「窗口在不在」。
     */
    const cliOpen = (t: PortalTaskData) => t.process != null;

    // 再铺活跃会话：它们优先级最高，后面历史里的同 id 不覆盖它
    for (const t of this._tasks) {
      if (!mine(t)) {
        continue;
      }
      const key = ensureOf(t);
      if (t.status === "running") {
        byClient.get(key)!.running += 1;
      }
      /* CLI 那一列只铺**当前打开的**：`_tasks` 里还会留着刚结束、进程已经没了的
         那些，它们对 CLI 来说就是残留记录，不该混在「我现在开着哪几个终端」里。
         桌面那一侧照旧全收（它的会话本来就是持久的）。 */
      if (!byClient.get(key)!.desktop && !cliOpen(t)) {
        continue;
      }
      if (!t.id || !hit(t.note, t.title, t.prompt, t.projectName, t.hostname)) {
        continue;
      }
      rowsByClient.get(key)!.set(t.id, t);
    }

    // 再铺历史。关键字已经由服务端过滤过（keyword 参数），这里不再筛一遍 ——
    // 两处各筛一遍就会出现「服务端说 30 条、列表只显示 12 条」。
    for (const state of Object.values(this._historyByClient)) {
      for (const h of state.list) {
        if (!mine(h)) {
          continue;
        }
        const key = ensureOf(h);
        const sec = byClient.get(key)!;
        /* CLI 组不吃历史。正常情况下它压根没有历史可吃（`loadClientHistory`
           在源头就挡了），这一条是防**换过语义之前留下的旧分页状态**
           在内存里把已经关掉的终端重新铺回来。 */
        if (!sec.desktop) {
          continue;
        }
        sec.total = Math.max(sec.total, state.total);
        sec.loading = state.loading;
        sec.loaded = state.loaded;
        sec.hasMore = state.nextCursor !== null;
        if (h.id && !rowsByClient.get(key)!.has(h.id)) {
          rowsByClient.get(key)!.set(h.id, h);
        }
      }
    }
    /* 分页状态也要落到「历史一条都没返回」的那些组上（空结果同样是结果）。
       **按 key 找组，找不到就跳过** —— 别的设备的分页状态本来就不在这一列里 */
    Object.entries(this._historyByClient).forEach(([key, state]) => {
      const sec = byClient.get(key);
      if (sec?.desktop) {
        sec.total = Math.max(sec.total, state.total);
        sec.loading = state.loading;
        sec.loaded = state.loaded;
        sec.hasMore = state.nextCursor !== null;
      }
    });

    const sections = [...byClient.values()];
    for (const sec of sections) {
      const rows = [...rowsByClient.get(sec.key)!.values()];

      if (!sec.desktop) {
        /**
         * **CLI 这一列按「会话开始时刻」倒序，不按活动时刻。**
         *
         * 从前这里是 `rows.sort((a,b) => b.mtimeMs - a.mtimeMs)` —— `mtimeMs` 是
         * 会话 jsonl 的最后写入时刻，**每轮上报（约 1.5~3 秒）都在变**。于是哪条
         * 会话冒出一条新消息就窜到本组最上面，整列跟着上下跳（用户原话：
         * 「侧边的会话位置不要跳上跳下的」）。而且下面那个项目分组是**按 `rows`
         * 的顺序首次出现建组**的，所以项目之间也跟着一起跳。
         *
         * `startedAt` 是一条会话的起点，**它一生不变**。于是：
         *   - 已经在列表里的会话，**无论收到多少新消息，位置纹丝不动**；
         *   - 只有「新开一个终端」这种用户自己的动作才会改变顺序，新的排在最上面
         *     （那正是你刚开的那个，本来就该一眼看到）。
         * 拿不到 `startedAt` 的排到最后，再用 id 兜底，保证**完全确定**、
         * 不依赖 sort 的稳定性实现。
         *
         * **没有用防抖/节流/动画去压住重排** —— 那是把抖动藏起来，滚动位置和
         * 点击目标照样会错位。这里换的是排序键本身。
         */
        const startedMs = (t: PortalTaskData) =>
          t.startedAt ? Date.parse(t.startedAt) || 0 : 0;
        rows.sort(
          (a, b) =>
            startedMs(b) - startedMs(a) || (a.id ?? "").localeCompare(b.id ?? ""),
        );

        /**
         * CLI：**按项目再分一层，不分桶、不翻页**。
         *
         * 终端会话天然长在某个工作目录下，「我在哪个项目上开着哪几个终端」才是
         * 这一列要回答的问题（用户原话：「cli agent 按项目分没了」）。层级因此是
         * 客户端 → 项目 → 会话 → 子代理，与参照的「项目行 → 会话行」同构。
         *
         * **不分时间桶**：分桶回答的是「这条会话是什么时候的」，对一列**全都开着**
         * 的终端窗口没有意义 —— 一个开了三天没关的终端会被标成「过去 7 天」，
         * 而它就在眼前。`buckets` 在 CLI 这一侧恒为空，渲染层走 `projects`。
         *
         * 项目内保持 `rows` 已有的顺序；项目之间按「组里最新开的那条会话」排 ——
         * 建组是按 `rows` 顺序首次出现，所以这一条是白来的。**只有新开终端才会
         * 改变项目顺序**，收消息不会。
         *
         * 计数换成**实际列出的条数**：历史总数（那台机器上攒下的 73 条 jsonl）
         * 与这一列没有关系，写上去就是标题说 73、列表只有 2 条。
         */
        const byProject = new Map<string, SessionProject>();
        for (const r of rows) {
          const title = projectTitleOf(r);
          const key = normProject(r.project) || title.toLowerCase();
          const grp = byProject.get(key);
          if (grp) {
            grp.items.push(r);
          } else {
            byProject.set(key, { key, title, items: [r] });
          }
        }
        sec.projects = [...byProject.values()];
        sec.buckets = [];
        sec.total = rows.length;
        sec.loading = false;
        sec.loaded = true;
        sec.hasMore = false;
        continue;
      }
      sec.projects = [];

      /* 桌面那一列**仍按最近活动倒序**，这不是疏忽：`HistorySession.mtimeMs` 同时是
         `/monitor/sessions/history` 的**翻页游标**（见 portal.ts 的字段说明），本地
         改用别的键排，「加载更早」翻回来的那一页就会插到莫名其妙的位置。
         而且这一列里绝大多数是已经结束的历史会话，`mtimeMs` 早就不动了 ——
         它本来就不跳。两列的契约不同，排序键跟着不同，不是两套并存。 */
      rows.sort((a, b) => (b.mtimeMs ?? 0) - (a.mtimeMs ?? 0));

      const buckets = new Map<string, PortalTaskData[]>();
      for (const r of rows) {
        const label = bucketOf(r.mtimeMs ?? 0);
        const arr = buckets.get(label);
        if (arr) {
          arr.push(r);
        } else {
          buckets.set(label, [r]);
        }
      }
      sec.buckets = BUCKET_ORDER.filter((l) => buckets.has(l)).map((label) => ({
        label,
        items: buckets.get(label)!,
      }));
    }
    /* **这里不再排序**：顺序就是上面的建组顺序（Map 保序）——
       选中设备的 `providers`（后端已按会话数降序给好）。
       再排一遍就是两套排序并存，后端调了口径这边不跟，每轮还可能抖。 */
    return sections;
  }

  get devices() {
    return this._devices;
  }

  get pendingCount() {
    return this._devices.filter((d) => !d.trusted).length;
  }

  /** 全量会话列表（分享接收等场景选目标用） */
  get tasks() {
    return this._tasks;
  }

  get openIds() {
    return this._openIds;
  }

  /**
   * 当前放大的会话 id；为空表示普通网格。
   *
   * 取值时兜一道「它还开着吗」：会话被关掉或从列表消失时，若不校验就会卡在一个空的
   * 放大态里 —— 主区什么都没有、右侧却列着其余几个，看着像坏了。
   */
  get focusedId() {
    return this._focusedId && this._openIds.includes(this._focusedId)
      ? this._focusedId
      : "";
  }

  /** 放大某个会话；传空串或再点一次当前放大的那个＝还原成网格 */
  public setFocused = (id: string) => {
    this._focusedId = this._focusedId === id ? "" : id;
  };

  /** 这一格的右栏开着吗。没记过＝开着 */
  public isRightPaneOpen = (taskId: string): boolean =>
    this._rightPaneById[taskId]?.open ?? true;

  /** 开/收某一格的右栏。**每格一份**：A 格开着、B 格关着是合法状态 */
  public toggleRightPane = (taskId: string) => {
    if (!taskId) {
      return;
    }
    this._rightPaneById = {
      ...this._rightPaneById,
      [taskId]: { open: !this.isRightPaneOpen(taskId) },
    };
    this.saveRightPaneState();
  };

  /**
   * 落盘，顺带清死键。
   *
   * 键是会话 id，会话是会被删掉的：不清的话这张表只增不减，攒上几个月就是一堆
   * 指向不存在会话的记录。判据取「服务端还认这个会话吗」（`_tasks`）—— 已关掉但
   * 还在列表里的格要留着（下次再打开仍是上次的开合），彻底消失的才丢。
   */
  private saveRightPaneState = () => {
    const live = new Set<string>([
      ...this._tasks.map((t) => t.id ?? ""),
      ...this._openIds,
    ]);
    const next: Record<string, RightPaneState> = {};
    Object.entries(this._rightPaneById).forEach(([id, v]) => {
      if (live.has(id)) {
        next[id] = v;
      }
    });
    this._rightPaneById = next;
    try {
      localStorage.setItem(RIGHT_PANE_KEY, JSON.stringify(next));
    } catch {
      // 隐私模式下写不进去也无妨，下次回到默认（开着）
    }
  };

  /**
   * 按 id 取一条会话。两种来源都认：
   *
   *   1. 活跃会话 —— `_tasks`（最新，带 status / lastAction / subTasks）
   *   2. 历史会话 —— `_histById`（侧栏翻页时攒下的静态资料）
   *
   * 从前还有第三种：把子会话合成一条只读会话，好让它复用整套会话 UI（复合 id
   * `父::agentId`）。那套连同侧栏的子会话树一并推翻了 —— 子代理是**执行链上的
   * 一步**，不是一条会话：它没有自己的进程、队列、备注、号位，把它塞进会话通道
   * 之后每一处都要现场把这些字段清空，而用户真正要问的「谁派的、派在哪一步」
   * 反倒在会话列表里丢掉了。现在它就地画在链上（见 `_components/AgentCard`）。
   */
  public taskOf = (id: string): PortalTaskData | undefined => {
    if (!id) {
      return undefined;
    }
    return this._tasks.find((t) => t.id === id) ?? this._histById[id];
  };

  get openTasks() {
    return this._openIds
      .map((id) => this.taskOf(id))
      .filter(Boolean) as PortalTaskData[];
  }

  messagesOf = (id: string): PortalMessage[] =>
    this._messagesById[id] ?? EMPTY_MESSAGES;

  isLoadingMessages = (id: string): boolean => this._loadingIds.includes(id);

  /**
   * 正文还在路上（hub 正让那台机器现读磁盘）。
   * 界面据此显示「读取中」——**不要**把它当成空会话，那是两回事。
   */
  isMessagesPending = (id: string): boolean => this._pendingIds.includes(id);

  /** 这条会话（或子会话）的正文为什么没取到。取到了 / 还没问过就是 undefined */
  public messagesFailOf = (id: string): FetchFailKind | undefined =>
    this._bodyFailById[id];

  /** 这条会话的子任务清单为什么没取到 */
  public subTasksFailOf = (id: string): FetchFailKind | undefined =>
    this._subTasksFailById[id];

  /**
   * 这次失败该归到哪一类。
   *
   * **判据全是结构化的**：后端信封里的 `code`（见 hub `admin.rs` 的 `err()`：
   * HTTP 恒 200，真正的状态码在 body 的 `code` 上）＋ 设备列表里的 `online`。
   * 一个中文文案都不匹配 —— 那是上游随时会改的措辞，拿它当接口用改一版就失灵。
   *
   * 为什么 404 还要再判一次设备在不在线：机器掉线超过 10 秒，hub 会把**它名下的
   * 任务一起丢掉**，于是「设备离线」从第 8 秒起就以 `404 任务不存在` 的形式出现
   * （实测：离线 4s 是 500，8s 起变 404）。只看 code 的话，用户会被告知
   * 「这条会话已经被删了」—— 而它其实好好躺在那台关着的机器上。
   */
  private failKindOf = (id: string, code?: number): FetchFailKind => {
    if (code === 500) {
      return "offline";
    }
    const machineId =
      this._tasks.find((t) => t.id === id)?.machineId ??
      this._histById[id]?.machineId ??
      "";
    if (
      machineId &&
      this._devices.some((d) => d.id === machineId && !d.online)
    ) {
      return "offline";
    }
    return "missing";
  };

  /** 记一笔 / 清掉某条会话正文的失败原因 */
  private setBodyFail = (id: string, kind?: FetchFailKind) => {
    if (this._bodyFailById[id] === kind) {
      return;
    }
    const next = { ...this._bodyFailById };
    if (kind) {
      next[id] = kind;
    } else {
      delete next[id];
    }
    this._bodyFailById = next;
  };

  /** 同 setBodyFail，作用在子任务清单上 */
  private setSubTasksFail = (id: string, kind?: FetchFailKind) => {
    if (this._subTasksFailById[id] === kind) {
      return;
    }
    const next = { ...this._subTasksFailById };
    if (kind) {
      next[id] = kind;
    } else {
      delete next[id];
    }
    this._subTasksFailById = next;
  };

  private setSubTasksPending = (id: string, on: boolean) => {
    const has = this._subTasksPending.includes(id);
    if (has === on) {
      return;
    }
    this._subTasksPending = on
      ? [...this._subTasksPending, id]
      : this._subTasksPending.filter((x) => x !== id);
  };

  // ---------- 子任务（子会话树）----------

  /**
   * 一条会话的子任务清单：全量快照为底，**同 id 以更新的那份为准**。
   *
   *   - 按需拉回来的那份（`getPortalSubTasks`）**全**，但它是一次读盘的快照；
   *   - 上报捎带的 `Task.subTasks`（只有会话还在 `_tasks` 里时才有，且只覆盖近 24 小时
   *     / 50 条）**新**，WS 每轮推送。
   *
   * **快照的活性由 `hasRunningSubAgent` ＋ ChatPane 的定时重拉负责**，不是靠 WS 兜底。
   * 曾经是靠 WS 兜的，那是个错的假设：会话一旦不在 `_tasks` 里（历史会话、进程已退出
   * 没配上），WS 那一路压根不存在，`outcome` 就永远冻结在打开那一刻 —— 子代理明明
   * 跑完了，卡片还写着「执行中 · 13分42秒」，链上那条 5 秒轮询的停止条件也因此
   * 永远不成立。那个假设整条推翻，见 `loadSubTasks` 与 `hasRunningSubAgent`。
   *
   * WS 那一份**仍然保留**：会话还活着时它是秒级的，比定时重拉快一个量级，白拿的新鲜度
   * 没有理由丢掉。两者不是两套刷新 —— 刷新只有一处（定时重拉），这里只是「同一条子任务
   * 谁的版本更新就用谁的」这一条读取规则。
   */
  public subTasksOf = (id: string): SubTask[] => {
    const full = this._subTasksById[id];
    const live = this._tasks.find((t) => t.id === id)?.subTasks;
    if (!full) {
      return live ?? EMPTY_SUB_TASKS;
    }
    if (!live?.length) {
      return full;
    }
    const byId = new Map(live.map((t) => [t.id, t]));
    const merged = full.map((t) => byId.get(t.id) ?? t);
    const seen = new Set(full.map((t) => t.id));
    live.forEach((t) => {
      if (!seen.has(t.id)) {
        merged.push(t);
      }
    });
    return merged;
  };

  /**
   * 这条会话名下**还有子代理在跑**吗 —— 定时重拉子任务清单的唯一开关。
   *
   * 「还在跑」这件事只有清单自己说得出来，所以它既是刷新的条件、也是刷新的结果：
   * 最后一个子代理翻成终态的那一轮，这里跟着翻假，定时器当场停 —— 不靠超时上限、
   * 不靠次数封顶那类「让它自己累死」的补丁。
   */
  public hasRunningSubAgent = (id: string): boolean =>
    this.subTasksOf(id).some((t) => t.kind === "agent" && t.outcome === "running");

  public isSubTasksLoading = (id: string): boolean =>
    this._subTasksLoading.includes(id);

  /** 清单问到头仍是 `pending` —— 显示「读取中」并给重试，**不当成空** */
  public isSubTasksPending = (id: string): boolean =>
    this._subTasksPending.includes(id);

  /**
   * 这条会话的全量子任务清单拉过了没有。
   *
   * 侧栏靠它区分「确定没有子会话」（拉过、空的 → 那一行不给展开箭头）与
   * 「还不知道」（没拉过 → 给箭头，点了才去拉）。活跃会话的 `Task.subTasks`
   * 不算「拉过」：那份只覆盖近 24 小时 / 50 条，空不代表真的没有。
   */
  public isSubTasksLoaded = (id: string): boolean =>
    this._subTasksTried.includes(id);

  /**
   * 按需拉一条会话的全量子任务清单（侧栏展开那一下调）。
   *
   * **一条会话只拉一次**（除非 `force`）。这个接口是现读磁盘的，实测一条 149 条子任务的
   * 会话要 0.8~2 秒 —— 收起再展开重拉一遍，每次都要再等两秒，而清单本身几乎不动。
   *
   * 所以它**不进那条 5 秒轮询**：跟着正文一起刷等于让客户端每 5 秒重开一遍 jsonl。
   * 要活性的只有一种情形 —— 这条会话名下还有子代理在跑（`hasRunningSubAgent`），
   * 那时由 ChatPane 以明显更慢的节律带 `force` 重拉，跑完即停。
   *
   * `pending` 同样要重试：这份也是现读磁盘的。404（会话不存在 / 机器离线）就落一份
   * 空清单，不再重试 —— 那条会话确实没有子会话可展，重试也只是白问。
   */
  public loadSubTasks = (id: string, force = false, tries = 0) => {
    if (!id) {
      return;
    }
    /* 「已经在读」这一条只挡**新发起**的读（`tries === 0`）。
       `tries > 0` 是 pending 重试 —— 它就是同一次读的下一轮，而这条会话在
       整个重试窗口里都留在 `_subTasksLoading` 里（界面得一直说「正在读取」，
       不能每 2.5 秒闪一句「没有子代理」）。两件事别用同一个判据：
       挡住重试的后果是**只问一次就永远停在读取中**。 */
    if (tries === 0 && this._subTasksLoading.includes(id)) {
      return;
    }
    // 上一次**失败**过的允许再问一次（设备离线是可恢复的）；成功拿到过的才真正缓存住
    if (
      !force &&
      tries === 0 &&
      this._subTasksTried.includes(id) &&
      !this._subTasksFailById[id]
    ) {
      return;
    }
    if (!this._subTasksLoading.includes(id)) {
      this._subTasksLoading = [...this._subTasksLoading, id];
    }
    if (force || tries === 0) {
      this.setSubTasksFail(id, undefined);
      this.setSubTasksPending(id, false);
    }
    /** 这一轮问完了（不再重试）才算「不在拉」 */
    const settle = () => {
      this._subTasksLoading = this._subTasksLoading.filter((x) => x !== id);
    };
    getPortalSubTasks(id)
      .then((res) => {
        if (res.code !== 0) {
          settle();
          // 设备离线（code 500）与会话不存在（code 404）是两回事：前者点一下重试
          // 就好，后者重试多少次都一样。判的是结构化的 code，不是 msg 里那句话。
          this.setSubTasksFail(id, this.failKindOf(id, res.code));
          if (!this._subTasksTried.includes(id)) {
            this._subTasksTried = [...this._subTasksTried, id];
          }
          return;
        }
        const list = res.data?.list ?? [];
        // pending 且一条都没有 = 那台机器还没把清单送回来，过一会儿再问。
        // **重试窗口里不松开 loading**：松开的话这 2.5 秒界面会先说一句
        // 「该会话没有子会话」，下一轮又变回来，一眼看过去就是在骗人。
        if (res.data?.pending && !list.length) {
          if (tries < PENDING_RETRY_MAX) {
            setTimeout(
              () => this.loadSubTasks(id, force, tries + 1),
              PENDING_RETRY_MS,
            );
            return;
          }
          /* 问到头了还是 pending。**不写空清单**：那会把「读不到」永久缓存成
             「没有」，而且 `_subTasksTried` 一旦落定就再也不会自己重问。
             记成 pending，界面显示「读取中」并给一颗重试。 */
          settle();
          this.setSubTasksPending(id, true);
          return;
        }
        settle();
        this._subTasksById = { ...this._subTasksById, [id]: list };
        this.setSubTasksFail(id, undefined);
        this.setSubTasksPending(id, false);
        if (!this._subTasksTried.includes(id)) {
          this._subTasksTried = [...this._subTasksTried, id];
        }
      })
      .catch(() => {
        settle();
        // **不写空清单**：活跃会话手上还有一份随上报捎带的 `Task.subTasks`，
        // 写空等于用一次失败把已经拿到的子任务擦掉
        this.setSubTasksFail(id, "network");
        if (!this._subTasksTried.includes(id)) {
          this._subTasksTried = [...this._subTasksTried, id];
        }
      });
  };

  // ---------- 侧栏搜索 + 历史会话分页 ----------

  get keyword() {
    return this._keyword;
  }

  /**
   * 改侧栏搜索关键字。
   *
   * 关键字变了，各客户端已经翻过的历史页就整份作废 —— 那些页是按旧关键字、旧游标
   * 取回来的，留着会和新结果混在一起（表现为「搜出来的列表里混着不匹配的旧条目」）。
   * 已经展开过的组当场重拉第一页。
   */
  public setKeyword = (kw: string) => {
    if (kw === this._keyword) {
      return;
    }
    this._keyword = kw;
    // 只作废，不在这儿重拉 —— 重拉由侧栏那个「展开的组就把第一页拉上」的副作用统一负责，
    // 两处都发请求就会出现同一组被打两遍。
    this._historyByClient = {};
  };

  /**
   * 拉某个客户端的历史会话。
   *
   * @param key `machineId|provider`
   * @param more 翻下一页（用上一次返回的 `nextCursor` 作时间游标）。
   *   **不用页码**：这份列表的底料是每轮上报刷新的内存快照，翻页期间新会话会插进头部，
   *   用 offset 会让某条被跳过或看两遍。
   */
  public loadClientHistory = (key: string, more = false) => {
    /* key 是 `machineId|provider|desktop`，**三件套一起传**。
       只传 provider 的话，Codex CLI 与 ChatGPT 桌面版会拉到同一份混着的列表，
       那就等于分组白拆了（接口注释里写明了这一条）。 */
    const [machineId, provider, desktopFlag] = key.split("|");
    const desktop = desktopFlag === "true";
    if (!machineId) {
      return;
    }
    /**
     * **CLI 没有「历史会话」这回事，所以这个请求压根不发。**
     *
     * CLI 的一条「会话」就是一个终端窗口：窗口一关，会话就结束了，磁盘上留下的
     * jsonl 只是残留记录，不是用户心智里「还在的那段对话」——他不会回去翻，
     * 翻了也接不上（那个终端已经没了）。桌面客户端才相反：它的会话是持久的
     * 对话列表，回去翻、接着聊都成立。两者的列表语义本来就不同，从前一视同仁
     * 当成「可浏览的历史」是错的。
     *
     * 守卫放在这一层而不是调用方：这个接口是**现读磁盘**的，漏掉任何一个调用点
     * 就白白多一次昂贵请求。CLI 那一列铺什么，全由 `clientSections` 从 `_tasks`
     * 里挑（见那边的 `desktop === false` 分支）。
     */
    if (!desktop) {
      return;
    }
    const prev = this._historyByClient[key];
    if (prev?.loading) {
      return;
    }
    // 已经拉过、且关键字没变、又不是要翻页 —— 没有必要再问一次
    if (!more && prev?.loaded && prev.keyword === this._keyword) {
      return;
    }
    const before = more ? (prev?.nextCursor ?? undefined) : undefined;
    if (more && before == null) {
      return;
    }
    this._historyByClient = {
      ...this._historyByClient,
      [key]: {
        list: more ? (prev?.list ?? []) : [],
        total: prev?.total ?? 0,
        nextCursor: prev?.nextCursor ?? null,
        loading: true,
        loaded: prev?.loaded ?? false,
        keyword: this._keyword,
      },
    };
    getHistorySessionList({
      machineId,
      provider: provider || undefined,
      desktop,
      keyword: this._keyword.trim() || undefined,
      before,
      limit: HISTORY_PAGE,
    })
      .then((res) => {
        const cur = this._historyByClient[key];
        // 关键字在请求飞行途中变过 —— 这份结果已经不是用户现在要看的，丢掉
        if (!cur || cur.keyword !== this._keyword) {
          return;
        }
        const list = res.code === 0 ? (res.data?.list ?? []) : [];
        const merged = more ? [...(prev?.list ?? []), ...list] : list;
        this._historyByClient = {
          ...this._historyByClient,
          [key]: {
            list: merged,
            total: res.data?.total ?? merged.length,
            nextCursor: res.data?.nextCursor ?? null,
            loading: false,
            loaded: true,
            keyword: this._keyword,
          },
        };
        // 攒一份静态资料：点开一条历史会话时 taskOf 要靠它才认得出这条会话
        const next = { ...this._histById };
        merged.forEach((h) => {
          if (h.id) {
            next[h.id] = h;
          }
        });
        this._histById = next;
      })
      .catch(() => {
        const cur = this._historyByClient[key];
        if (cur) {
          this._historyByClient = {
            ...this._historyByClient,
            [key]: { ...cur, loading: false, loaded: true },
          };
        }
      });
  };

  public init = () => {
    this.refresh();
    this.loadDevices();
    this.stopPolling();
    this.connectWs();
    // 设备列表不走 WS（推送只含会话），单独低频轮询
    this._deviceTimer = setInterval(this.loadDevices, 5000);
  };

  // ---------- 实时推送 ----------

  private _ws: WebSocket | null = null;
  private _wsRetry = 0;
  private _wsRetryTimer: ReturnType<typeof setTimeout> | null = null;
  /** WS 不可用时的兜底轮询 */
  private _pollTimer: ReturnType<typeof setInterval> | null = null;
  private _deviceTimer: ReturnType<typeof setInterval> | null = null;
  /** 主动关闭时置位，避免 onclose 触发重连 */
  private _wsClosing = false;
  /**
   * 连接世代号。PortalStore 是单例，close() 又是异步的：
   * 拆卸后组件若重新挂载（路由往返 / StrictMode），新连接会把 _wsClosing 重置，
   * 此时旧连接迟到的 onclose 会误判「不是主动关闭」而触发重连 → 双连接。
   * 每条连接记住自己建立时的世代，回调里比对，过期的直接退出。
   */
  private _wsGen = 0;

  private connectWs = () => {
    const token = getAccessToken();
    if (!token) {
      // 没登录态就别连了，直接退回轮询
      this.startPolling();
      return;
    }
    this._wsClosing = false;
    // 本条连接的世代号：所有回调里比对，认出自己是不是已经被拆卸/替换掉的旧连接
    const gen = ++this._wsGen;
    // 与 API 同源同前缀：开发走 /api 代理（已开 ws:true），生产 API_BASE 为空即同源
    const base = import.meta.env.API_BASE;
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${window.location.host}${base}/monitor/ws?token=${encodeURIComponent(
      token,
    )}`;

    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch {
      this.startPolling();
      return;
    }
    this._ws = ws;

    ws.onopen = () => {
      if (gen !== this._wsGen) {
        return;
      }
      this._wsRetry = 0;
      // 推送已接管会话列表，停掉兜底轮询
      this.stopPolling_();
    };

    ws.onmessage = (e) => {
      if (gen !== this._wsGen) {
        return;
      }
      try {
        const payload = JSON.parse(e.data);
        if (payload?.type === "tasks" && Array.isArray(payload.data)) {
          this.applyTasks(payload.data as PortalTaskData[]);
          return;
        }
        if (payload?.type === "error") {
          // 登录态失效等：服务端会随后断开，这里不重连，退回轮询让
          // 正常的 401 处理去接管跳登录
          this._wsClosing = true;
          this.startPolling();
        }
      } catch {
        // 坏帧忽略，不影响后续推送
      }
    };

    ws.onerror = () => {
      // onerror 后必定跟 onclose，重连逻辑统一放那儿
      ws.close();
    };

    ws.onclose = () => {
      // 过期连接（已被拆卸或新连接替换）的迟到 onclose：什么都不做，
      // 尤其不能重连——否则会和当前连接叠成双连接。
      if (gen !== this._wsGen) {
        return;
      }
      this._ws = null;
      if (this._wsClosing) {
        return;
      }
      // 断线期间先用轮询顶着，别让页面停更
      this.startPolling();
      const delay =
        WS_RETRY_MS[Math.min(this._wsRetry, WS_RETRY_MS.length - 1)];
      this._wsRetry += 1;
      this._wsRetryTimer = setTimeout(this.connectWs, delay);
    };
  };

  /** 兜底轮询（WS 正常时不跑） */
  private startPolling = () => {
    if (this._pollTimer) {
      return;
    }
    this._pollTimer = setInterval(this.refresh, POLL_MS);
  };

  private stopPolling_ = () => {
    if (this._pollTimer) {
      clearInterval(this._pollTimer);
      this._pollTimer = null;
    }
  };

  /** 页面卸载时的整体拆卸：断开推送、清掉全部定时器（沿用原有调用名） */
  public stopPolling = () => {
    this._wsClosing = true;
    // 推进世代号：当前连接迟到的 onclose 会认出自己已过期而不重连
    this._wsGen += 1;
    if (this._wsRetryTimer) {
      clearTimeout(this._wsRetryTimer);
      this._wsRetryTimer = null;
    }
    this._ws?.close();
    this._ws = null;
    this.stopPolling_();
    if (this._deviceTimer) {
      clearInterval(this._deviceTimer);
      this._deviceTimer = null;
    }
  };

  /**
   * 落地一份会话列表快照。WS 推送与兜底轮询共用，保证两条通路行为一致。
   */
  private applyTasks = (list: PortalTaskData[]) => {
    // 同一个终端进程换新会话（新 id/jsonl）时，pid 会从旧会话挪到新会话 —— /clear、
    // compact 如此，占位任务头一回落盘配上会话文件也如此。先记下旧表里各会话的 pid，
    // 换表后据此把打开的格子跟过去：否则要么卡在已失联（无 pid）的旧会话上「下发失败、
    // 终端没这个任务」，要么整个格子被清掉退回空态。
    const prevPidById = new Map<string, number | null | undefined>();
    for (const t of this._tasks) prevPidById.set(t.id ?? "", t.pid);

    // 内容没变就不换引用，否则整棵会话树白重渲染一遍。
    if (JSON.stringify(list) !== JSON.stringify(this._tasks)) {
      this._tasks = list;
    }

    // pid 跟随：打开的格子原地跟到继任会话，靠 pid 认人（前后是同一个 agent 进程）。
    // 两种换 id 的场景：
    //   ① /clear、compact —— 旧会话还留在列表里，只是把 pid 让给了新会话；
    //   ② 进程占位任务（只扫到进程、还没配上 jsonl，id 形如 `<machine>-pid-<pid>`）收到
    //      第一条输入后落了盘、配上真会话 —— 旧任务整条从列表消失，换成真会话 id。
    // ② 不跟随的话，刚给空终端下发完任务，格子就被下面的存活清理踢掉、退回空态，
    // 用户还得再点一次新冒出来的会话才能接着看。
    const carried: Array<[string, string]> = [];
    const followed = this._openIds.map((id) => {
      const prevPid = prevPidById.get(id);
      if (!prevPid) {
        return id;
      }
      // 还持有原 pid 就没换人（会话仍在，绝大多数刷新走这条）
      if (this._tasks.find((t) => t.id === id)?.pid) {
        return id;
      }
      const succ = this._tasks.find((t) => t.pid === prevPid && t.id !== id);
      if (!succ?.id) {
        return id;
      }
      carried.push([id, succ.id]);
      return succ.id;
    });
    // 本地回显（刚发出、终端还没同步回来的那条）跟着搬家，否则存活清理连它一起丢，
    // 看着就像「刚发的消息凭空没了」。继任者已有内容时不覆盖，只清掉旧账。
    //
    // **只搬本地回显，绝不搬旧会话的历史**：继任会话是另一份 jsonl，内容由它自己
    // 说了算。把旧账整份倒过去，配上 fetchMessages 的「累积合并（只增不减）」，
    // 旧内容就再也退不掉了 —— 现象是下发 `/clear` 后终端已经清空，网页对话流却
    // 还挂着清空前的全部内容，要切走再切回（走 dropMessageCache 清缓存）才正常。
    if (carried.length) {
      const next = { ...this._messagesById };
      carried.forEach(([from, to]) => {
        const echoes = (next[from] ?? []).filter((m) => m.local);
        if (echoes.length && !next[to]?.length) {
          next[to] = echoes;
        }
        delete next[from];
      });
      this._messagesById = next;
    }
    // 跟随后可能与已打开的会话撞车，去重保序
    const deduped = Array.from(new Set(followed));
    if (deduped.join("\u0000") !== this._openIds.join("\u0000")) {
      this._openIds = deduped;
    }

    // 仅做存活清理：已消失的会话从打开列表里剔除。
    //
    // 判据走 `taskOf` 而不是「在不在 `_tasks` 里」：`_tasks` 只有活跃会话，
    // 而打开的可能是一条历史会话（资料在 `_histById`）或一条子会话（复合 id，
    // 靠父会话认人）。按老判据，这两种只要下一次推送一到就被当场关掉。
    const alive = this._openIds.filter((id) => !!this.taskOf(id));
    if (alive.length !== this._openIds.length) {
      this._openIds = alive;
      this.dropMessageCache();
    }
    // 对话内容不走推送（推送只含会话列表），仍按需拉取（跟随后的新会话首拉即有内容）
    this._openIds.forEach((id) => this.fetchMessages(id, false));
  };

  public refresh = () => {
    getPortalTaskList()
      .then((res) => {
        if (res.code === 0) {
          this.applyTasks(res.data?.list ?? []);
        }
      })
      .catch(() => {});
  };

  public loadDevices = () => {
    getPortalDevices()
      .then((res) => {
        if (res.code === 0) {
          const list = res.data?.list ?? [];
          // 同 refresh：内容没变就保持引用，避免 2s 一次的无谓重渲染
          if (JSON.stringify(list) !== JSON.stringify(this._devices)) {
            this._devices = list;
          }
        }
      })
      .catch(() => {});
  };

  public trustDevice = (id: string) => {
    trustPortalDevice(id)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success("已信任该设备");
          this.loadDevices();
          this.refresh();
        } else {
          antdMessage.error(res.msg ?? "操作失败");
        }
      })
      .catch(() => antdMessage.error("信任设备失败，请检查网络"));
  };

  public untrustDevice = (id: string) => {
    untrustPortalDevice(id)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success("已撤销信任");
          this.loadDevices();
          this.refresh();
        } else {
          antdMessage.error(res.msg ?? "操作失败");
        }
      })
      .catch(() => antdMessage.error("撤销信任失败，请检查网络"));
  };

  public deleteDevice = (id: string) => {
    deletePortalDevice(id)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success("已删除设备");
          this.loadDevices();
          this.refresh();
        } else {
          antdMessage.error(res.msg ?? "操作失败");
        }
      })
      .catch(() => antdMessage.error("删除设备失败，请检查网络"));
  };

  /**
   * 给会话起名 / 改名；`note` 传空串 = 清除，标题退回自动推断的那个。
   *
   * 成功后**当场把本地这条改掉**，不等下一次推送：WS 推的是整份会话列表，
   * 名字改完却要隔一拍才变，用起来像没保存上。推送随后会带着同样的 note 回来，
   * 覆盖上去是同一个值，不会打架。
   *
   * @returns 是否保存成功（调用方据此决定收起编辑框还是留着让用户改）
   */
  public setNote = async (id: string, note: string): Promise<boolean> => {
    try {
      const res = await setPortalTaskNote(id, note);
      if (res.code !== 0) {
        // 超长等业务错误：后端 msg 说得比任何本地兜底文案都准（带实际字数）
        antdMessage.error(res.msg ?? "保存失败");
        return false;
      }
      const saved = res.data?.note ?? null;
      this._tasks = this._tasks.map((t) =>
        t.id === id ? { ...t, note: saved } : t,
      );
      antdMessage.success(saved ? "已重命名" : "已清除备注，标题恢复自动生成");
      return true;
    } catch (e) {
      // 带响应的失败（400/404 等）由响应拦截器把服务端原话弹出来了（见 Axios.ts），
      // 这里再补一条只会盖住它；真正没人吭声的只有「请求根本没发出去」。
      if (!(e && typeof e === "object" && "response" in e)) {
        antdMessage.error("保存失败，请检查网络");
      }
      return false;
    }
  };

  /** 单击会话：替换为单格视图 */
  public select = (id: string) => {
    if (this._openIds.length === 1 && this._openIds[0] === id) {
      return;
    }

    this._openIds = [id];
    // 从多格切回单格时，被挤掉的会话缓存必须一起丢：
    // refresh 只在会话消失时清理，单纯换视图不会触发它。
    this.dropMessageCache();
    this.fetchMessages(id, true);
  };

  /** 拆分：加入右侧网格 */
  public splitOpen = (id: string) => {
    if (this._openIds.includes(id)) {
      return;
    }
    if (this._openIds.length >= MAX_PANES) {
      antdMessage.warning(`最多同时显示 ${MAX_PANES} 个会话`);
      return;
    }

    this._openIds = [...this._openIds, id];
    this.fetchMessages(id, true);
  };

  public closePane = (id: string) => {
    this._openIds = this._openIds.filter((x) => x !== id);
    this.dropMessageCache();
  };

  /** 丢掉已关闭会话的消息缓存：不清理的话这张表只增不减，长时间挂着会一直涨 */
  private dropMessageCache = () => {
    const keep: Record<string, PortalMessage[]> = {};
    this._openIds.forEach((id) => {
      const msgs = this._messagesById[id];
      if (msgs) {
        keep[id] = msgs;
      }
    });
    this._messagesById = keep;
  };

  /**
   * 主动同步某个会话：丢掉本地累积的消息后重新拉取。
   *
   * 平时的轮询走「累积合并」（滑动窗口会丢老消息，故只增不减）。代价是本地
   * 副本一旦与终端实际对不上，后续轮询只会往上叠，自己纠不回来。
   * 这里直接清空重来，作为对不上时的兜底手段。
   */
  public syncMessages = (id: string) => {
    if (!id) {
      return;
    }
    const next = { ...this._messagesById };
    delete next[id];
    this._messagesById = next;
    // 重新同步 = 从头来过：重试计数一并归零，否则之前问满 6 次的会话再也不会重试
    const tries = { ...this._pendingTries };
    delete tries[id];
    this._pendingTries = tries;
    // 重试就是「从头来过」：上一次的失败原因先清掉，不然重试期间还挂着旧提示
    this.setBodyFail(id, undefined);
    this.fetchMessages(id, true);
  };

  /** 会话在 hub 队列里待下发的输入（任何端发的都在这，供跨端显示） */
  public hubQueuedOf = (id: string) =>
    this._hubQueuedById[id] ?? EMPTY_HUB_QUEUED;

  /**
   * 拉取 hub 队列：既用来去掉本地回显的排队标记，也用来同步**别的端**发的任务。
   *
   * 这里原先有个短路 —— 本端没有 `local && queued` 的回显就直接返回、连请求都不发。
   * 那样一来「手机发、电脑看」永远看不到：别的端发的任务在本端没有任何本地回显，
   * 短路条件恒不成立。接口本来就把 text 一起返回了，白白丢掉。
   */
  private refreshQueued = (id: string) => {
    getQueuedInputs(id)
      .then((res) => {
        if (res.code !== 0) {
          return;
        }
        const list = (res.data?.list ?? []).map((x) => ({
          cmdId: x.cmdId ?? "",
          text: x.text ?? "",
        }));
        // 内容没变就不换引用，避免每轮轮询都触发重渲染
        const prev = this._hubQueuedById[id] ?? EMPTY_HUB_QUEUED;
        if (JSON.stringify(list) !== JSON.stringify(prev)) {
          this._hubQueuedById = { ...this._hubQueuedById, [id]: list };
        }
        const still = new Set(list.map((x) => x.cmdId));
        const cur = this._messagesById[id] ?? [];
        if (cur.some((m) => m.local && m.queued && !still.has(m.cmdId ?? ""))) {
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.map((m) =>
              m.local && m.queued && !still.has(m.cmdId ?? "")
                ? { ...m, queued: false, delivered: true }
                : m,
            ),
          };
        }
      })
      .catch(() => void 0);
  };

  /** 撤回后把原文回填给对应会话的对话框（供 Composer 监听消费） */
  public consumeComposerRefill = () => {
    this.composerRefill = null;
  };

  /**
   * 向终端注入按键：撤回终端原生排队（up，按 count 次）/ 插入排队到会话（esc）。
   * 仅 iTerm2(mac) 与 Windows 控制台可干净注入；Terminal.app 会失败并提示手动按键。
   */
  public termKey = (id: string, key: "up" | "esc", count = 1) => {
    termKeyTask(id, key, count)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(
            key === "up" ? "已撤回终端排队" : "已插入排队到会话",
          );
          // 撤回后把「已送达终端、尚未执行」的本地回显一并清掉。
          //
          // 不清的话排队条看着像「撤回了却还在」：那一条其实由两份数据接力显示 ——
          // 排队条优先铺 queuedInputs，同内容的本地回显被去重挡在后面；撤回让
          // queuedInputs 随下一轮扫描清空，而本地回显此刻多半还在 stillQueued 的
          // 6 秒宽限里，于是立刻顶替上来占住同一个位置。
          //
          // 判据 `local && !queued`：queued 为真的还在 hub 队列、压根没到终端，
          // 归 recallAllQueued 按 cmdId 精确撤，不能在这里一并抹掉。
          if (key === "up") {
            const cur = this._messagesById[id] ?? [];
            const kept = cur.filter((m) => !(m.local && !m.queued));
            if (kept.length !== cur.length) {
              this._messagesById = { ...this._messagesById, [id]: kept };
            }
          }
        } else {
          antdMessage.warning(res.msg ?? "按键注入失败");
        }
      })
      .catch(() => antdMessage.error("操作失败，请检查网络"));
  };

  /**
   * 一起撤回底部挂载的多条排队任务，并把它们的原文合并回填进对话框。
   * 只有仍在 hub 队列（带 cmdId、还没被终端取走）的能真正撤回；已进终端原生队列的
   * 撤不回（claude 不开放出队），这里只负责把可撤的撤掉、并把全部文本回填供改后再发。
   */
  public recallAllQueued = (id: string, cmdIds: string[], allText: string) => {
    cmdIds.forEach((cmdId) =>
      recallPortalInput(id, cmdId)
        .then((res) => {
          if (res.code === 0) {
            const cur = this._messagesById[id] ?? [];
            this._messagesById = {
              ...this._messagesById,
              [id]: cur.filter((m) => !(m.local && m.cmdId === cmdId)),
            };
          }
        })
        .catch(() => void 0),
    );
    if (allText.trim()) {
      this.composerRefill = {
        taskId: id,
        text: allText,
        nonce: ++this._refillNonce,
      };
    }
    antdMessage.success("已撤回排队任务");
  };

  /**
   * 撤回还在排队的输入（已被终端接收则提示失败并去掉排队标记）。
   *
   * `fallbackText` = 排队条那一行显示的原文。原文优先取本地回显，但**不是每条排队
   * 都有回显**：斜杠命令一开始就不建回显（见 sendInput），别的端发来的任务在本端
   * 也只有 hub 队列这一份。缺了它，撤回就只是撤回，对话框空着，想改完再发得重打。
   */
  public recallInput = (id: string, cmdId: string, fallbackText?: string) => {
    recallPortalInput(id, cmdId)
      .then((res) => {
        const cur = this._messagesById[id] ?? [];
        if (res.code === 0) {
          antdMessage.success("已撤回");
          // 撤下排队回显，同时把原文回填进该会话的对话框，方便改完再发
          const recalled = cur.find((m) => m.local && m.cmdId === cmdId);
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.filter((m) => !(m.local && m.cmdId === cmdId)),
          };
          const refill = recalled?.content || fallbackText;
          if (refill) {
            this.composerRefill = {
              taskId: id,
              text: refill,
              nonce: ++this._refillNonce,
            };
          }
        } else {
          antdMessage.warning(res.msg ?? "已被终端接收，无法撤回");
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.map((m) =>
              m.local && m.cmdId === cmdId ? { ...m, queued: false } : m,
            ),
          };
        }
      })
      .catch(() => antdMessage.error("撤回失败，请检查网络"));
  };

  /**
   * 标记 / 撤销「正文还在路上」，并在需要时排下一次重试。
   *
   * @returns 是否已经安排了重试（调用方据此知道「这一轮别把它当成空会话」）
   */
  private markPending = (
    id: string,
    pending: boolean,
    empty: boolean,
    retry: () => void,
  ): boolean => {
    const tries = this._pendingTries[id] ?? 0;
    // 只有「说了 pending 且一条都没拿到」才算还在路上：拿到了内容就先显示，
    // 后续轮询会把剩下的补齐，没必要让人对着「读取中」干等。
    const waiting = !!pending && empty && tries < PENDING_RETRY_MAX;
    if (waiting) {
      if (!this._pendingIds.includes(id)) {
        this._pendingIds = [...this._pendingIds, id];
      }
      this._pendingTries = { ...this._pendingTries, [id]: tries + 1 };
      setTimeout(() => {
        // 期间被关掉了就别再问了
        if (this._openIds.includes(id)) {
          retry();
        }
      }, PENDING_RETRY_MS);
      return true;
    }
    if (this._pendingIds.includes(id)) {
      this._pendingIds = this._pendingIds.filter((x) => x !== id);
    }
    if (!pending && this._pendingTries[id]) {
      const next = { ...this._pendingTries };
      delete next[id];
      this._pendingTries = next;
    }
    return false;
  };

  // ---------- 子代理正文（执行链里的智能体卡）----------

  /** 子代理正文的缓存键。父会话 id 是 uuid、agentId 是 `agent-xxxx`，都不含 `|` */
  private subKey = (parentId: string, agentId: string) =>
    `${parentId}|${agentId}`;

  /** 这个子代理走过的那条链的原始消息。结构与主会话完全一致，可直接复用链渲染 */
  public subAgentMessagesOf = (
    parentId: string,
    agentId: string,
  ): PortalMessage[] =>
    this._subMsgsById[this.subKey(parentId, agentId)] ?? EMPTY_MESSAGES;

  public isSubAgentLoading = (parentId: string, agentId: string): boolean =>
    this._subMsgsLoading.includes(this.subKey(parentId, agentId));

  /** 正文还在路上（那台机器正在现读磁盘）。**不要当成空**，那是两回事 */
  public isSubAgentPending = (parentId: string, agentId: string): boolean =>
    this._subMsgsPending.includes(this.subKey(parentId, agentId));

  public subAgentFailOf = (
    parentId: string,
    agentId: string,
  ): FetchFailKind | undefined =>
    this._subMsgsFail[this.subKey(parentId, agentId)];

  /**
   * 拉一个子代理的正文（点开那张小卡时调）。
   *
   * 与主会话正文最大的不同：这份是一次性读盘的快照，**整份替换**即可 ——
   * 主会话那边的累积合并是为了对付「滑动窗口会丢老消息」的实时流，子代理没有
   * 这个问题，套上去只会把两次读盘的结果叠成重复内容。
   *
   * `pending` 要重试：hub 得点名让那台机器现读磁盘，一次往返两轮上报（最多 8 秒）。
   * 失败原因走与主会话同一套 `failKindOf`（离线 500 / 不存在 404 + 设备 online
   * 现场复核），不另写一套判断。
   */
  public loadSubAgentMessages = (
    parentId: string,
    agentId: string,
    force = false,
    tries = 0,
  ) => {
    if (!parentId || !agentId) {
      return;
    }
    const key = this.subKey(parentId, agentId);
    if (this._subMsgsLoading.includes(key)) {
      return;
    }
    /* 拿到过就不再问：读盘代价高，而**跑完之后这份内容不会再变**。
       还在跑的那些不走这条路 —— 它们由展开着的那条子链每 5 秒带 `force` 刷一次
       （见 TerminalFeed 的 SubAgentChain）：这是个监控工具，点开一个正在跑的
       子代理却只看到一张静止快照，等于把「执行中看不到正在执行的内容」又演一遍。 */
    if (!force && tries === 0 && this._subMsgsById[key]) {
      return;
    }
    this._subMsgsLoading = [...this._subMsgsLoading, key];
    /* 重试计数归零，但**不提前清失败原因**：跑着的子代理每 5 秒自动刷一次
       （见 TerminalFeed 的 SubAgentChain），清了又置回去会让「设备离线」那一行
       一闪一闪。成功路径本来就会清，失败也不会被后台刷新静默吞掉。
       `tries > 0` 是 pending 重试链自己在往下走，别把它的计数踩回 0。 */
    if (force && tries === 0) {
      this._subMsgsTries = { ...this._subMsgsTries, [key]: 0 };
    }
    getPortalSubAgentMessages(parentId, agentId, 200)
      .then((res) => {
        this._subMsgsLoading = this._subMsgsLoading.filter((x) => x !== key);
        if (res.code !== 0) {
          this.setSubMsgsFail(key, this.failKindOf(parentId, res.code));
          this._subMsgsPending = this._subMsgsPending.filter((x) => x !== key);
          return;
        }
        const list = res.data?.list ?? [];
        const done = this._subMsgsTries[key] ?? tries;
        // pending 且一条都没有 = 那台机器还没送回来，过一会儿再问
        if (res.data?.pending && !list.length && done < PENDING_RETRY_MAX) {
          if (!this._subMsgsPending.includes(key)) {
            this._subMsgsPending = [...this._subMsgsPending, key];
          }
          this._subMsgsTries = { ...this._subMsgsTries, [key]: done + 1 };
          setTimeout(
            () => this.loadSubAgentMessages(parentId, agentId, true, done + 1),
            PENDING_RETRY_MS,
          );
          return;
        }
        this._subMsgsPending = this._subMsgsPending.filter((x) => x !== key);
        this.setSubMsgsFail(key, undefined);
        this._subMsgsById = { ...this._subMsgsById, [key]: list };
      })
      .catch(() => {
        this._subMsgsLoading = this._subMsgsLoading.filter((x) => x !== key);
        this._subMsgsPending = this._subMsgsPending.filter((x) => x !== key);
        this.setSubMsgsFail(key, "network");
      });
  };

  /** 记一笔 / 清掉某个子代理正文的失败原因 */
  private setSubMsgsFail = (key: string, kind?: FetchFailKind) => {
    if (this._subMsgsFail[key] === kind) {
      return;
    }
    const next = { ...this._subMsgsFail };
    if (kind) {
      next[key] = kind;
    } else {
      delete next[key];
    }
    this._subMsgsFail = next;
  };

  public fetchMessages = (id: string, showLoading: boolean) => {
    if (!id) {
      return;
    }
    this.refreshQueued(id);
    if (showLoading && !this._loadingIds.includes(id)) {
      this._loadingIds = [...this._loadingIds, id];
    }

    getPortalTaskMessages(id, 200)
      .then((res) => {
        if (!this._openIds.includes(id)) {
          return;
        }
        if (res.code !== 0) {
          // 设备离线（500）、会话不存在（404）—— 两条路径的提示不一样，不合并成兜底
          this.setBodyFail(id, this.failKindOf(id, res.code));
          this._pendingIds = this._pendingIds.filter((x) => x !== id);
        }
        if (res.code === 0) {
          this.setBodyFail(id, undefined);
          const list = res.data?.list ?? [];
          // 历史会话的正文是 hub 点名让那台机器现读磁盘取回来的，一次往返要两轮上报。
          // 这一轮还没到 = **不是空会话**，标一下「读取中」、过 2.5 秒再问一次。
          if (
            this.markPending(id, !!res.data?.pending, !list.length, () =>
              this.fetchMessages(id, false),
            )
          ) {
            this._loadingIds = this._loadingIds.filter((x) => x !== id);
            return;
          }
          // 「当前状态」快照（todos / bgtasks）**不能走下面那套累积去重**：
          // 它们是每轮重算的当前状态，新的一份必须整个顶掉旧的。
          //
          // 混进去会被静默丢掉：去重键是 `时间戳|role|全文长度|前 60 字`，而快照的时间戳
          // 取自最后一条对话消息 —— 父会话闲着时它一动不动；`running` 与 `stopped` 又
          // 恰好都是 7 个字符，全文长度分毫不差；前 60 字则止步于第一条任务的 status 之前
          // —— `[{"id":"<17 位>","label":"` 固定占 36 字，首条 label 满 12 字时，status 的值
          // 就落到下标 60 开外切不进来了（实测 11 字还切得到、12 字起切不到；线上那两条
          // 是 13、14 字）；若翻转的不是首条，那更是与首条 label 长短无关，必撞。三段全撞，
          // 新旧快照的键完全一样。实测线上会话 7d4a7af0 那份 2371 字节的快照，把两条
          // running 改成 stopped 之后键逐字节相同 —— 于是「子会话已经不在跑了」这个更新
          // 永远递不到界面上，胶囊一直挂着。
          // （`completed` 是 9 个字符、长度会变，所以只有 stopped 这条路被吞，更阴。）
          const incoming = list.filter((m) => !isStateSnapshot(m.role));
          const snapshots = list.filter((m) => isStateSnapshot(m.role));
          // 累积合并：拉取是滑动窗口会丢老消息，这里按 key 去重后只增不减，
          // 保证对话流稳定增长、不因窗口滑动丢历史。
          const prevSnapshots = (this._messagesById[id] ?? []).filter((m) =>
            isStateSnapshot(m.role),
          );
          let prev = (this._messagesById[id] ?? []).filter(
            (m) => !isStateSnapshot(m.role),
          );
          // key 带上全文长度，降低同时间戳+同前缀不同消息被误判重复的概率
          const mkey = (m: PortalMessage) =>
            `${m.timestamp}|${m.role}|${m.content.length}|${m.content.slice(0, 60)}`;
          // 先算「这一轮新出现的消息」，回显接管只认它们（理由见 echoTakenOver）。
          const seen = new Set(prev.map(mkey));
          const fresh = incoming.filter((m) => {
            const k = mkey(m);
            if (seen.has(k)) {
              return false;
            }
            seen.add(k);
            return true;
          });
          // 终端同步回了同内容的 user 消息 → 撤下对应的本地乐观回显，
          // 让真实消息（带终端时间戳）接管，避免同一条显示两遍。
          // 归一化比对：空白差异（换行/缩进/首尾）一律视为同一条
          const norm = (s: string) => s.replace(/\s+/g, " ").trim();
          // 回显能否被某条同步回来的 user 消息接管：
          // 除了完全相等，还接受「同步内容包含回显全文」——终端把注入的文本
          // 记进 jsonl 时常会带上结构化前后文（工具结果、上下文块等），导致内容
          // 比原始输入更长，只做全等比对会漏判、两条并存。长度阈值挡掉过短回显
          // （如 “ok”）被任意长消息命中的误伤。
          //
          // **只认这一轮新出现的消息（fresh），不比时间戳**。
          // 需要「只认新的」是因为 incoming 是整个滑动窗口、里头全是历史：不加约束的话，
          // 会话里任何一条旧消息只要含这段文本，就会把刚发出去的回显判成「已被接管」
          // 而撤下，正文里根本看不到自己刚发的内容。
          // 但这个约束**不能拿时间戳来做**：回显的时间戳是**看的这台机器**的浏览器时钟，
          // 同步回来的消息的时间戳是**跑终端那台机器**写进 jsonl 的时钟，两者毫无关系。
          // 原先的 `u.at >= echoAt - 5000` 等于假设两台机器的钟差不超过 5s——只要看的
          // 这端快一点，接管就**永久**失效：回显一直留着，真实消息又照常进流，
          // 于是排队清掉的那一刻同一句话在对话流里冒出两条一模一样的气泡。
          // 「新出现」本身就是顺序事实（回显先于本轮响应被处理），不需要任何时钟。
          const freshUserTexts = fresh
            .filter((m) => m.role === "user")
            .map((m) => norm(m.content));
          const echoTakenOver = (echoNorm: string) => {
            if (!echoNorm) {
              return false;
            }
            return freshUserTexts.some(
              (t) =>
                t === echoNorm ||
                (echoNorm.length >= 4 && t.includes(echoNorm)),
            );
          };
          const withoutEcho = prev.filter((m) => {
            if (!m.local) {
              return true;
            }
            if (echoTakenOver(norm(m.content))) {
              return false;
            }
            // 回显一旦送达终端就**永久保留**，直到被同步回来的真实消息接管。
            //
            // 这里原先有条「超 5 分钟就撤下」的自愈：那是为了避免它和真实消息
            // 一起显示出重复观感。但注入终端的输入在 jsonl 里常常压根不写 user
            // 记录（只留一条 queue-operation，而那类记录不进对话流），于是永远
            // 等不到接管 —— 撤下就是永久消失。执行中下发的任务要在队列里排上
            // 好几分钟，正好撞线，表现就是「下发的任务有概率被吞掉」。
            //
            // 现在对话流里的回显就是条普通用户气泡（排队状态与撤回已归排队条），
            // 留着它不会造成任何重复观感；真来了同名消息也有 echoTakenOver 兜着。
            return true;
          });
          const echoReplaced = withoutEcho.length !== prev.length;
          prev = withoutEcho;
          // 状态快照按内容比，变了就整份换掉（它是替换语义，不是追加语义）
          const sameSnapshots =
            prevSnapshots.length === snapshots.length &&
            prevSnapshots.every((m, i) => m.content === snapshots[i].content);
          // 无新消息就不换引用：轮询每 2s 一次，无条件替换会让整条对话流
          // 每 2s 白重渲染一遍（长会话下明显掉帧）。
          if (fresh.length || echoReplaced || !sameSnapshots) {
            // 单会话上限：只增不减的合并会随长会话无限涨，超出后丢最老的。
            const merged = [...prev, ...fresh];
            const kept =
              merged.length > MAX_MESSAGES_PER_TASK
                ? merged.slice(-MAX_MESSAGES_PER_TASK)
                : merged;
            // 快照挂在末尾：parseLast 从后往前找，取到的就是最新这份
            this._messagesById = {
              ...this._messagesById,
              [id]: [...kept, ...snapshots],
            };
          }
        }

        this._loadingIds = this._loadingIds.filter((x) => x !== id);
      })
      .catch(() => {
        this._loadingIds = this._loadingIds.filter((x) => x !== id);
        this.setBodyFail(id, "network");
      });
  };

  public control = (id: string, action: PortalControlAction) => {
    const task = this._tasks.find((t) => t.id === id);
    if (!task?.id) {
      return;
    }

    controlPortalTask(task.id, action, task.pid)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(res.data?.result ?? "操作成功");
          this.refresh();
        } else {
          antdMessage.error(res.msg ?? "操作失败");
        }
      })
      .catch(() => antdMessage.error("操作失败，请检查网络"));
  };

  /** 向会话发布任务（注入一行输入） */
  /**
   * 向会话发一行输入。
   *
   * `fromSelect` = 这是在回答终端弹出的选择卡（选项序号或自定义答案）。
   * 那类内容对着对话流念出来毫无意义 —— 孤零零一个「1」「2」，看不出在答什么，
   * 问题本身又不在流里（选择卡挂在输入框上方）。标记出来，让它不入流。
   */
  public sendInput = (
    id: string,
    text: string,
    opts?: { fromSelect?: boolean },
  ) => {
    const task = this._tasks.find((t) => t.id === id);
    const content = text.trim();
    if (!task?.id || !content) {
      return Promise.resolve(false);
    }

    return sendPortalInput(task.id, content, task.pid, opts?.fromSelect)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(res.data?.result ?? "已发送");
          // 乐观回显：发出的内容立即上屏为 user 气泡，
          // 不等终端收到再同步回来（那要好几秒，体感像没发出去）。
          //
          // 斜杠命令除外，**一条回显都不建**：CLI 自己把它吃掉，既不写 jsonl 的 user
          // 记录、也不进 queued_inputs，于是回显唯一的退场路径（被同步回来的真实消息
          // 接管）永远不会发生 —— 留下的就是撤不掉的孤儿气泡（`/clear` 之后新会话里
          // 那条孤零零的「/clear」）。判据见 _utils/slashCommand。
          // 反馈不靠回显：上面的 toast 已经确认发出，若排上了 hub 队列，下一轮
          // getQueuedInputs 就把它铺进底部排队条（带 cmdId，撤回照常可用）。
          if (!isSlashCommand(content)) {
            const echo = {
              role: "user",
              content,
              timestamp: new Date().toISOString(),
              local: true,
              cmdId: res.data?.cmdId,
              queued: !!res.data?.cmdId,
              fromSelect: opts?.fromSelect,
            };
            this._messagesById = {
              ...this._messagesById,
              [id]: [...(this._messagesById[id] ?? []), echo],
            };
          }
          setTimeout(() => this.fetchMessages(id, false), 1200);
          return true;
        }

        antdMessage.error(res.msg ?? "发送失败");
        return false;
      })
      .catch(() => {
        // 必须吞成 false 返回给调用方：Composer 靠返回值决定要不要把
        // 输入框内容还给用户，抛出去会让草稿连同报错一起丢掉。
        antdMessage.error("发送失败，请检查网络");
        return false;
      });
  };
}

export default new PortalStore();
