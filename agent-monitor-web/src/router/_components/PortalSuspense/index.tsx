import React, { Suspense } from "react";

import PageLoading from "../PageLoading";

/**
 * 前台视图的懒加载边界。
 *
 * 前台这几条静态路由**不经过** `RouterContainer`（那是后管动态菜单那条链上的
 * 包装器，见 RouterService.wrapRoutes 只包了 `/admin` 的 children），所以
 * `Suspense` 必须在这里自己给 —— 少了它，`lazy()` 的第一次挂起会一路冒到根，
 * 整棵树被丢掉（React 18 在同步更新里遇到挂起就是这个后果，表现为整页空白）。
 */
const PortalSuspense: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <Suspense fallback={<PageLoading />}>{children}</Suspense>
);

export default PortalSuspense;
