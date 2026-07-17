import { useEffect, useState, useContext, useMemo } from "react";
import { useLocation } from "react-router";
import { BreadcrumbType } from "..";
import { checkBreadcrumbPathMatch } from "../_utils/pathMatch";
import { NavTabBarTitleContent } from "@/hooks/useSetTabTitle";

/**
 * 处理面包屑路径匹配的 hook
 */
export const useBreadcrumbPath = (_routes: BreadcrumbType[]) => {
  const location = useLocation();
  const { tabTitles } = useContext(NavTabBarTitleContent);
  const [breadcrumb, setBreadcrumb] = useState<BreadcrumbType[]>([]);

  const pathname = useMemo(
    () => decodeURI(location.pathname),
    [location.pathname]
  );

  useEffect(() => {
    const _setBreadcrumb = (
      routes: BreadcrumbType[],
      parents?: BreadcrumbType[]
    ) => {
      const checkPath = (item: BreadcrumbType) => {
        if (item.path) {
          const isMatch = checkBreadcrumbPathMatch(
            pathname,
            item.path,
            item.parent?.path
          );

          if (isMatch) {
            // 如果 tabTitles 中有当前路径的标题，则应用自定义标题
            const finalItem =
              tabTitles && tabTitles[pathname] !== undefined
                ? { ...item, title: tabTitles[pathname] }
                : item;

            if (parents) {
              return [...parents, finalItem];
            } else {
              return [finalItem];
            }
          }
        }

        return [];
      };

      routes?.forEach((item) => {
        if (item.children) {
          if (item.route?.index) {
            const _breadcrumb = checkPath(item);
            if (_breadcrumb.length > 0) {
              setBreadcrumb(_breadcrumb);
            } else {
              const _newParents = parents ? [...parents, item] : [item];
              _setBreadcrumb(item.children, _newParents);
            }
          } else {
            const _newParents = parents ? [...parents, item] : [item];
            _setBreadcrumb(item.children, _newParents);
          }
        } else {
          const _breadcrumb = checkPath(item);
          if (_breadcrumb.length > 0) {
            setBreadcrumb(_breadcrumb);
          }
        }
      });
    };

    setBreadcrumb([]);
    _setBreadcrumb(_routes);
  }, [_routes, pathname, tabTitles]);

  // 当 tabTitles 变化时，更新最后一个面包屑项的标题
  useEffect(() => {
    if (!pathname) return;

    setBreadcrumb((prev) => {
      if (prev.length === 0) return prev;

      const newBreadcrumb = [...prev];
      const lastItem = newBreadcrumb[newBreadcrumb.length - 1];

      // 检查当前路径是否匹配最后一个面包屑项
      const isPathMatch =
        lastItem.path === pathname ||
        (lastItem.path &&
          checkBreadcrumbPathMatch(
            pathname,
            lastItem.path,
            lastItem.parent?.path
          ));

      if (isPathMatch && tabTitles && tabTitles[pathname] !== undefined) {
        // 如果 tabTitles 中有当前路径的标题，则更新
        newBreadcrumb[newBreadcrumb.length - 1] = {
          ...lastItem,
          title: tabTitles[pathname],
        };
      }

      return newBreadcrumb;
    });
  }, [tabTitles, pathname]);

  return breadcrumb;
};
