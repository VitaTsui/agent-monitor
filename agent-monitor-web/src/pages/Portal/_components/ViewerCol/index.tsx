import React, { useCallback, useEffect, useRef, useState } from "react";

import styles from "./index.module.scss";

/** 宽度占本格的比例。夹在这两个数之间 —— 照参照（30%~70%） */
const MIN_RATIO = 0.3;
const MAX_RATIO = 0.7;
const DEFAULT_RATIO = 0.42;
const KEY = "am.portal.viewer.ratio";

const readRatio = (): number => {
  try {
    const raw = Number(localStorage.getItem(KEY));
    if (Number.isFinite(raw) && raw >= MIN_RATIO && raw <= MAX_RATIO) {
      return raw;
    }
  } catch {
    // 隐私模式下读不到就用默认值
  }
  return DEFAULT_RATIO;
};

interface ViewerColProps {
  children: React.ReactNode;
}

/**
 * 文件查看器那一列：**与正文并排、宽度可拖**。
 *
 * 形制照 VitaAgent 的 `ViewerPane`
 * （`web/src/pages/chat/_components/ViewerPane/index.tsx`）：一条 8px 的把手
 * （`col-resize`、中间一根 3×48 的小条、悬停才显）＋ 一列浮着的卡，比例夹在
 * 30%~70%、存 localStorage。**不是抽屉、不是浮层**：无遮罩，正文照常可操作。
 *
 * **为什么这一列可拖、而右栏（`RightPane`）固定 320 不可拖**：
 * 右栏装的是一张状态清单（几条任务、几个子代理），320 够用、再宽也不会多出信息，
 * 用户当初明确要求过「不要调整宽度的功能」；这一列装的是任意宽度的源码与图片，
 * 宽度直接决定「一行代码折在哪、图看不看得清」——那是内容本身的诉求。
 * 参照两者也正是这么分的：`ActivityPanel` 定宽 320、`ViewerPane` 可拖。
 *
 * 卡的形制（8px 露底 ＋ 圆角 8 ＋ 描边环 ＋ 影）与右栏那张**完全一致**，
 * 一屏之内不出现两种「浮起来的面板」。
 */
const ViewerCol: React.FC<ViewerColProps> = ({ children }) => {
  const [ratio, setRatio] = useState(readRatio);
  const rootRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef(false);

  const onMove = useCallback((e: PointerEvent) => {
    if (!dragRef.current) {
      return;
    }
    const row = rootRef.current?.parentElement;
    if (!row) {
      return;
    }
    const box = row.getBoundingClientRect();
    if (!box.width) {
      return;
    }
    // 把手在这一列的左边缘：指针离右边缘多远，这一列就有多宽
    const next = (box.right - e.clientX) / box.width;
    setRatio(Math.min(MAX_RATIO, Math.max(MIN_RATIO, next)));
  }, []);

  const onUp = useCallback(() => {
    if (!dragRef.current) {
      return;
    }
    dragRef.current = false;
    document.body.style.removeProperty("cursor");
    document.body.style.removeProperty("user-select");
  }, []);

  useEffect(() => {
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, [onMove, onUp]);

  // 落盘拖完的比例。**不落每一帧**：拖动中每帧写一次 localStorage 是同步 IO
  useEffect(() => {
    const t = window.setTimeout(() => {
      try {
        localStorage.setItem(KEY, String(ratio));
      } catch {
        // 写不进去也无妨，下次回到默认
      }
    }, 300);
    return () => window.clearTimeout(t);
  }, [ratio]);

  return (
    <div
      ref={rootRef}
      className={styles.ViewerCol}
      style={{ flexBasis: `${Math.round(ratio * 100)}%` }}
    >
      <div
        className={styles.handle}
        role="separator"
        aria-label="拖动调整文件查看器的宽度"
        onPointerDown={(e) => {
          dragRef.current = true;
          document.body.style.cursor = "col-resize";
          document.body.style.userSelect = "none";
          e.preventDefault();
        }}
      >
        <span className={styles.grip} />
      </div>
      <div className={styles.card}>{children}</div>
    </div>
  );
};

export default ViewerCol;
