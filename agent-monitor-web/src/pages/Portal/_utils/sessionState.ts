import { PortalMessage } from "@/services/apis/portal";

/**
 * 会话的「当前状态」快照解析。
 *
 * 后端（am-core scanner）把任务清单与后台任务作为两条状态快照消息挂在消息流末尾，
 * 每次只产出最终一份。头部的子会话胶囊与右侧的 SessionPanels 都吃这份数据，
 * 过滤口径必须一致 —— 所以集中在这里，两处共用，别各写各的。
 */

/**
 * 「当前状态」快照消息的 role。
 *
 * 这两条与对话消息**语义相反**：对话是只增不减的事件流，它们是每轮重算的当前状态，
 * 新的一份就该整个顶掉旧的。混进对话流的累积去重里会被当成重复丢掉 —— 见
 * PortalStore.fetchMessages 里的说明。
 */
export const STATE_ROLES = ["todos", "bgtasks"];

/** 这条消息是状态快照而非对话内容 */
export const isStateSnapshot = (role: string) => STATE_ROLES.includes(role);

/** 任务清单里的一项（后端 todos 消息的 JSON 结构） */
export interface TodoItem {
  id: string;
  subject: string;
  status: string;
}

/** 后台运行的任务（后端 bgtasks 消息的 JSON 结构） */
export interface BgTask {
  id: string;
  label: string;
  status: string;
  /** "agent"=异步子代理 / "bg"=后台命令（缺省按后台命令） */
  kind?: string;
  /** 起跑时刻（ISO8601）。老客户端上报的数据里可能没有，此时不显示耗时 */
  startedAt?: string;
  /**
   * **为什么是这个收场**：完成通知里 `<summary>` 的英文原文，≤300 字符、单行。
   *
   * 退出码、限流原因、卡死时长都嵌在这一句话里（`Agent "X" failed: Agent stalled:
   * no progress for 600s`）。**原样显示，不解析**：那是上游随时会改的英文文案，
   * 去里面抠 `exit code (\d+)` 就是拿字面量当接口用，上游改一版就整条哑掉。
   *
   * 只有收到完成通知才有值；后端为空时该键整个不出现（不是空串），
   * 老客户端上报的数据里没有它，退回改前的样子（只说「哪一条、什么收场」）。
   */
  summary?: string;
}

/**
 * **正常跑完**的后台任务：结论已经并回主对话，清单里不必再占位置。
 *
 * 这里曾经把 `failed / killed / stopped` 一起算作「已结束」滤掉 —— 于是一个跑砸的
 * 子代理在界面上是**无声消失**的：它先显示「执行中」，某一刻自己没了，既没有失败提示
 * 也没有痕迹，人只会以为它跑完了。而恰恰是失败/被终止这几种收场需要被看见：
 * 成功的产出会出现在正文里，失败的什么都不会留下。
 */
export const BG_DONE = ["completed"];

/**
 * **异常收场**的后台任务：跑砸了、被杀了、被停了。
 *
 * 与 `BG_DONE` 分开的原因见上：这几种不能滤掉，要留在清单里并标成失败态。
 * 失败原因走 [`BgTask.summary`]（后端从完成通知的 `<summary>` 里带出来），
 * 所以这几条现在说得出「为什么」，不只是「什么收场」。
 */
export const BG_FAILED = ["failed", "killed", "stopped"];

/** 这条后台任务是异常收场（跑砸 / 被终止） */
export const isBgFailed = (status: string) => BG_FAILED.includes(status);

/** 后台任务状态的中文说法 */
export const BG_LABEL: Record<string, string> = {
  running: "执行中",
  pending: "等待中",
  queued: "等待中",
  failed: "失败",
  killed: "已终止",
  stopped: "已停止",
};

/**
 * 取某个 role 的最后一条并解析成数组。
 * 后端对这两类「状态快照」只产出最终一份，所以这里取到的就是当前状态 ——
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
 * 值得展示的后台任务：还在跑的、在等的，**以及跑砸/被终止的**。
 * 只有正常跑完的那些掉出去（理由见 `BG_DONE`）。
 */
export const aliveBgTasks = (messages: PortalMessage[]): BgTask[] =>
  parseLast<BgTask>(messages, "bgtasks").filter(
    (t) => !BG_DONE.includes(t.status),
  );

/**
 * 还在跑的子会话（异步子代理）—— 头部胶囊专用。
 *
 * 胶囊上写的是「运行中的子会话 · N」，所以这里比 `aliveBgTasks` 多滤一道异常收场：
 * 失败的子代理该留在清单里被看见，但不该被数进「还在跑」的条数。
 */
export const runningSubAgents = (messages: PortalMessage[]): BgTask[] =>
  aliveBgTasks(messages).filter(
    (t) => t.kind === "agent" && !isBgFailed(t.status),
  );

/** 展示用的子会话（含失败/被终止的）—— 清单里要看得见 */
export const visibleSubAgents = (messages: PortalMessage[]): BgTask[] =>
  aliveBgTasks(messages).filter((t) => t.kind === "agent");

/** 值得展示的后台命令（子代理之外的那些，含失败/被终止的） */
export const aliveBgCommands = (messages: PortalMessage[]): BgTask[] =>
  aliveBgTasks(messages).filter((t) => t.kind !== "agent");

/** 一条会话此刻的「当前状态」：未完成的清单条目、活着的后台命令、子代理。 */
export interface SessionState {
  todos: TodoItem[];
  bgTasks: BgTask[];
  subAgents: BgTask[];
}

/**
 * 取一条会话的当前状态。
 *
 * **这一份是唯一定义**：状态卡（`SessionPanels`）自己渲染它，右栏
 * （`SessionStatePane`）还要先问「这条会话有没有东西可展示」才决定要不要给它一个
 * 标题。两处各写一遍筛选条件，改一处漏一处就会出现「右栏列了标题、底下却空着」。
 *
 * 清单只留没做完的：做完的条目没有关注价值。后台任务同理只留没正常跑完的，
 * 跑砸/被终止的**要留着**（理由见 `BG_DONE`）。
 */
export const sessionStateOf = (messages: PortalMessage[]): SessionState => ({
  todos: parseLast<TodoItem>(messages, "todos").filter(
    (t) => t.status !== "completed",
  ),
  bgTasks: aliveBgCommands(messages),
  subAgents: visibleSubAgents(messages),
});

/** 这条会话此刻没有任何可展示的状态 —— 状态卡整块不渲染、右栏不给它标题 */
export const isEmptySessionState = (s: SessionState): boolean =>
  !s.todos.length && !s.bgTasks.length && !s.subAgents.length;

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
