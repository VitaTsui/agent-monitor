import React, { useEffect, useMemo, useState, useCallback } from "react";

import RouterStore from "./RouterService";
import { observer } from "mobx-react-lite";
import { useRoutes } from "react-router";
import { AliveScope } from "react-activation";
import { ReloadContent } from "@/hooks/useReload";
import { PermissionsContent } from "@/hooks/usePermissions";
import { NavTabBarContent } from "@/hooks/useDropTab";
import { NavTabBarTitleContent } from "@/hooks/useSetTabTitle";
import { getAccessToken } from "@/utils/auth";
import { ConfigProvider as HsuConfigProvider } from "@hsu-react/ui";
import { get, post, del, put } from "@/services/Axios";

const Routes: React.FC = observer(() => {
  const { router, permissions } = RouterStore;
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
    <HsuConfigProvider permissions={permissions} request={{ get, post, del, put }}>
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
