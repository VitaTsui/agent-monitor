import { useEffect, useState } from "react";

/**
 * 系统的「减少动态效果」开关。CSS 侧各组件自己写
 * `@media (prefers-reduced-motion: reduce)`（口径见 `StatusIcon` / `TerminalFeed`
 * 里那两段注释）；**JS 驱动的动效 CSS 管不到**，得读同一支查询自己判断 ——
 * 这个文件就是 JS 侧的那一份，别再各处抄一遍 matchMedia 字符串。
 */
const REDUCE_QUERY = "(prefers-reduced-motion: reduce)";

/**
 * 一次性读取（事件回调里用）。
 *
 * 回调里不能用 hook，而 `scrollTo({ behavior: "smooth" })` 这类调用恰恰都发生在
 * 回调里 —— 调用当下现读一次即可，用户改系统设置后下一次点击就生效，
 * 不需要订阅。
 */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    !!window.matchMedia?.(REDUCE_QUERY).matches
  );
}

/**
 * 渲染里用（开关变了要重渲染的场合，例如 `ScrollText` 要在「滚动」与
 * 「省略号截断」两种 DOM 之间切）。
 */
export default function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(prefersReducedMotion);
  useEffect(() => {
    const mq = window.matchMedia?.(REDUCE_QUERY);
    if (!mq) {
      return;
    }
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}
