import React from "react";
import { Button } from "antd";
import { Icon } from "@hsu-react/ui";
import { TabType } from "../..";
import {
  closeTab,
  closeLeftTabs,
  closeRightTabs,
  closeOtherTabs,
  closeAllTabs,
} from "../../_utils/tabActions";
import styles from "../../index.module.scss";

interface TabContextMenuProps {
  item: TabType;
  index: number;
  openKeys: TabType[];
  basePath: string;
  affixRouter: string[];
  onReload: (key: string) => void;
  refresh: (key: string) => void;
  drop: (key: string) => void;
  setOpenkeys: React.Dispatch<React.SetStateAction<TabType[]>>;
  setOpen: (key: string) => void;
  navigate: (path: string) => void;
}

/**
 * 标签页右键菜单组件
 */
const TabContextMenu: React.FC<TabContextMenuProps> = ({
  item,
  index,
  openKeys,
  basePath,
  affixRouter,
  onReload,
  refresh,
  drop,
  setOpenkeys,
  setOpen,
  navigate,
}) => {
  // 判断当前标签页是否为固定标签页
  const isAffixed = affixRouter.includes(item.key) || item.affix;

  return (
    <div className={styles.tabContextMenu}>
      <div className={styles.part}>
        <Button
          type="text"
          onClick={() => {
            onReload(item.key?.split("?")[0] || "");
            refresh(item.key?.split("?")[0] || "");
            setOpen("");
          }}
          icon={<Icon icon="ep:refresh" />}
        >
          重新加载
        </Button>
        <Button
          type="text"
          disabled={isAffixed}
          onClick={() => {
            const newOpenKeys = closeTab(openKeys, index, drop, affixRouter);
            setOpenkeys(newOpenKeys);
            setOpen("");
            // 关闭标签页后，选中前一个标签页，如果没有前一个则选中第一个
            const targetIndex = index > 0 ? index - 1 : 0;
            navigate(newOpenKeys[targetIndex]?.key || basePath);
          }}
          icon={<Icon icon="ep:close" />}
        >
          关闭标签页
        </Button>
      </div>
      <div className={styles.part}>
        <Button
          type="text"
          disabled={index === 0}
          onClick={() => {
            const newOpenKeys = closeLeftTabs(
              openKeys,
              index,
              drop,
              affixRouter
            );
            setOpenkeys(newOpenKeys);
            setOpen("");
            navigate(item.key);
          }}
          icon={<Icon icon="ep:d-arrow-left" />}
        >
          关闭左侧标签页
        </Button>
        <Button
          type="text"
          disabled={index === openKeys.length - 1}
          onClick={() => {
            const newOpenKeys = closeRightTabs(
              openKeys,
              index,
              drop,
              affixRouter
            );
            setOpenkeys(newOpenKeys);
            setOpen("");
            navigate(item.key);
          }}
          icon={<Icon icon="ep:d-arrow-right" />}
        >
          关闭右侧标签页
        </Button>
      </div>
      <div className={styles.part}>
        <Button
          type="text"
          disabled={openKeys.length <= 1}
          onClick={() => {
            const newOpenKeys = closeOtherTabs(
              openKeys,
              item,
              drop,
              affixRouter
            );
            setOpenkeys(newOpenKeys);
            setOpen("");
            navigate(item.key);
          }}
          icon={<Icon icon="ep:discount" />}
        >
          关闭其他标签页
        </Button>
        <Button
          type="text"
          disabled={isAffixed && openKeys.length <= 1}
          onClick={() => {
            const newOpenKeys = closeAllTabs(openKeys, drop, affixRouter);
            setOpenkeys(newOpenKeys);
            setOpen("");
            // 如果有保留的固定标签页，导航到第一个固定标签页
            if (newOpenKeys.length > 0) {
              navigate(newOpenKeys[0].key);
            } else {
              navigate(basePath);
            }
          }}
          icon={<Icon icon="ep:minus" />}
        >
          关闭全部标签页
        </Button>
      </div>
    </div>
  );
};

export default TabContextMenu;
