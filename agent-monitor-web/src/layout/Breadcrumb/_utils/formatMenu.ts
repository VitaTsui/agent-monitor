import React from "react";
import { BreadcrumbType } from "..";
import { ItemType as MenuItemType } from "antd/es/menu/interface";
import BreadcrumbMenuItem from "../_components/BreadcrumbMenuItem";
import { cloneDeep } from "lodash";

/**
 * 格式化面包屑子菜单
 * @param children 子面包屑项
 * @param navigate 导航函数
 * @returns 菜单项数组
 */
export const formatBreadcrumbMenu = (
  children: BreadcrumbType[],
  navigate: (path: string) => void
): Omit<MenuItemType, "null">[] => {
  return children?.map((item) => {
    const children = item.children?.filter((i) => !i.path?.includes(":"));
    const childrenBreadcrumb = formatBreadcrumbMenu(
      cloneDeep(children || []),
      navigate
    );

    return {
      key: decodeURI(item.path ?? ""),
      label: React.createElement(BreadcrumbMenuItem, { item, navigate }),
      children: childrenBreadcrumb.length > 0 ? childrenBreadcrumb : undefined,
    };
  });
};
