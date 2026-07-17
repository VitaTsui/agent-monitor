import { TabType } from "..";

/**
 * 关闭指定索引的标签页（保留固定标签页）
 */
export const closeTab = (
  openKeys: TabType[],
  index: number,
  drop: (key: string) => void,
  affixRouter: string[] = []
): TabType[] => {
  const targetItem = openKeys[index];
  // 如果是固定标签页，则不关闭
  if (affixRouter.includes(targetItem.key) || targetItem.affix) {
    return openKeys;
  }

  const newOpenKeys = [...openKeys];
  const removed = newOpenKeys.splice(index, 1);
  removed?.forEach((item) => {
    drop(item.key?.split("?")[0] || "");
  });
  return newOpenKeys;
};

/**
 * 关闭左侧标签页（保留固定标签页）
 */
export const closeLeftTabs = (
  openKeys: TabType[],
  index: number,
  drop: (key: string) => void,
  affixRouter: string[] = []
): TabType[] => {
  // 分离左侧标签页
  const leftTabs = openKeys.slice(0, index);
  const rightTabs = openKeys.slice(index);

  // 过滤出左侧非固定的标签页并关闭
  const removedTabs = leftTabs.filter(
    (item) => !affixRouter.includes(item.key) && !item.affix
  );
  removedTabs.forEach((item) => {
    drop(item.key?.split("?")[0] || "");
  });

  // 保留左侧固定的标签页和右侧所有标签页
  const keptLeftTabs = leftTabs.filter(
    (item) => affixRouter.includes(item.key) || item.affix
  );
  return [...keptLeftTabs, ...rightTabs];
};

/**
 * 关闭右侧标签页（保留固定标签页）
 */
export const closeRightTabs = (
  openKeys: TabType[],
  index: number,
  drop: (key: string) => void,
  affixRouter: string[] = []
): TabType[] => {
  // 分离左侧标签页和右侧标签页
  const leftTabs = openKeys.slice(0, index + 1);
  const rightTabs = openKeys.slice(index + 1);

  // 过滤出右侧非固定的标签页并关闭
  const removedTabs = rightTabs.filter(
    (item) => !affixRouter.includes(item.key) && !item.affix
  );
  removedTabs.forEach((item) => {
    drop(item.key?.split("?")[0] || "");
  });

  // 保留左侧所有标签页和右侧固定的标签页
  const keptRightTabs = rightTabs.filter(
    (item) => affixRouter.includes(item.key) || item.affix
  );
  return [...leftTabs, ...keptRightTabs];
};

/**
 * 关闭其他标签页（保留固定标签页）
 */
export const closeOtherTabs = (
  openKeys: TabType[],
  currentItem: TabType,
  drop: (key: string) => void,
  affixRouter: string[] = []
): TabType[] => {
  // 过滤出其他非固定的标签页并关闭
  const removedTabs = openKeys.filter(
    (item) =>
      item.key !== currentItem.key &&
      !affixRouter.includes(item.key) &&
      !item.affix
  );
  removedTabs.forEach((item) => {
    drop(item.key?.split("?")[0] || "");
  });

  // 保留当前标签页和所有固定的标签页
  return openKeys.filter(
    (item) =>
      item.key === currentItem.key ||
      affixRouter.includes(item.key) ||
      item.affix
  );
};

/**
 * 关闭全部标签页（保留固定标签页）
 */
export const closeAllTabs = (
  openKeys: TabType[],
  drop: (key: string) => void,
  affixRouter: string[] = []
): TabType[] => {
  // 过滤出非固定的标签页并关闭
  const removedTabs = openKeys.filter(
    (item) => !affixRouter.includes(item.key) && !item.affix
  );
  removedTabs.forEach((item) => {
    drop(item.key?.split("?")[0] || "");
  });

  // 返回固定的标签页
  return openKeys.filter(
    (item) => affixRouter.includes(item.key) || item.affix
  );
};
