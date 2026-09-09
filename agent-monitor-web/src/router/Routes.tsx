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

/**
 * 主色。与 styles/tokens.scss 里的 --primary 同一取值：完整照搬 shadcn 默认主题——
 * 浅色取中性阶最深的 zinc-900，暗色翻到最浅的 zinc-50。
 *
 * 必须是**字面量**，不能填 var(--primary)：antd 要由它派生一整条 10 级色板，
 * 拿到 var() 会直接算不出来。两处取值重复是有意的，改动时一起改。
 *
 * 这一层原本由本项目的 layout/Theme 承担（写死 #13C2C2）；布局收进组件库后没人接手，
 * 主色会静默回落成 antd 默认的蓝，所以在这里显式传给 ConfigProvider。
 */
const primaryColorOf = (isDark: boolean) => (isDark ? "#fafafa" : "#18181b");

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
      primaryColor={primaryColorOf(headerTheme !== "light")}
      // antd 的链接色不跟随 colorPrimary。主色换成单色墨黑后，若不一并设置，
      // 表格里的「修改」「重置密码」这类 link 按钮会留在默认蓝上，
      // 整站只剩它们是彩色。
      theme={{ token: { colorLink: primaryColorOf(headerTheme !== "light") } }}
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
