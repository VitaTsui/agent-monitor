import React, { useCallback, useEffect, useRef, useState } from "react";

import classNames from "classnames";

import styles from "./index.module.scss";

/**
 * 分栏比例存这个键：换一个会话、刷新一次仍是上次拖到的位置。
 * 与 VitaAgent 的 `vita.viewerPane.ratio` 同一套语义，键名带本项目前缀。
 */
const RATIO_KEY = "am.portal.rightPane.ratio";
const MIN_RATIO = 0.3;
const MAX_RATIO = 0.7;
/**
 * 默认取下限而不是一半。
 *
 * VitaAgent 那边 50% 是**产物预览**那一栏的默认值（要看文件，越宽越好）；
 * 本项目右栏装的是「会话状态」，对应的是它另一种形态 —— 固定 320 宽的任务面板。
 * 30% 在 1440 下约 345px，是这块内容真正需要的宽度；给到一半只会让正文白挨一刀。
 */
const DEFAULT_RATIO = MIN_RATIO;

const clamp = (v: number) => Math.min(MAX_RATIO, Math.max(MIN_RATIO, v));

const readRatio = () => {
  try {
    const v = Number(localStorage.getItem(RATIO_KEY));
    return v >= MIN_RATIO && v <= MAX_RATIO ? v : DEFAULT_RATIO;
  } catch {
    return DEFAULT_RATIO;
  }
};

const writeRatio = (v: number) => {
  try {
    localStorage.setItem(RATIO_KEY, String(v));
  } catch {
    // 隐私模式下写不进去也无妨，下次回到默认宽度
  }
};

interface RightPaneProps {
  className?: string;
  children: React.ReactNode;
}

/**
 * 右栏的容器：8px 的拖拽把手 ＋ 一块与正文并排的栏。
 *
 * 照 VitaAgent 的 `ViewerPane` 做的（`web/src/pages/chat/_components/ViewerPane/`）：
 * **不是抽屉、不是浮层，是与正文列并排、可拖的分栏** —— 无遮罩、正文照常可操作。
 *
 * 比例靠 `flex-grow` 实现：正文列的 `flex: 1` 不动，本栏的 grow 取 `r / (1 − r)`，
 * 两者一比正好是 r。这样不必去改正文列的样式。
 *
 * 必须放在一个 `display: flex` 的行里、紧跟正文列之后（见 Portal 的 `.contentRow`）。
 */
const RightPane: React.FC<RightPaneProps> = (props) => {
  const { className, children } = props;
  const [ratio, setRatio] = useState(readRatio);
  const [dragging, setDragging] = useState(false);
  const sepRef = useRef<HTMLDivElement>(null);
  const latest = useRef(ratio);
  latest.current = ratio;

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
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
    const move = (ev: PointerEvent) => {
      // 栏在右边：把手离行右缘的距离占整行的比例，就是本栏的份额
      const r = clamp((rect.right - ev.clientX) / rect.width);
      latest.current = r;
      setRatio(r);
    };
    const up = () => {
      sep.removeEventListener("pointermove", move);
      sep.removeEventListener("pointerup", up);
      sep.removeEventListener("pointercancel", up);
      setDragging(false);
      writeRatio(latest.current);
    };
    sep.addEventListener("pointermove", move);
    sep.addEventListener("pointerup", up);
    sep.addEventListener("pointercancel", up);
  }, []);

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
        aria-valuemin={MIN_RATIO * 100}
        aria-valuemax={MAX_RATIO * 100}
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
        style={{ flexGrow: ratio / (1 - ratio) }}
      >
        {children}
      </aside>
    </>
  );
};

export default RightPane;
