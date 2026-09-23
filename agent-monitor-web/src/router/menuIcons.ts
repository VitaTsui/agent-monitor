/**
 * 后管菜单图标：**语义 key → 图标名**。
 *
 * hub 的 `GET /sys/menu/getMenuATopATopMenu` 只下发语义 key（`user` / `version`，
 * 见 `agent-task-monitor/hub/src/admin.rs` 的 `menus`），具体长什么样由这里决定。
 *
 * 为什么必须放在源码里：图标子集是构建期**扫源码**生成的（`scripts/genIconCollections.cjs`），
 * 扫不到的名字没有注册，组件库的离线渲染器画不出来 —— 就是一个空位。图标名从接口下发时扫描器天然看不见，
 * 从前只能靠一张手工登记表补，而那张表一旦忘了同步就静默漂。
 * 名字回到这里之后，加图标 = 改源码 = 生成器自动收进子集，漂不了。
 */
export const MENU_ICONS: Record<string, string> = {
  user: "ph:users-three",
  version: "ph:arrow-fat-lines-up",
};

/**
 * 映射查不到时的兜底图标。
 *
 * 宁可画一个「有个菜单项在这儿」的通用图标，也不要什么都不画 —— 后者是
 * 「空白且不报错」，正是这次要根除的那类问题。会走到这里的只有版本错配：
 * 前端新、hub 旧（还在下发 `carbon:*` 这类老图标名），或者 hub 加了新菜单
 * 而这张表没跟上。两种情况菜单都照常可点，只是图标是通用的。
 */
export const MENU_ICON_FALLBACK = "ph:dot-outline";

/** 取菜单图标名：认 key，查不到给兜底。**只此一条判断**，不再兼认图标名。 */
export const menuIconName = (key?: string): string =>
  (key && MENU_ICONS[key]) || MENU_ICON_FALLBACK;
