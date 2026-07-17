import { Tabs as AntdTabs } from "antd";
import React, { ReactNode, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import type { DragEndEvent, DragStartEvent } from "@dnd-kit/core";
import {
  closestCenter,
  DndContext,
  PointerSensor,
  useSensor,
} from "@dnd-kit/core";
import {
  restrictToHorizontalAxis,
  restrictToParentElement,
} from "@dnd-kit/modifiers";
import {
  arrayMove,
  horizontalListSortingStrategy,
  SortableContext,
} from "@dnd-kit/sortable";

import { RouteType } from "@/router/router.config";
import styles from "./index.module.scss";
import { useAliveController } from "react-activation";
import useReload from "@/hooks/useReload";
import { formatNavTabBar } from "./_utils/formatNavTabBar";
import { useTabPath } from "./_hooks/useTabPath";
import { useTabContextMenu } from "./_hooks/useTabContextMenu";
import { useDropTabKey } from "./_hooks/useDropTabKey";
import { useTabTitle } from "./_hooks/useTabTitle";
import TabLabel from "./_components/TabLabel";
import DraggableTabNode from "./_components/SortableTab";
import TabDragOverlay from "./_components/TabDragOverlay";
import { isLegacyHasSelectorBrowser } from "@/utils/cssSupports";

export interface TabType {
  label: ReactNode;
  key: string;
  element?: React.ReactNode;
  children?: TabType[];
  path?: string;
  name?: ReactNode;
  affix?: boolean;
  icon?: ReactNode;
}

export interface NavTabBarProps {
  router: RouteType[];
  affixRouter?: string[];
  basePath?: string;
}

const NavTabBar: React.FC<NavTabBarProps> = (props) => {
  const { router, affixRouter = [], basePath = "/" } = props;
  const navigate = useNavigate();
  const { drop, refresh } = useAliveController();
  const onReload = useReload();
  const legacyHasSelector = isLegacyHasSelectorBrowser();

  const items = useMemo(() => {
    return formatNavTabBar(router);
  }, [router]);

  const { tabKey, openKeys, setOpenkeys } = useTabPath({
    items,
    affixRouter,
  });

  const { open, setOpen } = useTabContextMenu();

  useDropTabKey(setOpenkeys);
  useTabTitle(setOpenkeys);

  // 当前正在拖拽的标签页 ID
  const [activeId, setActiveId] = useState<string | null>(null);

  // 配置拖拽传感器，设置10px的激活距离以避免误触
  const sensor = useSensor(PointerSensor, {
    activationConstraint: { distance: 10 },
  });

  // 处理拖拽开始事件
  const onDragStart = (event: DragStartEvent) => {
    setActiveId(String(event.active.id));
  };

  // 处理拖拽结束事件
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (active.id !== over?.id) {
      setOpenkeys((prev) => {
        const activeIndex = prev.findIndex((i) => i.key === active.id);
        const overIndex = prev.findIndex((i) => i.key === over?.id);
        return arrayMove(prev, activeIndex, overIndex);
      });
    }
    setActiveId(null);
  };

  // 处理拖拽取消事件
  const onDragCancel = () => {
    setActiveId(null);
  };

  return (
    <AntdTabs
      className={styles.NavTabBar}
      items={openKeys?.map((item, idx) => {
        const closable = !(affixRouter.includes(item.key) || item.affix);

        return {
          label: (
            <TabLabel
              item={item}
              index={idx}
              openKeys={openKeys}
              basePath={basePath}
              affixRouter={affixRouter}
              open={open}
              onReload={onReload}
              refresh={refresh}
              drop={drop}
              setOpenkeys={setOpenkeys}
              setOpen={setOpen}
              navigate={navigate}
              className={
                legacyHasSelector && closable
                  ? styles.legacyClosableTabLabel
                  : undefined
              }
            />
          ),
          key: item.key,
          closeIcon: closable ? undefined : false,
        };
      })}
      activeKey={tabKey}
      hideAdd
      type="editable-card"
      onEdit={(key, action) => {
        if (action === "remove") {
          drop((key as string)?.split("?")[0] || "");
          // 找到当前要关闭的标签页的索引
          const currentIndex = openKeys.findIndex((item) => item.key === key);
          const newOpenKeys = openKeys.filter((item) => item.key !== key);
          setOpenkeys(newOpenKeys);
          setOpen("");
          if (key === tabKey) {
            // 如果关闭的是当前标签页，选中前一个标签页，如果没有前一个则选中第一个
            const targetIndex = currentIndex > 0 ? currentIndex - 1 : 0;
            navigate(newOpenKeys[targetIndex]?.key || basePath);
          }
        }
      }}
      size="small"
      renderTabBar={(tabBarProps, DefaultTabBar) => (
        <DndContext
          sensors={[sensor]}
          onDragStart={onDragStart}
          onDragEnd={onDragEnd}
          onDragCancel={onDragCancel}
          collisionDetection={closestCenter}
          modifiers={[restrictToHorizontalAxis, restrictToParentElement]}
        >
          <SortableContext
            items={openKeys.map((i) => i.key)}
            strategy={horizontalListSortingStrategy}
          >
            <DefaultTabBar {...tabBarProps}>
              {(node) => (
                <DraggableTabNode {...node.props} key={node.key}>
                  {node}
                </DraggableTabNode>
              )}
            </DefaultTabBar>
          </SortableContext>
          <TabDragOverlay
            activeId={activeId}
            openKeys={openKeys}
            basePath={basePath}
            affixRouter={affixRouter}
            open={open}
            onReload={onReload}
            refresh={refresh}
            drop={drop}
            setOpenkeys={setOpenkeys}
            setOpen={setOpen}
            navigate={navigate}
          />
        </DndContext>
      )}
    />
  );
};
export default NavTabBar;
