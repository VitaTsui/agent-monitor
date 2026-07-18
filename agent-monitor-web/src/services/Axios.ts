import axios, {
  AxiosHeaders,
  AxiosResponse,
  AxiosResponseHeaders,
  ResponseType,
} from "axios";

import { Typeof } from "hsu-utils";
import { debounce } from "lodash";
import { getAccessToken, getAdminToken, removeAdminToken } from "@/utils/auth";
import { notification } from "antd";
import wsCache from "@/utils/wsCache";
import { getFileNameFromHeader } from "hsu-utils/lib/DownloadFile";

/**
 * 获取 CSRF Token
 * 优先从 cookie 中获取，如果没有则从 meta 标签获取
 * @returns CSRF Token 字符串
 */
function getCsrfToken(): string {
  // 方法1: 从 cookie 中获取 CSRF token（常见名称）
  const csrfCookieNames = ["XSRF-TOKEN", "X-CSRF-TOKEN", "CSRF-TOKEN", "_csrf"];
  for (const name of csrfCookieNames) {
    const cookies = document.cookie.split(";");
    for (const cookie of cookies) {
      const [key, value] = cookie.trim().split("=");
      if (key === name && value) {
        return decodeURIComponent(value);
      }
    }
  }

  // 方法2: 从 meta 标签获取
  const metaToken = document.querySelector<HTMLMetaElement>(
    'meta[name="csrf-token"]'
  );
  if (metaToken?.content) {
    return metaToken.content;
  }

  // 方法3: 从 data 属性获取
  const dataToken = document.querySelector<HTMLElement>("[data-csrf-token]");
  if (dataToken?.dataset?.csrfToken) {
    return dataToken.dataset.csrfToken;
  }

  return "";
}

/**
 * 验证请求是否为同源请求
 * @param url 请求 URL
 * @returns 是否为同源请求
 */
function isSameOrigin(url: string): boolean {
  try {
    const urlObj = new URL(url, window.location.origin);
    return urlObj.origin === window.location.origin;
  } catch {
    // 如果是相对路径，认为是同源
    return url.startsWith("/") || !url.includes("://");
  }
}

const codeMessage: Record<number, string> = {
  200: "服务器成功返回请求的数据。",
  201: "新建或修改数据成功。",
  202: "一个请求已经进入后台排队（异步任务）。",
  204: "删除数据成功。",
  400: "发出的请求有错误，服务器没有进行新建或修改数据的操作。",
  401: "用户没有权限（令牌、用户名、密码错误）。",
  403: "用户得到授权，但是访问是被禁止的。",
  404: "发出的请求针对的是不存在的记录，服务器没有进行操作。",
  406: "请求的格式不可得。",
  410: "请求的资源被永久删除，且不会再得到的。",
  422: "当创建一个对象时，发生一个验证错误。",
  500: "服务器发生错误，请检查服务器。",
  502: "网关错误。",
  503: "服务不可用，服务器暂时过载或维护。",
  504: "网关超时。",
};

//清除所有cookie函数
export function clearAllCookie() {
  const keys = document.cookie.match(/[^ =;]+(?==)/g);

  keys?.forEach((key: string) => {
    document.cookie = key + "=0;expires=" + new Date(0).toUTCString();
  });
}

/**
 * 安全的重定向函数，防止 Open Redirect 漏洞
 * @param path 重定向路径，必须是相对路径（以 / 开头）
 */
function safeRedirect(path: string): void {
  // 验证路径必须是相对路径（以 / 开头）
  if (!path || typeof path !== "string" || !path.startsWith("/")) {
    path = "/login"; // 默认重定向到登录页
  }

  // 移除可能的查询参数和哈希，防止注入
  const cleanPath = path.split("?")[0].split("#")[0];

  // 确保路径不包含协议、域名等，只允许相对路径
  if (cleanPath.includes("://") || cleanPath.includes("//")) {
    // 使用当前 origin 构建完整 URL，确保只重定向到当前域名
    const currentOrigin = window.location.origin;
    window.location.href = `${currentOrigin}/login`;
    return;
  }

  // 使用当前 origin 构建完整 URL，确保只重定向到当前域名
  const currentOrigin = window.location.origin;
  window.location.href = `${currentOrigin}${cleanPath}`;
}

/** 无需登录即可浏览的页面：401 只静默清理过期登录态，不打断浏览 */
const PUBLIC_PATHS = ["/", "/login"];

// 重新登录
const reLogin = debounce(() => {
  // 官网首页等公开页：带着过期 token 刷新时，后台预取（菜单/权限）会 401，
  // 此时强跳登录页等于把公开页锁在登录后面。清掉过期凭证让页面回到
  // 未登录形态即可；真正需要登录的页面（/portal、后管）仍走跳转。
  if (PUBLIC_PATHS.includes(window.location.pathname)) {
    wsCache.clear();
    clearAllCookie();
    return;
  }
  notification.error({
    message: "登录已过期，请重新登录",
    duration: 0.5,
    onClose: () => {
      safeRedirect("/login");
      wsCache.clear();
      clearAllCookie();
    },
  });
});

const errMsg = debounce((status: number, url: string, errorText: string) => {
  notification.error({
    message: `请求错误 ${status}：${window.decodeURIComponent(url)}`,
    description: errorText,
  });
});

/**
 * fetch 响应拦截
 */
const { fetch: originalFetch } = window;
window.fetch = async (...args) => {
  const [resource, config = {}] = args;
  const requestUrl =
    typeof resource === "string"
      ? resource
      : resource instanceof Request
      ? resource.url
      : resource.toString();

  // CSRF 防护：为同源请求添加 CSRF token 和自定义 header
  if (requestUrl && isSameOrigin(requestUrl)) {
    const csrfToken = getCsrfToken();
    const headers = new Headers(config.headers);

    if (csrfToken) {
      headers.set("X-CSRF-TOKEN", csrfToken);
      headers.set("X-XSRF-TOKEN", csrfToken);
    }

    // 添加自定义 header 标识 AJAX 请求
    headers.set("X-Requested-With", "XMLHttpRequest");

    config.headers = headers;
  }

  const response = await originalFetch(resource, {
    ...config,
  });

  const { status, url, statusText, ok } = response;

  if (!ok) {
    if (status === 401) {
      reLogin();
    } else {
      const errorText = codeMessage[status] || statusText;

      errMsg(status, url, errorText);

      return Promise.reject(resource);
    }
  }

  try {
    const { value } = (await response.clone().body?.getReader()?.read()) ?? {};
    const decoder = new TextDecoder();
    if (value) {
      const text = decoder.decode(value);
      const { header } = JSON.parse(text ?? JSON.stringify({}));
      if (header?.code === 401) {
        reLogin();
      }
    }
  } catch {
    return Promise.resolve(response);
  }

  return Promise.resolve(response);
};

/**
 * axios 请求拦截
 */
axios.interceptors.request.use((config) => {
  const { url } = config;
  const dev = process.env.NODE_ENV === "development";
  const apiBase = process.env.API_BASE;
  if (dev && apiBase && url && !url.startsWith(apiBase)) {
    config.url = `${apiBase}${config.url}`;
  }

  // CSRF 防护：添加 CSRF token 和自定义 header
  if (url && isSameOrigin(url)) {
    const csrfToken = getCsrfToken();
    if (csrfToken) {
      // 添加 CSRF token 到请求头（常见的 header 名称）
      config.headers = config.headers || {};
      config.headers["X-CSRF-TOKEN"] = csrfToken;
      config.headers["X-XSRF-TOKEN"] = csrfToken;
    }

    // 添加自定义 header 标识 AJAX 请求，防止 CSRF 攻击
    config.headers = config.headers || {};
    config.headers["X-Requested-With"] = "XMLHttpRequest";
  }

  // 后管访问令牌：仅同源 /sys/* 管理接口附带，避免令牌外带到其它请求
  const adminToken = getAdminToken();
  if (
    adminToken &&
    config.url &&
    isSameOrigin(config.url) &&
    config.url.includes("/sys/")
  ) {
    config.headers = config.headers || {};
    config.headers["X-Admin-Token"] = adminToken;
  }

  return config;
});

/**
 * axios 响应拦截
 */
axios.interceptors.response.use(
  (res) => {
    const data = res.data;
    const { header } = data;

    if (header?.code === 401 && !skipAuthRedirect(res.config)) {
      reLogin();
    }

    // 4031 = 后管访问令牌无效（见 Rust admin_gate）
    if (data?.code === 4031) {
      onAdminTokenInvalid();
    }

    return res;
  },
  (err) => {
    if (!err?.response) return Promise.reject(err);

    const { status, config, statusText } = err.response;

    if (status === 401) {
      // 标记为静默的请求（如登录页探测钉钉开关）401 时不跳登录，仅向上抛错
      if (!skipAuthRedirect(err.config)) {
        reLogin();
      }
      return Promise.reject(err);
    } else {
      const errorText = codeMessage[status] || statusText;

      errMsg(status, config.url, errorText);

      return Promise.reject(err);
    }
  }
);

/** 读取请求配置上的「跳过 401 重定向」标记 */
function skipAuthRedirect(config?: unknown): boolean {
  return !!(config as { skipAuthRedirect?: boolean } | undefined)
    ?.skipAuthRedirect;
}

/**
 * 后管访问令牌失效（服务端轮换等）：清除本地令牌并回到锁屏，
 * 避免停留在「已解锁但所有管理接口都失败」的死锁状态。
 */
const onAdminTokenInvalid = debounce(() => {
  if (getAdminToken()) {
    removeAdminToken();
    notification.error({
      message: "后管访问令牌已失效，请重新解锁",
      duration: 0.8,
      onClose: () => window.location.reload(),
    });
  }
});

/**
 * axios 响应数据预处理
 */
export interface ResType<T = unknown> {
  header?: Record<string, string>;
  data: T;
  code: number;
  msg?: string;
}
const response = <T>(res: AxiosResponse<ResType<T>>): ResType<T> => {
  const data = res.data;

  if (Typeof(data) === "blob" || Typeof(data) === "arraybuffer") {
    const filename = getFileNameFromHeader(
      res as { headers: AxiosResponseHeaders }
    );

    return {
      data: {
        filename,
        data: data,
      } as unknown as T,
      code: 0,
    };
  }

  if (data.code === undefined) {
    return {
      data: data as unknown as T,
      code: 0,
    };
  }

  if (data.code === 401 && !skipAuthRedirect(res.config)) {
    reLogin();
  }

  return data;
};

// 读取 Token
const getToken = () => {
  const token = getAccessToken();
  // return token ? "Bearer " + token : "";
  return token ?? "";
};

/**
 * default config
 */
axios.defaults.withCredentials = true;
axios.defaults.headers["tenant-id"] = "0";

interface Params {
  query?: string;
}

/**
 * GET
 * @param url
 * @param config
 * @returns
 */
export const get = async <T, P = object>(
  url: string,
  config?: {
    params?: Params | P;
    responseType?: ResponseType;
    headers?: AxiosHeaders;
    /** 为 true 时该请求 401 不触发全局跳登录（用于未登录态的探测请求） */
    skipAuthRedirect?: boolean;
  }
): Promise<ResType<T>> => {
  if (!url) return Promise.reject("url is required");

  let _url = url;

  const params = config?.params as Params;
  if (params?.query) {
    _url = `${_url}?query=${params.query}`;
    delete params.query;
  }

  const res = await axios.get(_url, {
    ...config,
    headers: {
      Authorization: getToken(),
      ...config?.headers,
    },
  });
  return response<T>(res);
};

/**
 * POST
 * @param url
 * @param config
 * @returns
 */
export const post = async <T = undefined, D = object, P = object>(
  url: string,
  data?: D,
  config?: {
    params?: Params | P;
    responseType?: ResponseType;
    headers?: AxiosHeaders;
  }
): Promise<ResType<T>> => {
  if (!url) return Promise.reject("url is required");

  let _url = url;

  const params = config?.params as Params;
  if (params?.query) {
    _url = `${_url}?query=${params.query}`;
    delete params.query;
  }

  const res = await axios.post(_url, data, {
    ...config,
    headers: {
      Authorization: getToken(),
      ...config?.headers,
    },
  });
  return response<T>(res);
};

/**
 * DELETE
 * @param url
 * @param config
 * @returns
 */
export const del = async <T = undefined, P = object>(
  url: string,
  config?: {
    params?: Params | P;
    headers?: AxiosHeaders;
  }
): Promise<ResType<T>> => {
  if (!url) return Promise.reject("url is required");

  let _url = url;

  const params = config?.params as Params;
  if (params?.query) {
    _url = `${_url}?query=${params.query}`;
    delete params.query;
  }

  const res = await axios.delete(_url, {
    ...config,
    headers: {
      Authorization: getToken(),
      ...config?.headers,
    },
  });
  return response<T>(res);
};

/**
 * PUT
 * @param url
 * @param config
 * @returns
 */
export const put = async <T = undefined>(
  url: string,
  data?: object,
  config?: {
    headers?: AxiosHeaders;
  }
): Promise<ResType<T>> => {
  if (!url) return Promise.reject("url is required");

  const res = await axios.put(url, data, {
    headers: {
      Authorization: getToken(),
      ...config?.headers,
    },
  });
  return response<T>(res);
};

