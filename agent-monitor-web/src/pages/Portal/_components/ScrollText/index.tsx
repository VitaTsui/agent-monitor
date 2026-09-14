import React, { useEffect, useRef, useState } from "react";

import useReducedMotion from "@/hooks/useReducedMotion";
import styles from "./index.module.scss";

interface ScrollTextProps {
  text: React.ReactNode;
  /** 纯文本（测量与 title 提示用；text 含图标等节点时必传） */
  plain?: string;
  /** 仅激活（选中）且溢出时才滚动；未激活时按省略号截断 */
  active?: boolean;
  className?: string;
}

/**
 * 一屏跑马灯要走多快（**像素 / 秒**）。
 *
 * 从前定的是**时长**不是速度（`max(8, ceil(字数/10) + 6)` 秒跑完一整圈），
 * 而一圈的距离是文字宽度 —— 于是**标题越长滚得越快**：实测 20 字的标题约
 * 40 px/s，60 字的约 73 px/s，差了将近一倍。恰恰是最需要看清的长标题飞得最快
 * （用户原话：「会话名称的滚动有点太快了」）。
 *
 * 30 px/s 的依据：侧栏正文 14px，一个汉字约占 14px 宽，30 px/s ≈ 每秒 2.1 个字。
 * 跑马灯比静止文本难读（视线要跟着走、读错了没法回看），所以要显著慢于常速
 * 默读；再慢就成了看着不动。
 */
const SPEED_PX_PER_SEC = 30;

/**
 * 开跑前先停一下，让人把开头读进去。
 * 只影响第一圈（CSS 的 `animation-delay` 对 infinite 动画只延迟起步，不是每圈都等）。
 */
const START_DELAY = "1.2s";

/** 两份内容之间的间隔，与 scss 里 `.copy` 的 padding-right 必须同值 */
const COPY_GAP = 40;

/**
 * 过长文本自动滚动（marquee）：选中项且内容溢出容器时，循环滚动展示全文；
 * 其余情况维持省略号截断。滚动用两份内容首尾相接 + translateX(-50%)。
 *
 * **按速度定时长，不按字数定时长**（理由见 `SPEED_PX_PER_SEC`）：一圈要走的距离
 * 是「文字宽度 ＋ 两份之间的间隔」，除以速度就是时长。长短标题因此一样快。
 */
const ScrollText: React.FC<ScrollTextProps> = (props) => {
  const { text, plain, active, className } = props;
  const outerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  /** 文字的实际宽度（px）。0 = 还没量到 / 没溢出 */
  const [textW, setTextW] = useState(0);
  /* 系统开了「减少动态效果」就**不滚**，退回省略号截断 —— 前庭功能敏感的人
     看一行自己动的字会难受，而这一行的内容 `title` 里本来就有全文。
     判断落在 `hooks/useReducedMotion`（JS 侧唯一一份，ChatPane 的平滑滚动同源）。 */
  const reduced = useReducedMotion();

  useEffect(() => {
    const outer = outerRef.current;
    const measure = measureRef.current;
    if (!outer || !measure) {
      return;
    }
    // 用隐藏测量节点比对：静态态的省略号截断不影响 scrollWidth 判定。
    // **量的是宽度不是字数**：中英混排、数字、图标各占多少像素，只有量了才知道。
    const w = measure.scrollWidth;
    setTextW(w > outer.clientWidth + 1 ? w : 0);
  }, [text, plain, active]);

  const scrolling = !!active && textW > 0 && !reduced;
  /* 一圈走的距离 = 文字宽 ＋ 两份之间的间隔（`translateX(-50%)` 正好走这么多），
     除以速度得时长。长标题不再比短标题快。 */
  const duration = `${((textW + COPY_GAP) / SPEED_PX_PER_SEC).toFixed(2)}s`;

  return (
    <div
      ref={outerRef}
      className={`${styles.ScrollText} ${className ?? ""}`}
      title={plain}
    >
      <span ref={measureRef} className={styles.measure} aria-hidden>
        {text}
      </span>
      {scrolling ? (
        <div
          className={styles.track}
          style={{ animationDuration: duration, animationDelay: START_DELAY }}
        >
          <span className={styles.copy}>{text}</span>
          <span className={styles.copy} aria-hidden>
            {text}
          </span>
        </div>
      ) : (
        <span className={styles.static}>{text}</span>
      )}
    </div>
  );
};

export default ScrollText;
