import { array_is_includes } from "hsu-utils";

/**
 * 检查路径是否匹配（用于标签页）
 * 注意：固定路由（affixRouter 和 affix）的匹配已在 useTabPath 的 _checkAffix 中单独处理
 * @param pathname 当前路径
 * @param key 路由 key
 * @returns 是否匹配
 */
export const checkTabPathMatch = (pathname: string, key: string): boolean => {
  // 处理根路径
  if (pathname === "/" && key === "/") {
    return true;
  }

  const pathArr = pathname.split("/").filter(Boolean);
  const keyArr = key.split("/").filter(Boolean);

  if (keyArr.length === 0) {
    return false;
  }

  if (pathArr.length !== keyArr.length) {
    return false;
  }

  // 处理包含参数的路由（如 /user/:id）
  if (key.includes(":")) {
    return array_is_includes(
      keyArr.filter((i) => !i.startsWith(":")),
      pathArr
    );
  } else {
    // 普通路径匹配
    return array_is_includes(keyArr, pathArr);
  }
};
