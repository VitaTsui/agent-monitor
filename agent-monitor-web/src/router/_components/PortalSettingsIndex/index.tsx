import React, { useEffect, useState } from "react";

import { Navigate } from "react-router-dom";

import { MOBILE_QUERY, isMobileViewport } from "@/utils/breakpoint";
import { settingsPath } from "@/pages/Portal/_utils/portalNav";

/**
 * `/portal/settings` 的落地行为，按视口分两态：
 *
 * - 桌面：左右分栏，右边不能空着 → 重定向到「账户」。
 * - 移动端：iOS 设置式两级，这一层**就是**一级菜单列表（由设置壳渲染），
 *   所以什么都不渲染，出口留空即可。无条件重定向会让一级菜单永远看不到。
 *
 * 判断放在路由层而不是设置壳里：它决定的是「这条地址等不等价于另一条地址」，
 * 属于路由的事；壳里再判一次就成了两处各写一遍的同一个条件。
 */
const PortalSettingsIndex: React.FC = () => {
  const [isMobile, setIsMobile] = useState(isMobileViewport);

  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const sync = () => setIsMobile(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  return isMobile ? null : <Navigate to={settingsPath("account")} replace />;
};

export default PortalSettingsIndex;
