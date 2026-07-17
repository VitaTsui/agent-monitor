import "./App.scss";

import { Avatar, Button, Layout, Popover, Segmented, Space } from "antd";
import {
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { Outlet, useNavigate } from "react-router-dom";
import React, { useEffect, useState } from "react";
import { getUserInfo } from "./utils/auth";

import Breadcrumb from "./layout/Breadcrumb";
import { Icon } from "@hsu-react/ui";
import Menu, { MenuType } from "./layout/Menu";
import PwdChange from "./pages/PwdChange";
import RouterService from "./router/RouterService";
import { ADMIN_HOME } from "./router/router.config";
import { clearAllCookie } from "./services/Axios";
import { observer } from "mobx-react-lite";
import wsCache from "./utils/wsCache";
import NavTabBar from "./layout/NavTabBar";
import LoginStore from "./pages/Login/LoginStore";
import ThemeStore from "./layout/Theme/ThemeStore";
import classNames from "classnames";
import Theme from "./layout/Theme";
import I18nStore from "./layout/I18n/I18nStore";
import usePermissions from "@/hooks/usePermissions";

const { Header, Sider, Content } = Layout;

const App: React.FC = observer(() => {
  const { logout } = LoginStore;
  const { router } = RouterService;

  const { layout, headerTheme, appearance, setAppearance } = ThemeStore;
  const { locale, setLocale } = I18nStore;

  // CF 做法：外观 + 语言放进用户下拉，跟随当前语言显示双语标签
  const isEn = locale === "en-US";
  const appearanceOptions = [
    { label: isEn ? "Light" : "浅色", value: "light" },
    { label: isEn ? "Dark" : "深色", value: "dark" },
    { label: isEn ? "System" : "跟随", value: "system" },
  ];
  const languageOptions = [
    { label: "中文", value: "zh-CN" },
    { label: "English", value: "en-US" },
  ];

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

  const menu = [
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
  ].filter((item) => checkPermission(item.hasPermi));

  return (
    <Theme>
      <Layout id="App" className={headerTheme}>
        <Header className={classNames("header", headerTheme)}>
          <div className="header-left">
            {/* 标题 */}
            {["left", "mixed"].includes(layout) ? (
              <div
                className={classNames("title", { titleCollapsed: collapsed })}
              >
                {collapsed ? Config.smallTitle : Config.title}
              </div>
            ) : (
              <div className={classNames("title", "titleTop")}>
                {Config.title}
              </div>
            )}

            {/* 折叠按钮 */}
            {["left", "mixed"].includes(layout) && (
              <Button
                className="collapsed"
                type="text"
                icon={collapsed ? <MenuUnfoldOutlined /> : <MenuFoldOutlined />}
                onClick={() => setCollapsed(!collapsed)}
                style={{
                  fontSize: "16px",
                  width: 64,
                  height: 64,
                }}
              />
            )}

            {/* 面包屑 */}
            {["left"].includes(layout) && (
              <Breadcrumb router={router} className={"breadcrumb"} />
            )}

            {/* 顶部菜单 */}
            {["top", "mixed"].includes(layout) && (
              <Menu
                router={router}
                mode="horizontal"
                theme={navTheme}
                onlyLvOneMenu={layout === "mixed"}
                getCurrChildItems={setChildrenItems}
              />
            )}
          </div>
          <div className="header-right">
            {/* 用户信息（外观 + 语言 + 账号操作，CF 做法集中在此下拉） */}
            <Popover
              overlayClassName="userPopover"
              placement="bottomRight"
              content={
                <div className="userMenuPanel">
                  <div className="settingRow">
                    <span className="settingLabel">
                      {isEn ? "Appearance" : "外观"}
                    </span>
                    <Segmented
                      size="small"
                      value={appearance}
                      options={appearanceOptions}
                      onChange={(v) => setAppearance(v as typeof appearance)}
                    />
                  </div>
                  <div className="settingRow">
                    <span className="settingLabel">
                      {isEn ? "Language" : "语言"}
                    </span>
                    <Segmented
                      size="small"
                      value={locale}
                      options={languageOptions}
                      onChange={(v) => setLocale(v as string)}
                    />
                  </div>

                  <div className="settingDivider" />

                  <div className="menu">
                    {menu?.map((item, index) => (
                      <Button
                        key={index}
                        icon={<Icon icon={item.icon} />}
                        onClick={item.onclick}
                        type="text"
                      >
                        {item.title}
                      </Button>
                    ))}
                  </div>
                </div>
              }
            >
              <Space className="user">
                <Avatar
                  style={{ backgroundColor: "#1677ff", verticalAlign: "middle" }}
                  icon={nickname ? undefined : <UserOutlined />}
                >
                  {nickname?.[0]?.toUpperCase()}
                </Avatar>
                {nickname}
              </Space>
            </Popover>
          </div>
        </Header>
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
              <Menu
                router={router}
                collapsed={collapsed}
                theme={navTheme}
                menuItems={layout === "mixed" ? childrenItems : undefined}
              />
            </Sider>
          )}

          <Layout className="content">
            {/* 内容标签栏 */}
            <NavTabBar
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
    </Theme>
  );
});

export default App;
