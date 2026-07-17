import { MenuList, getMenuList, getPermissions } from "@/services/apis/permit";
import { autorun, makeAutoObservable } from "mobx";
import { cloneDeep, debounce } from "lodash";
import wsCache, { CACHE_KEY } from "@/utils/wsCache";

import { Icon, Panel } from "@hsu-react/ui";
import NoFoundPage from "@/404";
import { Outlet } from "react-router";
import { ADMIN_BASE, RouteType } from "./router.config";
import Router from "./router.config";
import RouterContainer from "./_components/RouterContainer";
import { getAccessToken } from "@/utils/auth";
import { lazy } from "react";
import { array_is_includes } from "hsu-utils";

// 菜单路径映射：旧路径 -> 新路径
const MENU_PATH_MAPPING: Readonly<Record<string, string>> = {
  "sys/dept/index": "permit/dept/index",
  "sys/menu/index": "permit/menu/index",
  "sys/role/index": "permit/role/index",
  "sys/user/index": "permit/user/index",
  "sys/log/log": "syslog/ApiLog/index",
  "sys/logError/logError": "syslog/ErrorLog/index",
  "sys/logLogin/logLogin": "syslog/LoginLog/index",
  "sys/oprLog/oprLog": "syslog/OprLog/index",
  "sys/logChange/logChange": "syslog/LogChange/index",
  "sys/dict/index": "sysmgmt/Dict/index",
  "sys/param/param": "sysmgmt/Param/index",
  "sys/fileConf/index": "systool/File/index",
};

// 路径前缀映射：旧前缀 -> 新前缀
const PATH_PREFIX_MAPPING: Readonly<Record<string, string>> = {
  "/sys": "/platform",
  "/log": "/syslog",
};

/**
 * 递归包装路由，为每个路由添加 RouterContainer 容器组件
 */
function wrapRoutes(routes: RouteType[]): RouteType[] {
  return routes?.map((route) => {
    const newRoute = { ...route };

    newRoute.element = <RouterContainer {...route} />;

    if (route.children?.length) {
      newRoute.children = wrapRoutes(route.children);
    }

    return newRoute;
  });
}

/**
 * 使用 webpack 的 require.context 动态导入所有页面组件
 * 这是 webpack 特有的功能，用于批量导入模块
 */
const pages = require.context("../pages/", true, /\.tsx$/);

/**
 * 将所有页面组件转换为懒加载组件映射表
 * @param r - webpack require.context 对象
 * @returns 组件路径到懒加载组件的映射
 */
function importAll(r: __WebpackModuleApi.RequireContext) {
  const modules: Record<
    string,
    React.LazyExoticComponent<React.ComponentType>
  > = {};

  r.keys()
    .filter((key) => !key.includes("/_contComps/") && !key.includes("\\_contComps\\"))
    .forEach((key) => {
      const normalizedKey = key
        .toLowerCase()
        .replace(/\.tsx$/, "")
        .replace(/^\.\//, "");

      // lazy() 需要一个返回 Promise 的函数
      // webpack 的 require.context 返回的模块需要包装成 Promise
      modules[normalizedKey] = lazy(() => Promise.resolve(r(key)));
    });

  return modules;
}

const modules = importAll(pages);

class RouterStore {
  get router() {
    return this._router;
  }
  private _router: RouteType[] = Router;

  get permissions() {
    return this._permissions;
  }
  private _permissions: string[] | null = null;

  private _menuList: MenuList[] = [];

  // 防抖函数需要持久化，避免每次 autorun 时创建新实例
  private _debouncedGetMenuList: ReturnType<typeof debounce>;
  private _debouncedGetPermissions: ReturnType<typeof debounce>;

  constructor() {
    makeAutoObservable(this);

    // 在构造函数中初始化防抖函数
    this._debouncedGetMenuList = debounce(this.getMenuList.bind(this), 300);
    this._debouncedGetPermissions = debounce(
      this.getPermissions.bind(this),
      300
    );

    autorun(() => {
      if (getAccessToken()) {
        this._debouncedGetMenuList();
        this._debouncedGetPermissions();
      }
    });
  }

  public getPermissions = async (reload: boolean = false) => {
    if (wsCache.get(CACHE_KEY.ROLE_PERMISSION) && !reload) {
      this._permissions = wsCache.get(CACHE_KEY.ROLE_PERMISSION);
    } else {
      const res = await getPermissions();
      if (res.code === 0) {
        this._permissions = res.data.stringPermissions;
        wsCache.set(CACHE_KEY.ROLE_PERMISSION, this._permissions);
      }
    }
  };

  public getMenuList = async (reload: boolean = false) => {
    const router = cloneDeep(Router);

    if (wsCache.get(CACHE_KEY.ROLE_ROUTERS) && !reload) {
      this._menuList = wsCache.get(CACHE_KEY.ROLE_ROUTERS);
    } else {
      const res = await getMenuList();
      if (res.code === 0) {
        this._menuList = res.data.menuList;
        wsCache.set(CACHE_KEY.ROLE_ROUTERS, this._menuList);
      }
    }

    const _router = this._formatMenuLIst(this._menuList);
    const children = router[0].children
      ? router[0].children.concat(_router)
      : _router;
    router[0].children = wrapRoutes(children);

    router.push({
      path: "*",
      element: <NoFoundPage />,
    });

    this._router = router;
  };

  /**
   * 规范化路径：处理 index 路径、添加斜杠前缀、应用路径映射等
   */
  private _normalizePath = (
    path: string,
    parentPath?: string,
    applyMapping: boolean = true
  ): string => {
    let normalizedPath = path;

    // 处理 index 路径
    if (normalizedPath === "index" && parentPath) {
      normalizedPath = parentPath;
    }

    // 确保路径以 / 开头
    if (!normalizedPath.startsWith("/")) {
      normalizedPath = `/${normalizedPath}`;
    }

    // 后管页面统一挂到 /admin 前缀下（AI 问答页独占根路径）。
    // 父子层级都会在此补齐，保证 _normalizePath 的父子合并逻辑不受影响。
    if (
      normalizedPath !== ADMIN_BASE &&
      !normalizedPath.startsWith(`${ADMIN_BASE}/`)
    ) {
      normalizedPath = `${ADMIN_BASE}${normalizedPath}`;
    }

    // 应用路径前缀映射（仅在根级路由时）
    if (applyMapping && !parentPath) {
      for (const [oldPrefix, newPrefix] of Object.entries(
        PATH_PREFIX_MAPPING
      )) {
        if (normalizedPath.startsWith(oldPrefix)) {
          normalizedPath = normalizedPath.replace(oldPrefix, newPrefix);
          break;
        }
      }
    }

    // 合并父路径
    if (parentPath) {
      const parentParts = parentPath.split("/");
      const currentParts = normalizedPath.split("/");

      if (!array_is_includes(parentParts, currentParts)) {
        normalizedPath = `${parentPath}${normalizedPath}`;
      }
    }

    return normalizedPath;
  };

  /**
   * 规范化组件 URL
   */
  private _normalizeComponentUrl = (url: string): string => {
    const cleanUrl = url.startsWith("/")
      ? url.split("/").filter(Boolean).join("/")
      : url;
    return MENU_PATH_MAPPING[cleanUrl] || cleanUrl;
  };

  /**
   * 根据 URL 设置路由的 element 属性
   */
  private _setRouteElement = (_router: RouteType, url: string): void => {
    const normalizedUrl = this._normalizeComponentUrl(url);
    const lowerCaseUrl = normalizedUrl.toLowerCase();

    // 检查是否存在对应的模块
    if (modules?.[lowerCaseUrl]) {
      const Element = modules[lowerCaseUrl];

      if (_router.children) {
        _router.element = <Outlet />;
        _router.meta = {
          ..._router.meta,
          noLazy: true,
          noCache: true,
        };
        _router.children.unshift({
          index: true,
          element: <Element />,
          // 继承父级的 name/icon，否则该 index 默认页会生成一个 label 为空的标签
          meta: { name: _router.meta?.name, icon: _router.meta?.icon },
        });
      } else {
        _router.element = <Element />;
      }
      return;
    }

    // 检查是否为外部 URL 或内部路由
    const urlType = this._detectHttpUrlType(normalizedUrl);

    if (urlType === "invalid" || urlType === "localhost") {
      return;
    }

    if (urlType === "route") {
      const router = Router.find((route) => route.path === `/${normalizedUrl}`);
      if (router) {
        _router.element = <Panel.Iframe children={router.element} fullBtn />;
        _router.meta = {
          ..._router.meta,
          noLazy: true,
        };
      }
    } else {
      _router.element = <Panel.Iframe src={normalizedUrl} fullBtn />;
      _router.meta = {
        ..._router.meta,
        noLazy: true,
        noCache: true,
      };
    }
  };

  private _formatMenuLIst = (menuList: MenuList[], parentPath?: string) => {
    const router: RouteType[] = [];

    menuList?.forEach((item) => {
      let children: RouteType[] = [];

      // 使用统一的路径规范化函数
      const path = this._normalizePath(item.path, parentPath);

      const _router: RouteType = {
        path,
        meta: {
          name: item.nm,
          title: item.nm,
          menu: !item.status,
          icon: item.icon ? <Icon icon={item.icon} /> : undefined,
          hasPermi: item.perm ? [item.perm] : undefined,
        },
      };

      if (parentPath === path) {
        delete _router.path;

        (_router as { index: boolean }).index = true;
      }

      if (!_router.index && item.children) {
        children = this._formatMenuLIst(item.children, _router.path);
        _router.children = children.length > 0 ? children : undefined;
      }

      // 处理组件 URL
      if (item.url) {
        this._setRouteElement(_router, item.url);
      } else if (_router.children) {
        // 没有 URL 但有子路由，设置为 Outlet
        _router.element = <Outlet />;
        _router.meta = {
          ..._router.meta,
          noLazy: true,
          noCache: true,
        };
      }

      if (_router.element) router.push(_router);
    });

    return router;
  };

  /**
   * 判断路径是否存在于路由配置中
   * @param path - 路由路径，如 "/home" 或 "/platform/user/index"
   * @param exactMatch - 是否精确匹配，默认 true。false 时会匹配路径前缀
   * @returns 路径是否存在
   * @example
   * RouterService.hasPath("/home") // true
   * RouterService.hasPath("/nonexistent") // false
   * RouterService.hasPath("/platform", false) // true (匹配 /platform/xxx)
   */
  public hasPath = (path: string, exactMatch: boolean = true): boolean => {
    if (!path || typeof path !== "string") {
      return false;
    }

    const normalizedPath = path.trim();

    const searchInRoutes = (routes: RouteType[]): boolean => {
      for (const route of routes) {
        // 跳过没有 path 的路由（如 index 路由）
        if (!route.path) {
          if (route.children?.length) {
            if (searchInRoutes(route.children)) return true;
          }
          continue;
        }

        // 精确匹配或前缀匹配
        if (exactMatch) {
          if (route.path === normalizedPath) return true;
        } else {
          if (normalizedPath.startsWith(route.path)) return true;
        }

        // 递归搜索子路由
        if (route.children?.length) {
          if (searchInRoutes(route.children)) return true;
        }
      }
      return false;
    };

    return searchInRoutes(this._router);
  };

  /**
   * 根据组件URL路径筛选对应的路由
   * @param componentUrl - 组件文件路径，如 "permit/user/index" 或 "sys/user/index"
   * @returns 匹配的路由对象或undefined
   */
  public findRouteByComponentUrl = (
    componentUrl: string
  ): RouteType | undefined => {
    if (!componentUrl || typeof componentUrl !== "string") {
      return undefined;
    }

    // 标准化组件URL：移除开头的斜杠，转为小写
    const normalizedUrl = componentUrl.trim().toLowerCase().replace(/^\/+/, "");

    // 应用菜单路径映射
    const mappedUrl = MENU_PATH_MAPPING[normalizedUrl] || normalizedUrl;

    const findInRoutes = (routes: RouteType[]): RouteType | undefined => {
      for (const route of routes) {
        // 获取路由对应的组件URL并进行匹配
        const routeComponentUrl = this._getRouteComponentUrl(route);

        if (
          routeComponentUrl &&
          (routeComponentUrl === mappedUrl ||
            routeComponentUrl === normalizedUrl)
        ) {
          return route;
        }

        // 递归搜索子路由
        if (route.children?.length) {
          const found = findInRoutes(route.children);
          if (found) return found;
        }
      }
      return undefined;
    };

    return findInRoutes(this._router);
  };

  /**
   * 从路由对象中提取组件URL路径
   * @param route - 路由对象
   * @returns 组件URL或undefined
   */
  private _getRouteComponentUrl = (route: RouteType): string | undefined => {
    const findUrlInMenuList = (
      menuList: MenuList[],
      targetPath?: string,
      parentPath?: string
    ): string | undefined => {
      for (const item of menuList) {
        // 使用统一的路径规范化函数
        const path = this._normalizePath(item.path, parentPath);

        // 匹配目标路径
        if (path === targetPath && item.url) {
          return item.url.split("/").filter(Boolean).join("/").toLowerCase();
        }

        // 递归搜索子路由
        if (item.children) {
          const found = findUrlInMenuList(item.children, targetPath, path);
          if (found) return found;
        }
      }
      return undefined;
    };

    return findUrlInMenuList(this._menuList, route.path);
  };

  /**
   * 检测URL类型
   * @param url - 待检测的URL字符串
   * @returns URL类型：
   *  - "localhost": http(s)://localhost 或 http(s)://127.0.0.1
   *  - "ip": http(s)://IP地址
   *  - "domain": http(s)://域名
   *  - "ip-port": IP:端口格式（不带协议）
   *  - "ip-only": IP地址（不带端口和协议）
   *  - "route": 固定内部路由路径
   *  - "invalid": 无效URL
   */
  private _detectHttpUrlType = (
    url: string
  ):
    | "localhost"
    | "ip"
    | "domain"
    | "ip-port"
    | "ip-only"
    | "route"
    | "invalid" => {
    if (!url || typeof url !== "string") {
      return "invalid";
    }

    const trimmedUrl = url.trim();

    // URL 匹配规则配置
    const URL_PATTERNS = {
      localhost: /^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?(\/.*)?$/i,
      ip: /^https?:\/\/((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(:\d+)?(\/.*)?$/,
      domain:
        /^https?:\/\/([a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}(:\d+)?(\/.*)?$/,
      ipPort:
        /^((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d):\d+$/,
      ipOnly:
        /^((25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$/,
    };

    // 按优先级检测URL类型
    if (URL_PATTERNS.localhost.test(trimmedUrl)) {
      return "localhost";
    }

    if (URL_PATTERNS.ip.test(trimmedUrl)) {
      return "ip";
    }

    if (URL_PATTERNS.domain.test(trimmedUrl)) {
      return "domain";
    }

    if (URL_PATTERNS.ipPort.test(trimmedUrl)) {
      return "ip-port";
    }

    if (URL_PATTERNS.ipOnly.test(trimmedUrl)) {
      return "ip-only";
    }

    // 检测是否为固定内部路由
    if (Router.some((route) => route.path === `/${trimmedUrl}`)) {
      return "route";
    }

    return "invalid";
  };
}

const RouterService = new RouterStore();
export default RouterService;
