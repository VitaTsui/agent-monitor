import AdminGate from "@/components/AdminGate";
import App from "@/App";
import Login from "@/pages/Login";
import Portal from "@/pages/Portal";
import { ReactNode } from "react";
import { Navigate, RouteObject } from "react-router-dom";

// 详情/子页面（带路由参数，不在后端菜单里）在此显式注册为后管子路由。
// 示例：const FooDetail = lazy(() => import("@/pages/foo/Detail"));

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
    // 前台：终端任务监控对话页（页面内自校验登录态，未登录跳 /login）
    path: "/portal",
    element: <Portal />,
    meta: {
      title: "任务监控",
      noAuth: true,
      noTabsView: true,
    },
  },
  {
    // 根路径默认进入前台（后管仅管理员经令牌解锁后使用）
    path: "/",
    element: <Navigate to="/portal" replace />,
    meta: {
      noAuth: true,
      noTabsView: true,
    },
  },
];

export default Router;
