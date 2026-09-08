import { PortalMessage } from "@/services/apis/portal";

/**
 * 会话的「当前状态」快照解析。
 *
 * 后端（am-core scanner）把任务清单与后台任务作为两条状态快照消息挂在消息流末尾，
 * 每次只产出最终一份。头部的子会话胶囊与右侧的 SessionPanels 都吃这份数据，
 * 过滤口径必须一致 —— 所以集中在这里，两处共用，别各写各的。
 */

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
}

/** 已结束的后台任务状态：不再有关注价值，不展示 */
export const BG_DONE = ["completed", "failed", "killed", "stopped"];

/** 后台任务状态的中文说法 */
export const BG_LABEL: Record<string, string> = {
  running: "执行中",
  pending: "等待中",
  queued: "等待中",
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

/** 还没结束的后台任务（执行中 / 等待中）。跑完的一律不算 */
export const aliveBgTasks = (messages: PortalMessage[]): BgTask[] =>
  parseLast<BgTask>(messages, "bgtasks").filter(
    (t) => !BG_DONE.includes(t.status),
  );

/** 还在跑的子会话（异步子代理）—— 头部胶囊与子代理面板共用这一条口径 */
export const runningSubAgents = (messages: PortalMessage[]): BgTask[] =>
  aliveBgTasks(messages).filter((t) => t.kind === "agent");

/** 还没结束的后台命令（子代理之外的那些） */
export const aliveBgCommands = (messages: PortalMessage[]): BgTask[] =>
  aliveBgTasks(messages).filter((t) => t.kind !== "agent");

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
