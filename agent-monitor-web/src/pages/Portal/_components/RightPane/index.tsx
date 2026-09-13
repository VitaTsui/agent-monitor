import React from "react";

import classNames from "classnames";

import styles from "./index.module.scss";

interface RightPaneProps {
  className?: string;
  children: React.ReactNode;
}

/**
 * 一格内的右栏：与本格正文并排的**一条固定 320 宽的白列**。
 *
 * 几何照 VitaAgent 的 `ActivityPanel`
 * （`web/src/pages/chat/_components/ActivityPanel/index.module.scss:1-11`）：
 * 320 定宽、`flex-shrink: 0`、内容自己滚。**不是抽屉、不是浮层** ——
 * 无遮罩，正文照常可操作。
 *
 * **宽度不可调。** 上一版有一条 8px 的把手、0.3~0.7 的上下限、按会话 id 存盘的
 * 一份占比；那套设计已经整条撤销，组件里、样式里、store 里、磁盘上都不再有它。
 * 参照本身就是定宽的：这栏装的是一张状态清单，宽一点窄一点都不改变它能说的事，
 * 可调只是把「摆不下」的问题推给用户自己去拖。
 *
 * 必须放在一个 `display: flex` 的行里、紧跟本格正文之后（见 ChatPane 的 `.paneRow`）。
 */
const RightPane: React.FC<RightPaneProps> = (props) => {
  const { className, children } = props;
  return (
    <aside className={classNames(styles.RightPane, className)}>{children}</aside>
  );
};

export default RightPane;
