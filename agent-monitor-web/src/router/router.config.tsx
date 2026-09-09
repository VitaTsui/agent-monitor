import AdminGate from "@/components/AdminGate";
import App from "@/App";
import Home from "@/pages/Home";
import Login from "@/pages/Login";
import Portal from "@/pages/Portal";
import PortalSettingsIndex from "./_components/PortalSettingsIndex";
import PortalSuspense from "./_components/PortalSuspense";
import { ReactNode, lazy } from "react";
import { Navigate, RouteObject } from "react-router-dom";

// 详情/子页面（带路由参数，不在后端菜单里）在此显式注册为后管子路由。
// 示例：const FooDetail = lazy(() => import("@/pages/foo/Detail"));

/**
 * 前台 `/portal` 出口下的视图。
 *
 * 早先是 Portal 这个壳内部用 `useState` 在「会话网格 / 设置弹窗（7 个分栏）/
 * 历史弹窗」之间 if/else 切，代价是刷新丢失当前页、浏览器前进后退失效、
 * 页面发不出去（地址永远停在 `/portal`）。现在每个视图都有真实地址。
 *
 * 一律懒加载：设置的七个分栏里有配置同步、机器人接入这类首屏用不到的重块，
 * 静态引进来会跟着会话页一起进首屏。
 */
const PanesView = lazy(() => import("@/pages/Portal/_views/PanesView"));
const HistoryView = lazy(() => import("@/pages/Portal/_views/HistoryView"));
const SettingsView = lazy(() => import("@/pages/Portal/_views/SettingsView"));
const AccountView = lazy(
  () => import("@/pages/Portal/_views/SettingsView/AccountView"),
);
const AppearanceView = lazy(
  () => import("@/pages/Portal/_views/SettingsView/AppearanceView"),
);
const DevicesView = lazy(
  () => import("@/pages/Portal/_views/SettingsView/DevicesView"),
);
const ConfigsView = lazy(
  () => import("@/pages/Portal/_views/SettingsView/ConfigsView"),
);
const BotsView = lazy(() => import("@/pages/Portal/_views/SettingsView/BotsView"));
const SecurityView = lazy(
  () => import("@/pages/Portal/_views/SettingsView/SecurityView"),
);
const AboutView = lazy(() => import("@/pages/Portal/_views/SettingsView/AboutView"));

/**
 * MetaType 路由元信息
 *
 * title: 浏览器标签页后缀标题
 * name: 菜单名称
 * menu: 是否显示在菜单中，动态路由始终不显示在菜单中
 * icon: 菜单图标
 * activeIcon: 菜单激活图标
 * disabled: 是否禁用
 * affix: 是否固定在标签页中
 * noTagsView: 是否不显示在标签页中，如果为true，则不显示在标签页中，但会显示在菜单中
 * noCache: 是否不缓存，默认 true，当有 children 时，固定不缓存
 * noLazy: 是否不是懒加载，默认 false
 * noAuth: 是否不进行权限校验，默认 false
 * hasPermi: 是否有权限限制
 */
export type MetaType = {
  title?: string;
  name?: string;
  menu?: boolean;
  icon?: ReactNode;
  activeIcon?: ReactNode;
  disabled?: boolean;
  affix?: boolean;
  noTabsView?: boolean;
  noCache?: boolean;
  noLazy?: boolean;
  noAuth?: boolean;
  hasPermi?: string[];
};

export type RouteType = {
  children?: RouteType[];
  meta?: MetaType;
} & RouteObject;

// 后管（后台管理）统一路由前缀
export const ADMIN_BASE = "/admin";
// 后管默认落地页（登录后管理端、关闭全部标签时的兜底）——后管仅保留用户管理
export const ADMIN_HOME = `${ADMIN_BASE}/permit/user`;

/**
 * 拼接后管页面的绝对路径，保证所有跳转都跟随 ADMIN_BASE 变化。
 * 任何指向后管页面的硬编码跳转都应改用此函数，而非手写 "/admin/xxx"。
 * @example adminPath("permit/user/index")   // "/admin/permit/user/index"
 * @example adminPath(`syslog/detail/${id}`) // "/admin/syslog/detail/1"
 * @example adminPath()                      // "/admin"
 */
export const adminPath = (sub = ""): string => {
  const clean = sub.replace(/^\/+/, "");
  return clean ? `${ADMIN_BASE}/${clean}` : ADMIN_BASE;
};

const Router: RouteType[] = [
  {
    // 后管布局：动态菜单（来自后端）会作为本路由的 children 挂载，路径统一带 /admin 前缀
    // AdminGate：部署令牌锁，未解锁不渲染后管
    path: ADMIN_BASE,
    element: (
      <AdminGate>
        <App />
      </AdminGate>
    ),
    meta: {
      noTabsView: true,
    },
    children: [
      {
        index: true,
        element: <Navigate to={ADMIN_HOME} replace />,
        // 仅作登录后/兜底重定向用，不应作为标签页（否则裸 /admin 会留下一个空白标签）
        meta: { noTabsView: true },
      },
      // 在此显式注册带路由参数、不在后端菜单中的详情/子页面，例如：
      // {
      //   path: adminPath("foo/detail/:id"),
      //   element: <FooDetail />,
      //   meta: { name: "详情", title: "详情", noCache: true },
      // },
    ] as RouteType[],
  },
  {
    path: "/login",
    element: <Login />,
    meta: {
      title: "登录",
      noAuth: true,
    },
  },
  {
    // 前台：终端任务监控对话页（页面内自校验登录态，未登录跳 /login）。
    // Portal 是壳（侧栏 ＋ 顶栏 ＋ 出口），下面每个视图各有真实地址。
    path: "/portal",
    element: <Portal />,
    meta: {
      title: "任务监控",
      noAuth: true,
      noTabsView: true,
    },
    children: [
      {
        index: true,
        element: (
          <PortalSuspense>
            <PanesView />
          </PortalSuspense>
        ),
        meta: { title: "任务监控", noAuth: true, noTabsView: true },
      },
      {
        /* 远程往来。原来是 720 宽的弹窗 —— 一条会话的往来动辄几十屏，
           弹窗里读不了，刷新就没了，链接也发不出去 */
        path: "history/:taskId",
        element: (
          <PortalSuspense>
            <HistoryView />
          </PortalSuspense>
        ),
        meta: { title: "远程往来", noAuth: true, noTabsView: true },
      },
      {
        // 设置壳：左侧分类导航 ＋ 二级出口。七个分栏各一条子路由
        path: "settings",
        element: (
          <PortalSuspense>
            <SettingsView />
          </PortalSuspense>
        ),
        meta: { title: "设置", noAuth: true, noTabsView: true },
        children: [
          {
            /* 桌面进 `/portal/settings` 落到「账户」；移动端是 iOS 设置式两级，
               这一层只显示菜单列表，所以**不能**在这里无条件重定向 ——
               那样一进设置就直接冲进二级页，一级菜单永远看不到。
               条件由 SettingsIndex 这个薄壳按视口判断（见 _components）。 */
            index: true,
            element: <PortalSettingsIndex />,
            meta: { noAuth: true, noTabsView: true },
          },
          {
            path: "account",
            element: (
              <PortalSuspense>
                <AccountView />
              </PortalSuspense>
            ),
            meta: { title: "账户", noAuth: true, noTabsView: true },
          },
          {
            path: "appearance",
            element: (
              <PortalSuspense>
                <AppearanceView />
              </PortalSuspense>
            ),
            meta: { title: "外观", noAuth: true, noTabsView: true },
          },
          {
            path: "devices",
            element: (
              <PortalSuspense>
                <DevicesView />
              </PortalSuspense>
            ),
            meta: { title: "设备管理", noAuth: true, noTabsView: true },
          },
          {
            path: "configs",
            element: (
              <PortalSuspense>
                <ConfigsView />
              </PortalSuspense>
            ),
            meta: { title: "配置同步", noAuth: true, noTabsView: true },
          },
          {
            path: "bots",
            element: (
              <PortalSuspense>
                <BotsView />
              </PortalSuspense>
            ),
            meta: { title: "机器人管理", noAuth: true, noTabsView: true },
          },
          {
            path: "security",
            element: (
              <PortalSuspense>
                <SecurityView />
              </PortalSuspense>
            ),
            meta: { title: "安全防护", noAuth: true, noTabsView: true },
          },
          {
            path: "about",
            element: (
              <PortalSuspense>
                <AboutView />
              </PortalSuspense>
            ),
            meta: { title: "关于", noAuth: true, noTabsView: true },
          },
        ],
      },
    ],
  },
  {
    // 根路径：产品官网首页（公开，无需登录）
    path: "/",
    element: <Home />,
    meta: {
      title: "终端任务监控",
      noAuth: true,
      noTabsView: true,
    },
  },
];

export default Router;
