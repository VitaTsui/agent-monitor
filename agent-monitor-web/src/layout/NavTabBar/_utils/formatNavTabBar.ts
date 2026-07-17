import { cloneDeep } from "lodash";
import { RouteType } from "@/router/router.config";
import { TabType } from "..";

/**
 * 格式化路由为标签页项
 * @param routes 路由配置
 * @param parent 父标签页项
 * @returns 标签页项数组
 */
export const formatNavTabBar = (
  routes: RouteType[],
  parent?: TabType
): TabType[] => {
  const newRoutes: TabType[] = [];

  routes?.forEach((route) => {
    let children: TabType[] | undefined = undefined;
    if (route.children) {
      children = formatNavTabBar(route.children, cloneDeep(route) as TabType);
    }

    const { meta } = route;

    if (!meta?.noTabsView && route.path) {
      newRoutes.push({
        key: decodeURI(route.path),
        label: meta?.name ?? "",
        children: (children?.length || 0) > 0 ? children : undefined,
        element: route.element,
        affix: meta?.affix,
        icon: meta?.icon,
      });
    } else if (!meta?.noTabsView && route.index && parent?.path) {
      newRoutes.push({
        key: decodeURI(parent.path),
        label: meta?.name ?? "",
        element: route.element,
        affix: meta?.affix,
        icon: meta?.icon,
      });
    } else {
      if (children) {
        newRoutes.push(...children);
      }
    }
  });

  return newRoutes;
};
