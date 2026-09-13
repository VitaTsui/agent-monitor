import React, { useEffect, useMemo, useState, useCallback } from "react";

import RouterStore from "./RouterService";
import { observer } from "mobx-react-lite";
import { useRoutes } from "react-router";
import { AliveScope } from "react-activation";
import { ReloadContent } from "@hsu-react/ui/es/layout";
import { PermissionsContent } from "@hsu-react/ui";
import { NavTabBarContent } from "@hsu-react/ui/es/layout";
import { NavTabBarTitleContent } from "@hsu-react/ui/es/layout";
import { getAccessToken } from "@/utils/auth";
import { ConfigProvider as HsuConfigProvider } from "@hsu-react/ui";
import { get, post, del, put } from "@/services/Axios";
import HsuLayout from "@hsu-react/ui/es/layout";
import { primaryForegroundOf, primaryOf } from "@/styles/primary";

const Routes: React.FC = observer(() => {
  const { router, permissions } = RouterStore;
  const { headerTheme } = HsuLayout.ThemeStore;
  const [id, setId] = useState<string>("");
  const [dropKey, setDropKey] = useState<string>("");
  const [tabTitles, setTabTitles] = useState<Record<string, React.ReactNode>>(
    {}
  );

  useEffect(() => {
    const path = window.location.pathname;
    const noAuthPaths = ["/", "/login", "/portal"];
    if (!getAccessToken() && !noAuthPaths.includes(path)) {
      window.location.href = "/login";
    }
  }, []);

  useEffect(() => {
    document.title = Config.title || document.title;
  }, []);

  const value = useMemo(() => {
    return { id, setId };
  }, [id, setId]);

  const permissionsValue = useMemo(() => {
    return { permissions };
  }, [permissions]);

  const dropTabValue = useMemo(() => {
    return { dropKey, setDropKey };
  }, [dropKey, setDropKey]);

  const setTabTitle = useCallback((key: string, title: React.ReactNode) => {
    setTabTitles((prev) => ({
      ...prev,
      [key]: title,
    }));
  }, []);

  const tabTitleValue = useMemo(() => {
    return { tabTitles, setTabTitle };
  }, [tabTitles, setTabTitle]);

  return (
    // 注入 @hsu-react/ui 的权限与请求实现，供库内组件（Button hasPermi、ImportForm 等）使用
    <HsuConfigProvider
      permissions={permissions}
      request={{ get, post, del, put }}
      primaryColor={primaryOf(headerTheme !== "light")}
      theme={{
        token: {
          // antd 的链接色不跟随 colorPrimary。主色换成单色墨黑后，若不一并设置，
          // 表格里的「修改」「重置密码」这类 link 按钮会留在默认蓝上，
          // 整站只剩它们是彩色。
          colorLink: primaryOf(headerTheme !== "light"),
          // 实心主色控件（Button type=primary 等）上的文字色。antd 把它固定成白，
          // 那是「主色一定比白暗」的隐含前提 —— 而 shadcn 暗色主题把主色翻成
          // zinc-50，白字压在近白底上等于看不见（实测「保存」按钮对比度 1.02）。
          // 主色是从令牌取的，它的前景也必须从令牌取。
          colorTextLightSolid: primaryForegroundOf(headerTheme !== "light"),
          // 危险色的 ramp 与染底交给本项目令牌。
          //
          // 为什么不在这里接 `colorError` 本身：它是 antd 的 **seed**，会被
          // @ant-design/colors 拿去推一整条 10 级色板（antd/es/theme/themes/shared/
          // genColorMapToken.js:14），只能吃字面量 —— 喂 `var(--destructive)` 推出来
          // 的是一条黑。而且它已经由 @hsu-react/ui 的 ConfigProvider 从同一份
          // tokens.json 接好了（`colorError: t.error` = `--vita-error` = `--destructive`），
          // 这里再写一遍就成了两个源。
          //
          // 下面这四支都**不是** seed，antd 在 formatToken 的最后一步原样采信
          // （antd/es/theme/util/alias.js:17 把 seed 名从 override 里删掉，其余原样合并），
          // 所以可以走 CSS 变量、明暗由变量自己切，不用在 JS 里写两套色值。
          //
          // 非接不可的是 hover/active：antd 推的 hover 是**更浅**的一级
          // （#dc2626 → #e8524d），落到 `type="text" danger` 这种「文字本身是红的」
          // 按钮上，悬停等于把字调淡 —— 浅色实测 3.34:1，正文要 4.5:1。
          colorErrorHover: "var(--destructive-hover)",
          colorErrorActive: "var(--destructive-active)",
          colorErrorBg: "var(--destructive-subtle)",
          colorErrorText: "var(--destructive-subtle-foreground)",
        },
      }}
    >
      <ReloadContent.Provider value={value}>
        <NavTabBarContent.Provider value={dropTabValue}>
          <NavTabBarTitleContent.Provider value={tabTitleValue}>
            <PermissionsContent.Provider value={permissionsValue}>
              <AliveScope>{useRoutes(router)}</AliveScope>
            </PermissionsContent.Provider>
          </NavTabBarTitleContent.Provider>
        </NavTabBarContent.Provider>
      </ReloadContent.Provider>
    </HsuConfigProvider>
  );
});

export default Routes;
