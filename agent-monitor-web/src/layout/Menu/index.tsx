import React, { ReactNode, useMemo } from "react";
import { useNavigate } from "react-router";

import { Menu as AntdMenu, MenuProps as AntdMenuProps } from "antd";
import { ItemType } from "antd/es/menu/interface";
import { RouteType } from "@/router/router.config";
import { cloneDeep } from "lodash";
import classNames from "classnames";
import styles from "./index.module.scss";
import { formatMenu } from "./_utils/formatMenu";
import { setActiveIcon } from "./_utils/setActiveIcon";
import { useMenuPath } from "./_hooks/useMenuPath";
import { useOnlyLvOneMenu } from "./_hooks/useOnlyLvOneMenu";

export type MenuType = ItemType & {
  key: string;
  icon?: ReactNode;
  defaultIcon?: ReactNode;
  activeIcon?: ReactNode;
  children?: MenuType[];
  label?: ReactNode;
  path?: string;
  name?: ReactNode;
  title?: string;
  disabled?: boolean;
  renderLabel?: (label: string) => ReactNode;
  parent?: MenuType;
};

interface MenuProps extends AntdMenuProps {
  router: RouteType[];
  collapsed?: boolean;
  menuItems?: MenuType[];
  onlyLvOneMenu?: boolean;
  getCurrChildItems?: (children: MenuType[]) => void;
}

const Menu: React.FC<MenuProps> = (props) => {
  const {
    router,
    collapsed = false,
    className,
    theme = "dark",
    mode = "inline",
    getCurrChildItems,
    menuItems,
    onlyLvOneMenu = false,
    ...menuConfig
  } = props;
  const navigate = useNavigate();

  const items = useMemo(() => {
    return menuItems ? cloneDeep(menuItems) : formatMenu(router);
  }, [router, menuItems]);

  const allItems = useMemo(() => {
    return menuItems
      ? cloneDeep(menuItems)
      : formatMenu(cloneDeep(router), true);
  }, [router, menuItems]);

  const { menuKey, setMenuKey, openKeys, setOpenkeys } = useMenuPath({
    allItems,
    collapsed,
    mode,
  });

  const _items: MenuType[] = useMemo(() => {
    const _items: MenuType[] = cloneDeep(items);
    setActiveIcon(_items, menuKey);
    return _items;
  }, [items, menuKey]);

  useOnlyLvOneMenu({
    items,
    onlyLvOneMenu,
    getCurrChildItems,
    setOpenkeys,
    setMenuKey,
  });

  // Cloudflare 风格：侧边栏（inline）把一级目录渲染为不可折叠的「分组」，
  // 一级=灰色分区标题，二级=平铺项；展开模式下不再用可折叠子菜单。
  const cfGroupItems = useMemo<ItemType[]>(() => {
    return _items.map((it) =>
      it.children?.length && !collapsed
        ? ({
            type: "group",
            key: it.key,
            label: it.title ?? it.label,
            children: it.children,
          } as ItemType)
        : (it as ItemType)
    );
  }, [_items, collapsed]);

  return (
    <AntdMenu
      {...menuConfig}
      mode={mode}
      inlineCollapsed={collapsed}
      items={
        onlyLvOneMenu
          ? (_items?.map((i) => ({ ...i, children: undefined })) as ItemType[])
          : mode === "inline"
            ? cfGroupItems
            : _items
      }
      onClick={(v) => {
        if (onlyLvOneMenu) {
          getCurrChildItems?.(
            items.find((i) => i.key === v.key)?.children || []
          );
          setOpenkeys([v.key]);
          setMenuKey(v.key);
        } else {
          navigate(v.key);
        }
      }}
      selectedKeys={[menuKey]}
      openKeys={openKeys}
      onOpenChange={(keys: string[]) => {
        setOpenkeys(keys);
      }}
      theme={theme}
      className={classNames(styles.menu, className)}
    />
  );
};

export default Menu;
