import React from "react";
import { Popover } from "antd";
import { TabType } from "../..";
import TabContextMenu from "../TabContextMenu";
import styles from "../../index.module.scss";
import classNames from "classnames";

interface TabLabelProps {
  item: TabType;
  index: number;
  openKeys: TabType[];
  basePath: string;
  affixRouter: string[];
  open: string;
  onReload: (key: string) => void;
  refresh: (key: string) => void;
  drop: (key: string) => void;
  setOpenkeys: React.Dispatch<React.SetStateAction<TabType[]>>;
  setOpen: (key: string) => void;
  navigate: (path: string) => void;
  className?: string;
}

/**
 * 标签页标签组件
 */
const TabLabel: React.FC<TabLabelProps> = ({
  item,
  index,
  openKeys,
  basePath,
  affixRouter,
  open,
  onReload,
  refresh,
  drop,
  setOpenkeys,
  setOpen,
  navigate,
  className,
}) => {
  return (
    <Popover
      open={item.key === open}
      placement="bottomLeft"
      classNames={{ root: styles.popover }}
      content={
        <TabContextMenu
          item={item}
          index={index}
          openKeys={openKeys}
          basePath={basePath}
          affixRouter={affixRouter}
          onReload={onReload}
          refresh={refresh}
          drop={drop}
          setOpenkeys={setOpenkeys}
          setOpen={setOpen}
          navigate={navigate}
        />
      }
    >
      <span
        onContextMenu={(event) => {
          event.preventDefault();
        }}
        onMouseDown={(e) => {
          if (e.button === 2) {
            setOpen(item.key === open ? "" : item.key);
          } else {
            setOpen("");
            navigate(item.key);
          }
        }}
        className={classNames(styles.tabLabel, className)}
      >
        {item.icon}
        {item.label}
      </span>
    </Popover>
  );
};

export default TabLabel;
