import { useEffect, useState } from "react";

import { MOBILE_QUERY, isMobileViewport } from "@/utils/breakpoint";

/**
 * 当前视口是不是窄屏（手机），随窗口变化。
 *
 * 壳、会话网格、对话格三处都要按这一条改**结构**（渲染抽屉还是侧栏、
 * 状态卡放右栏还是放对话流末尾），原先各自写了一遍同样的 matchMedia 订阅。
 * 纯样式差异仍旧交给 CSS 的 `@include r.down(md)`，不必让 JS 参与。
 */
export function useIsMobile(): boolean {
  const [isMobile, setIsMobile] = useState(isMobileViewport);

  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const sync = () => setIsMobile(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  return isMobile;
}
