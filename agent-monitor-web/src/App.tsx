import "./App.scss";

import { Layout } from "antd";
import { Outlet, useNavigate } from "react-router-dom";
import React, { useEffect, useState } from "react";
import { getUserInfo } from "./utils/auth";

// 布局来自组件库。这几个组件此前是本项目的 src/layout/，2.0 起已收进 @hsu-react/ui，
// 本项目不再自己维护一份。走子路径引入：它们依赖 react-router / react-intl，而这两个
// 在组件库里是**可选** peerDependency，所以刻意没从包根导出。
import HsuLayout from "@hsu-react/ui/es/layout";
import type { AccountAction, MenuType } from "@hsu-react/ui/es/layout";
import PwdChange from "./pages/PwdChange";
import RouterService from "./router/RouterService";
import { ADMIN_HOME } from "./router/router.config";
import { clearAllCookie } from "./services/Axios";
import { observer } from "mobx-react-lite";
import wsCache from "./utils/wsCache";
import LoginStore from "./pages/Login/LoginStore";
import { usePermissions } from "@hsu-react/ui";

const { Sider, Content } = Layout;

const App: React.FC = observer(() => {
  const { logout } = LoginStore;
  const { router } = RouterService;

  const { layout, headerTheme } = HsuLayout.ThemeStore;

  // 导航(顶栏/侧边)明暗：亮色 -> light，暗色/主题色 -> dark
  const navTheme: "light" | "dark" =
    headerTheme === "light" ? "light" : "dark";

  const { nickname } = getUserInfo();
  const navigate = useNavigate();
  const [collapsed, setCollapsed] = useState(false);
  const [pwdOpen, setPwdOpen] = useState(false);
  const [childrenItems, setChildrenItems] = useState<MenuType[]>([]);
  const { checkPermission } = usePermissions();

  useEffect(() => {
    // 窗口宽度 <=1440 收起菜单、>1440 展开；只在跨越断点时自动切换，
    // 断点内的 resize 不覆盖用户手动展开/收起的选择
    const mql = window.matchMedia("(max-width: 1440px)");
    const onBreakpointChange = (e: MediaQueryListEvent | MediaQueryList) => {
      setCollapsed(e.matches);
    };

    onBreakpointChange(mql);
    mql.addEventListener("change", onBreakpointChange);

    return () => {
      mql.removeEventListener("change", onBreakpointChange);
    };
  }, []);

  const quit = () => {
    wsCache.clear();
    clearAllCookie();
    navigate(`/login`);
  };

  const menu = ([
    {
      title: "修改密码",
      icon: "fa-regular:edit",
      onclick: () => setPwdOpen(true),
      hasPermi: ["sys:user:updPwd"],
    },
    {
      title: "退出登录",
      icon: "ep:switch-button",
      onclick: () => logout(quit),
    },
  ] as (AccountAction & { hasPermi?: string[] })[]).filter((item) =>
    checkPermission(item.hasPermi)
  );

  // 外观控制器 HsuLayout.Theme 已上移到应用根（src/index.tsx）：写 html[data-theme]
  // 是文档级的事，包在后管壳里会让前台 /portal 拿不到暗色令牌。
  return (
    <>
      <Layout id="App" className={headerTheme}>
        {/* 站点标题与用户信息原本由本项目的 Header 自己去读全局 Config 与 @/utils/auth，
            组件收进库之后不再认识这两样，改由这里注入。
            外观 / 语言 / 账号操作那套下拉也一并由 Header 提供，不用再自绘 */}
        <HsuLayout.Header
          router={router}
          collapsed={collapsed}
          onToggleCollapsed={() => setCollapsed(!collapsed)}
          onChildItems={setChildrenItems}
          menu={menu}
          user={{ nickname }}
          title={Config.title}
          smallTitle={Config.smallTitle}
        />
        <Layout className="body">
          {/* 左侧菜单 */}
          {["left", "mixed"].includes(layout) && (
            <Sider
              trigger={null}
              collapsible
              collapsed={collapsed}
              width={230}
              theme={navTheme}
            >
              <HsuLayout.Menu
                router={router}
                collapsed={collapsed}
                theme={navTheme}
                menuItems={layout === "mixed" ? childrenItems : undefined}
              />
            </Sider>
          )}

          <Layout className="content">
            {/* 内容标签栏 */}
            <HsuLayout.NavTabBar
              router={router}
              affixRouter={[ADMIN_HOME]}
              basePath={ADMIN_HOME}
            />

            {/* 内容区域 */}
            <Content className="content-body">
              <Outlet />
            </Content>
          </Layout>
        </Layout>
      </Layout>

      <PwdChange
        open={pwdOpen}
        onCancel={() => setPwdOpen(false)}
        onOk={() => logout(quit)}
      />
    </>
  );
});

export default App;
