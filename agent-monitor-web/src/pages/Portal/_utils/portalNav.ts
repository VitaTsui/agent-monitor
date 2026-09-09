/**
 * 前台路由的地址常量与「上一层」推导。
 *
 * 拆成一处的理由：返回这件事有四个触发点 —— 页面里的返回按钮、浏览器后退、
 * Android 原生返回键、移动端 sheet 下滑。它们必须落到同一个目标，否则会出现
 * 「返回一次跳两层」或「返回不回去」。
 */

export const PORTAL_BASE = "/portal";
export const PORTAL_SETTINGS = `${PORTAL_BASE}/settings`;

/** 设置分栏的键。与路由末段一一对应（`/portal/settings/<key>`）。 */
export type SettingsTab =
  | "account"
  | "appearance"
  | "devices"
  | "configs"
  | "bots"
  | "security"
  | "about";

export const SETTINGS_TABS: readonly SettingsTab[] = [
  "account",
  "appearance",
  "devices",
  "configs",
  "bots",
  "security",
  "about",
];

export const settingsPath = (tab: SettingsTab): string =>
  `${PORTAL_SETTINGS}/${tab}`;

export const historyPath = (taskId: string): string =>
  `${PORTAL_BASE}/history/${encodeURIComponent(taskId)}`;

/**
 * 当前地址按「返回」应该去哪一层。返回 null = 已在最外层，返回键不该被消费。
 *
 * 移动端设置是两级（列表 → 分区），桌面是一屏（左右分栏），所以
 * `/portal/settings/<tab>` 的上一层在两端不同 —— 这正是必须集中判断的地方。
 */
export const portalBackTarget = (
  pathname: string,
  isMobile: boolean,
): string | null => {
  const path = pathname.replace(/\/+$/, "") || PORTAL_BASE;

  if (path === PORTAL_BASE) {
    return null;
  }

  if (path === PORTAL_SETTINGS) {
    return PORTAL_BASE;
  }

  if (path.startsWith(`${PORTAL_SETTINGS}/`)) {
    return isMobile ? PORTAL_SETTINGS : PORTAL_BASE;
  }

  // 历史等其余子页面：回会话页
  return PORTAL_BASE;
};
