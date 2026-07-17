import { array_is_includes } from "hsu-utils";

/**
 * 检查路径是否匹配
 * @param pathname 当前路径
 * @param key 路由 key
 * @param parentPath 父路径
 * @returns 是否匹配
 */
export const checkPathMatch = (
  pathname: string,
  key: string,
  parentPath?: string
): boolean => {
  const pathArr = pathname.split("/").filter(Boolean);
  const keyArr = key.split("/").filter(Boolean);

  if (keyArr.length === 0) {
    return pathname === "/" && key === "/";
  }

  if (pathArr.length !== keyArr.length) {
    return false;
  }

  // 处理根路径
  if (pathname === "/" && key === "/") {
    return true;
  }

  // 处理包含参数的路由（如 /user/:id）
  if (key.includes(":")) {
    if (parentPath === "/") {
      // 如果父路径是根路径，检查是否没有非参数段
      return !keyArr.filter((i: string) => !i.startsWith(":")).length;
    } else {
      // 过滤掉参数段，只比较非参数部分
      return array_is_includes(
        keyArr.filter((i: string) => !i.startsWith(":")),
        pathArr
      );
    }
  } else {
    // 普通路径匹配
    return array_is_includes(keyArr, pathArr);
  }
};

