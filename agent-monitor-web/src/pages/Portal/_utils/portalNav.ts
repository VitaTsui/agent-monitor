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

export const historyPath = (taskId: string): string =>
  `${PORTAL_BASE}/history/${encodeURIComponent(taskId)}`;

/**
 * 当前地址按「返回」应该去哪一层。返回 null = 已在最外层，返回键不该被消费。
 *
 * 前台只有「会话页」与「它的子页面（远程往来）」两层，所以除了会话页本身，
 * 任何地址的上一层都是会话页。
 */
export const portalBackTarget = (pathname: string): string | null =>
  pathname.replace(/\/+$/, "") === PORTAL_BASE ? null : PORTAL_BASE;
