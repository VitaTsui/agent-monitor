import { PortalMessage, SubTask, SubTaskOutcome } from "@/services/apis/portal";

/**
 * 会话的「当前状态」解析。
 *
 * 两份来源，语义不同、别混：
 *   - **任务清单**（todos）仍是挂在消息流末尾的一条状态快照消息，每轮重算、新的顶掉旧的；
 *   - **子任务**（异步子代理 + 后台命令）走 `Task.subTasks` 这个结构化字段。
 *
 * 子任务从前也是一条伪消息（`role: "bgtasks"`），消费方得自己从对话里摘出来再
 * `JSON.parse`，还得记着别把它渲染成聊天气泡。**后端已经把那条消息删掉了**，
 * 这里一并改读字段；旧的解析路径整条拆掉，不保留两套判断。
 *
 * **子代理不在这一份里**：它是执行链上的一步（谁派的、派了什么、跑成什么样），
 * 由正文里的智能体卡就地展示（见 `_components/AgentCard`）。这里只剩「没有对应
 * 执行链节点」的那两样 —— 任务清单与后台命令 —— 它们在链上无处可挂，才需要一块状态区。
 */

/**
 * 「当前状态」快照消息的 role。
 *
 * 它与对话消息**语义相反**：对话是只增不减的事件流，它是每轮重算的当前状态，
 * 新的一份就该整个顶掉旧的。混进对话流的累积去重里会被当成重复丢掉 —— 见
 * PortalStore.fetchMessages 里的说明。
 *
 * 只剩 `todos` 一条：`bgtasks` 已被后端删除，改由 `Task.subTasks` 下发。
 */
export const STATE_ROLES = ["todos"];

/** 这条消息是状态快照而非对话内容 */
export const isStateSnapshot = (role: string) => STATE_ROLES.includes(role);

/** 任务清单里的一项（后端 todos 消息的 JSON 结构） */
export interface TodoItem {
  id: string;
  subject: string;
  status: string;
}

/**
 * 子任务四种收场的中文说法。
 *
 * **`interrupted` 是「已中断」不是「失败」**：它表示这条子会话被父会话退出连带终止，
 * 子代理本身一点毛病没有。上游对这种情况发的是 `killed`，实测是父会话被中断时
 * **一次性发给当时所有在跑子代理的统一通知**（同一时刻三条同状态）—— 按上游那个字面量
 * 配色的结果就是「按了一下 Esc，一排子代理全爆红」。归类由后端按磁盘事实
 * （子会话记录有没有交回结果）算好，所以这里只看 `outcome`，一个字面量都不匹配。
 */
export const SUB_OUTCOME_LABEL: Record<SubTaskOutcome, string> = {
  running: "执行中",
  completed: "已完成",
  failed: "失败",
  interrupted: "已中断",
};

/**
 * 这条子任务**它自己跑砸了** —— 只有这一种才画成告警红。
 *
 * `interrupted` 不算：那是被父会话连带终止的，标红等于冤枉它（用户第一反应是
 * 「明明正常结束了，为什么显示失败」）。
 */
export const isSubTaskFailed = (t: SubTask) => t.outcome === "failed";

/** 这条子任务还在跑 */
export const isSubTaskRunning = (t: SubTask) => t.outcome === "running";

/** 能点开看正文的子会话：有独立会话记录的异步子代理。后台命令没有正文 */
export const hasSubTaskBody = (t: SubTask) => t.kind === "agent" && t.hasBody;

/**
 * **值得展示**的后台命令：还在跑的，以及跑砸 / 被中断的。
 *
 * 只有 `completed` 掉出去 —— 正常跑完的后台命令没有关注价值。反过来，异常收场的
 * 必须留着：成功的产出会出现在正文里，失败的什么都不会留下，滤掉就等于让一个
 * 跑砸的后台命令在界面上**无声消失**。
 *
 * **这一条只管后台命令。** 子代理从前也走同一个过滤，于是跑完的子代理会从界面上
 * 消失；现在子代理归执行链管（链要的是完整的「它派过谁」，一条都不能少），
 * 那套过滤随旧设计一并撤销，不留第二个入口。
 */
export const aliveBgCommands = (list?: SubTask[]): SubTask[] =>
  (list ?? []).filter((t) => t.kind !== "agent" && t.outcome !== "completed");

/**
 * 一条会话此刻的「当前状态」：未完成的清单条目、活着的后台命令。
 *
 * **不含子代理** —— 它有自己的执行链节点（智能体卡），状态区再列一遍就是同一件事
 * 说两遍，而且那一份还得自己再定义一次「哪些算值得展示」。
 */
export interface SessionState {
  todos: TodoItem[];
  bgTasks: SubTask[];
}

/**
 * 取某个 role 的最后一条并解析成数组。
 * 后端对「状态快照」只产出最终一份，所以这里取到的就是当前状态 ——
 * 状态更新表现为原地刷新，而不是往对话流里堆一条新的。
 */
export function parseLast<T>(messages: PortalMessage[], role: string): T[] {
  const hit = [...messages].reverse().find((m) => m.role === role);
  if (!hit) {
    return [];
  }
  try {
    const parsed = JSON.parse(hit.content);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    // 解析不了就当没有，别把整个面板带崩
    return [];
  }
}

/**
 * 取一条会话的当前状态。
 *
 * **这一份是唯一定义**：状态卡（`SessionPanels`）自己渲染它，右栏
 * （`SessionStatePane`）还要先问「这条会话有没有东西可展示」才决定要不要给它一个
 * 标题。两处各写一遍筛选条件，改一处漏一处就会出现「右栏列了标题、底下却空着」。
 *
 * 清单只留没做完的：做完的条目没有关注价值。后台命令同理只留没正常跑完的
 * （理由见 `aliveBgCommands`）。
 */
export const sessionStateOf = (
  messages: PortalMessage[],
  subTasks?: SubTask[],
): SessionState => ({
  todos: parseLast<TodoItem>(messages, "todos").filter(
    (t) => t.status !== "completed",
  ),
  bgTasks: aliveBgCommands(subTasks),
});

/** 这条会话此刻没有任何可展示的状态 —— 状态卡整块不渲染、右栏不给它标题 */
export const isEmptySessionState = (s: SessionState): boolean =>
  !s.todos.length && !s.bgTasks.length;

/**
 * 耗时口语化：37秒 / 4分12秒 / 1小时3分。
 * 起跑时刻缺失或解析不了就返回空串（调用方据此不渲染这一段）。
 */
export function fmtElapsed(startedAt?: string, now = Date.now()): string {
  if (!startedAt) {
    return "";
  }
  const t = Date.parse(startedAt);
  if (Number.isNaN(t)) {
    return "";
  }
  const sec = Math.max(0, Math.floor((now - t) / 1000));
  if (sec < 60) {
    return `${sec}秒`;
  }
  const min = Math.floor(sec / 60);
  if (min < 60) {
    const rest = sec % 60;
    return rest ? `${min}分${rest}秒` : `${min}分`;
  }
  const hour = Math.floor(min / 60);
  const restMin = min % 60;
  return restMin ? `${hour}小时${restMin}分` : `${hour}小时`;
}

/**
 * 子任务的耗时：还在跑的走「到现在为止」，已经收场的走「跑了多久」。
 *
 * 收场的那些不能再跟着 `now` 走 —— 一条昨天就结束的子代理显示「17小时」是在说
 * 它跑了 17 小时，而它其实只跑了 40 秒。`endedMs` 为 0 表示还没结束。
 */
export const fmtSubTaskElapsed = (t: SubTask, now = Date.now()): string =>
  fmtElapsed(t.startedAt, t.endedMs > 0 ? t.endedMs : now);
