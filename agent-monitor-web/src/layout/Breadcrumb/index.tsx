import { Breadcrumb as AntBreadcrumb } from "antd";
import React, { ReactNode, useMemo, useCallback } from "react";
import { useNavigate } from "react-router";

import { ItemType } from "antd/es/breadcrumb/Breadcrumb";
import { RouteType } from "@/router/router.config";
import styles from "./index.module.scss";
import { cloneDeep } from "lodash";
import classNames from "classnames";
import { formatRoutes } from "./_utils/formatRoutes";
import { formatBreadcrumbMenu } from "./_utils/formatMenu";
import { useBreadcrumbPath } from "./_hooks/useBreadcrumbPath";
import BreadcrumbItemIcon from "./_components/BreadcrumbItemIcon";

export interface BreadcrumbType extends ItemType {
  children?: BreadcrumbType[];
  icon?: ReactNode;
  parent?: RouteType;
  route?: RouteType;
  index?: boolean;
}

interface BreadcrumbProps {
  router: RouteType[];
  className?: string;
}

/**
 * 面包屑组件
 */
const Breadcrumb: React.FC<BreadcrumbProps> = (props) => {
  const { router, className } = props;
  const navigate = useNavigate();

  const _routes = useMemo(() => {
    return formatRoutes(router);
  }, [router]);

  const breadcrumb = useBreadcrumbPath(_routes);

  /**
   * 处理面包屑项点击事件
   */
  const handleItemClick = useCallback(
    (path: string) => {
      navigate(path);
    },
    [navigate]
  );

  /**
   * 格式化面包屑项
   */
  const formattedItems = useMemo(() => {
    return cloneDeep(breadcrumb)?.map((item, index) => {
      const isLastItem = index === breadcrumb.length - 1;
      const children = item.children?.filter((i) => !i.path?.includes(":"));
      const hasChildren = children && children.length > 0;

      // 如果有子菜单，设置菜单项
      if (hasChildren) {
        item.onClick = (e) => e.preventDefault();
        item.menu = {
          items: formatBreadcrumbMenu(cloneDeep(children), navigate),
        };
      }

      // 最后一项不显示路径
      if (isLastItem) {
        item.path = undefined;
      }

      // 如果有图标，包装标题
      if (item.icon) {
        item.title = <BreadcrumbItemIcon icon={item.icon} title={item.title} />;
      }

      // 如果有路径且没有子菜单，设置点击事件
      if (item.path && !hasChildren) {
        item.onClick = (e) => {
          e.preventDefault();
          handleItemClick(item.path as string);
        };
      }

      return { ...item };
    });
  }, [breadcrumb, navigate, handleItemClick]);

  return (
    <AntBreadcrumb
      className={classNames(styles.Breadcrumb, className)}
      items={formattedItems}
    />
  );
};

export default Breadcrumb;
