import { cloneDeep } from "lodash";
import { RouteType } from "@/router/router.config";
import { MenuType } from "..";

/**
 * 格式化路由为菜单项
 * @param routes 路由配置
 * @param all 是否包含所有路由（包括不在菜单中显示的）
 * @param parent 父菜单项
 * @returns 菜单项数组
 */
export const formatMenu = (
  routes: RouteType[],
  all: boolean = false,
  parent?: MenuType
): MenuType[] => {
  const newRoutes: MenuType[] = [];

  routes?.forEach((route) => {
    let children: MenuType[] | undefined = undefined;
    if (route.children) {
      children = formatMenu(route.children, all, cloneDeep(route) as MenuType);
    }

    const { meta } = route;

    if ((!all ? meta?.menu : true) && route.path) {
      newRoutes.push({
        key: route.path,
        title: meta?.name,
        label: meta?.name,
        icon: meta?.icon,
        defaultIcon: meta?.icon,
        activeIcon: meta?.activeIcon,
        disabled: meta?.disabled,
        children: (children?.length || 0) > 0 ? children : undefined,
        parent: parent,
      });
    } else if ((!all ? meta?.menu : true) && route.index && parent?.path) {
      newRoutes.push({
        key: parent.path,
        title: meta?.name,
        label: meta?.name,
        icon: meta?.icon,
        defaultIcon: meta?.icon,
        activeIcon: meta?.activeIcon,
        disabled: meta?.disabled,
      });
    } else {
      if (children) {
        newRoutes.push(...children);
      }
    }
  });

  return newRoutes;
};
