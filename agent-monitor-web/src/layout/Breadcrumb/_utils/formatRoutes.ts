import { cloneDeep } from "lodash";
import { RouteType } from "@/router/router.config";
import { BreadcrumbType } from "..";

/**
 * 格式化路由为面包屑项
 * @param router 路由配置
 * @param parent 父路由
 * @returns 面包屑项数组
 */
export const formatRoutes = (
  router: RouteType[],
  parent?: RouteType
): BreadcrumbType[] => {
  const newRoutes: BreadcrumbType[] = [];

  router?.forEach((route) => {
    let children: BreadcrumbType[] | undefined = undefined;
    if (route.children) {
      children = formatRoutes(route.children, cloneDeep(route) as RouteType);
    }

    const { meta } = route;

    if (meta?.menu || !meta?.noTabsView) {
      let path = "";

      if (route.path) {
        path = decodeURI(route.path);
      }

      if (route.index) {
        newRoutes.push({
          path: parent?.path,
          title: meta?.name,
          children: (children?.length || 0) > 0 ? children : undefined,
          icon: meta?.icon,
          parent: parent,
          route: route,
          index: true,
        });
      } else {
        const indexChild = children?.find((child) => child?.index);
        const newChildren = children?.filter((child) => !child?.index);

        if (indexChild) {
          newRoutes.push({
            ...indexChild,
            index: false,
            parent: parent,
            children: (newChildren?.length || 0) > 0 ? newChildren : undefined,
          });
        } else {
          newRoutes.push({
            path,
            title: meta?.name,
            children: (children?.length || 0) > 0 ? children : undefined,
            icon: meta?.icon,
            parent: parent,
          });
        }
      }
    } else {
      if (children) {
        newRoutes.push(...children);
      }
    }
  });

  return newRoutes;
};
