/**
 * 配置浏览器本地存储的方式，可直接存储对象数组。
 */

import WebStorageCache from "web-storage-cache";

type CacheType = "localStorage" | "sessionStorage";

export const CACHE_KEY = {
  ROLE_ROUTERS: "roleRouters",
  ROLE_PERMISSION: "rolePermission",
  DICT_CACHE: "dictCache",
  USER: "user",
  LANG: "lang",
  THEME: "theme",
  LAYOUT: "layout",
  HEADER_THEME: "headerTheme",
  PRIMARY_COLOR: "primaryColor",
  APPEARANCE: "appearance",
  PAGE_SIZE: "pageSize",
};

export function WSCache(type: CacheType = "localStorage") {
  const wsCache: WebStorageCache = new WebStorageCache({
    storage: type,
  });

  return wsCache;
}

const wsCache = WSCache();
export default wsCache;
