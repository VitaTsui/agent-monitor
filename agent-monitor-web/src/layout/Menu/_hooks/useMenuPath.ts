import { useEffect, useState } from "react";
import { useLocation } from "react-router";
import { MenuType } from "..";
import { checkPathMatch } from "../_utils/pathMatch";

interface UseMenuPathOptions {
  allItems: MenuType[];
  collapsed: boolean;
  mode?: string;
}

/**
 * 处理菜单路径匹配和激活状态的 hook
 */
export const useMenuPath = ({
  allItems,
  collapsed,
  mode = "inline",
}: UseMenuPathOptions) => {
  const location = useLocation();
  const [menuKey, setMenuKey] = useState("");
  const [openKeys, setOpenkeys] = useState<string[]>([]);

  useEffect(() => {
    const _checkPath = (items: MenuType[], parents?: MenuType[]) => {
      const pathname = decodeURI(location.pathname);

      items?.forEach((item) => {
        if (item.children) {
          const _parents = parents ? [...parents, item] : [item];
          _checkPath(item.children, _parents);
        } else {
          const isMatch = checkPathMatch(pathname, item.key, item.parent?.path);

          if (isMatch) {
            // 处理参数路由
            if (item.key.includes(":") && item.parent?.path) {
              setMenuKey(item.parent.path);
            } else {
              setMenuKey(item.key);
            }

            // 设置展开的菜单项
            if (parents && !collapsed && mode !== "horizontal") {
              const _openKeys = parents?.map((item) => item.key);
              setOpenkeys([..._openKeys]);
            }
          }
        }
      });
    };

    setMenuKey("");
    setOpenkeys([]);
    _checkPath(allItems);
  }, [allItems, location, collapsed, mode]);

  return { menuKey, setMenuKey, openKeys, setOpenkeys };
};
