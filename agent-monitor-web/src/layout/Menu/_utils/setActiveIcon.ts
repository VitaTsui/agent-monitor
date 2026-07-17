import { MenuType } from "..";

/**
 * 设置菜单项的激活图标
 * @param items 菜单项数组
 * @param menuKey 当前激活的菜单 key
 */
export const setActiveIcon = (items: MenuType[], menuKey: string): void => {
  items?.forEach((item) => {
    if (item.children) {
      setActiveIcon(item.children, menuKey);
    } else {
      if (item.activeIcon && item.key === menuKey) {
        item.icon = item.activeIcon;
      } else {
        item.icon = item.defaultIcon;
      }
    }

    delete item.activeIcon;
    delete item.defaultIcon;
  });
};
