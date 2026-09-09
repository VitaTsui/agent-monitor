/**
 * 前台路由的地址常量与「上一层」推导。
 *
 * 拆成一处的理由：返回这件事有多个触发点 —— 页面里的返回按钮、浏览器后退、
 * Android 原生返回键。它们必须落到同一个目标，否则会出现「返回一次跳两层」
 * 或「返回不回去」。
 *
 * 设置**不在这里**：它是弹窗不是地址（见 _components/SettingsModal），
 * 关它/退它一层由壳里的 state 判断，不经过路由。
 */

export const PORTAL_BASE = "/portal";

/** 设置分栏的键。 */
export type SettingsTab =
  | "account"
  | "appearance"
  | "devices"
  | "configs"
  | "bots"
  | "security"
  | "about";

/** 全部会话（远程往来列表页）。侧栏那条「查看全部会话」的落点。 */
export const HISTORY_LIST_PATH = `${PORTAL_BASE}/history`;

export const historyPath = (taskId: string): string =>
  `${PORTAL_BASE}/history/${encodeURIComponent(taskId)}`;

/**
 * 当前地址按「返回」应该去哪一层。返回 null = 已在最外层，返回键不该被消费。
 *
 * 前台是三层：会话页 → 全部会话 → 某条会话的远程往来。逐层往上走，
 * 页面里的返回按钮与浏览器 / Android 原生返回键必须落到同一个目标 ——
 * 否则会出现「按钮回上一层、返回键直接回首页」这种两套行为并存。
 */
export const portalBackTarget = (pathname: string): string | null => {
  const path = pathname.replace(/\/+$/, "");
  if (path === PORTAL_BASE) return null;
  // 远程往来详情（history/:taskId）的上一层是全部会话
  if (path.startsWith(`${HISTORY_LIST_PATH}/`)) return HISTORY_LIST_PATH;
  return PORTAL_BASE;
};
