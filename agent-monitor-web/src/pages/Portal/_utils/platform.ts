/** 平台图标：设备列表与会话卡片头部共用，避免两处各写一份漂移 */
const PLATFORM_ICON: Record<string, string> = {
  macos: "",
  windows: "🪟",
  linux: "🐧",
};

/** 取平台图标；未知平台返回空串（调用方直接拼接即可） */
export const platformIcon = (platform?: string) =>
  PLATFORM_ICON[platform ?? ""] ?? "";
