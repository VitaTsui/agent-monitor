import React, { useCallback, useEffect, useRef, useState } from "react";

import classNames from "classnames";
import { observer } from "mobx-react-lite";

import PortalStore, {
  RIGHT_PANE_MAX_RATIO,
  RIGHT_PANE_MIN_RATIO,
} from "../../PortalStore";
import styles from "./index.module.scss";

interface RightPaneProps {
  /**
   * 这一栏属于哪一格。
   *
   * **不是可选项**：开合与宽度都按会话 id 各记一份（见 PortalStore 的
   * `_rightPaneById`）。从前这里没有 taskId —— 一条全局比例、一个全局布尔，
   * 拆成 2~4 格后任何一格的开关都在拨同一个值，那是这次要推翻的旧设计。
   */
  taskId: string;
  className?: string;
  children: React.ReactNode;
}

/**
 * 一格内的右栏：8px 的拖拽把手 ＋ 一块浮在格子上的卡。
 *
 * 照 VitaAgent 的 `ViewerPane` 做的（`web/src/pages/chat/_components/ViewerPane/`）：
 * **不是抽屉、不是浮层，是与本格正文并排、可拖的分栏** —— 无遮罩、正文照常可操作。
 * 卡四周留 8 露底、圆角 10、一圈 1px 描边环加两层轻投影，靠「浮起来」与正文区分，
 * 而不是拿一条竖线把格子切两半。
 *
 * 宽度只由 `flex-basis` 一处说了算：`ratio × 本格宽 − 8`（8 = 把手 `.sep` 的宽）。
 * 右边那 8 的露底现在是这一栏**自己的 padding**（border-box，算在 basis 里），
 * 不再是外挂的 margin —— 所以要减掉的只剩把手那一份。grow/shrink 都是 0，
 * 两态之间是一次真正的宽度过渡。
 *
 * 必须放在一个 `display: flex` 的行里、紧跟本格正文之后（见 ChatPane 的 `.paneRow`）。
 */
const RightPane: React.FC<RightPaneProps> = observer((props) => {
  const { taskId, className, children } = props;
  const ratio = PortalStore.rightPaneRatio(taskId);
  const [dragging, setDragging] = useState(false);
  const sepRef = useRef<HTMLDivElement>(null);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) {
        return;
      }
      e.preventDefault();
      const sep = sepRef.current;
      const row = sep?.parentElement;
      if (!sep || !row) {
        return;
      }
      sep.setPointerCapture(e.pointerId);
      setDragging(true);
      const rect = row.getBoundingClientRect();
      let latest = PortalStore.rightPaneRatio(taskId);
      const move = (ev: PointerEvent) => {
        // 栏在右边：把手离本格右缘的距离占本格宽的比例，就是这一栏的份额
        latest = (rect.right - ev.clientX) / rect.width;
        PortalStore.setRightPaneRatio(taskId, latest);
      };
      const up = () => {
        sep.removeEventListener("pointermove", move);
        sep.removeEventListener("pointerup", up);
        sep.removeEventListener("pointercancel", up);
        setDragging(false);
        // 松手才落盘：拖动中每帧写一次 localStorage 会发涩
        PortalStore.setRightPaneRatio(taskId, latest, true);
      };
      sep.addEventListener("pointermove", move);
      sep.addEventListener("pointerup", up);
      sep.addEventListener("pointercancel", up);
    },
    [taskId],
  );

  // 拖的过程中全页禁选：不然把手划过正文会把文字一路选中
  useEffect(() => {
    if (!dragging) {
      return;
    }
    const prevSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";
    return () => {
      document.body.style.userSelect = prevSelect;
      document.body.style.cursor = "";
    };
  }, [dragging]);

  return (
    <>
      <div
        ref={sepRef}
        className={styles.sep}
        role="separator"
        aria-orientation="vertical"
        aria-label="调整右栏宽度"
        aria-valuemin={RIGHT_PANE_MIN_RATIO * 100}
        aria-valuemax={RIGHT_PANE_MAX_RATIO * 100}
        aria-valuenow={Math.round(ratio * 100)}
        onPointerDown={onPointerDown}
      >
        <span className={styles.handle} />
      </div>
      <aside
        className={classNames(
          styles.RightPane,
          { [styles.dragging]: dragging },
          className,
        )}
        style={{ flexBasis: `calc(${ratio * 100}% - 8px)` }}
      >
        {children}
      </aside>
    </>
  );
});

export default RightPane;
