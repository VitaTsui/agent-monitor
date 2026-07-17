import React, { ReactNode } from "react";
import { Space } from "antd";
import { Icon } from "@hsu-react/ui";

interface BreadcrumbItemIconProps {
  icon?: ReactNode;
  title?: ReactNode;
}

/**
 * 面包屑项图标和标题组件
 */
const BreadcrumbItemIcon: React.FC<BreadcrumbItemIconProps> = ({
  icon,
  title,
}) => {
  return (
    <Space size={5}>
      {typeof icon === "string" ? <Icon icon={icon} /> : icon}
      {title}
    </Space>
  );
};

export default BreadcrumbItemIcon;

