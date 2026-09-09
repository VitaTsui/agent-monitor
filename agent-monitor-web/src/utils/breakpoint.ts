import { breakpoints } from "@hsu-react/ui/es/styles/tokens";

/**
 * 「窄屏（手机）」的媒体查询字符串。
 *
 * 断点值取自组件库的设计令牌，与 scss 里的 `@include r.down(md)` **同源**
 * （两者都由组件库 tokens.json 生成）。JS 与 CSS 各写一个数字时，中间那一段宽度
 * 会出现「CSS 已经切成移动布局、JS 还以为是桌面」的错位 —— 本项目此前 CSS 与 JS
 * 各自硬编码 760px，属于同一份定义抄了两遍。
 *
 * 减 0.02 是组件库 `down()` mixin 的写法：max-width 取断点前一个可表示的值，
 * 保证 `down(md)` 与 `up(md)` 不重叠。
 *
 * 只在**结构**要变（渲染抽屉还是侧栏、回车提交还是换行）时才用它；
 * 纯样式差异一律交给 CSS 的 `@include r.down(md)`，不必让 JS 参与。
 */
export const MOBILE_QUERY = `(max-width: ${breakpoints.md - 0.02}px)`;

/** 当前视口是否是窄屏。用于事件回调里的一次性判断。 */
export const isMobileViewport = (): boolean =>
  window.matchMedia(MOBILE_QUERY).matches;
