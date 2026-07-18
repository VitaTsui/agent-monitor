import React, { useEffect, useRef, useState } from "react";

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
 * 过长文本自动滚动（marquee）：选中项且内容溢出容器时，循环滚动展示全文；
 * 其余情况维持省略号截断。滚动用两份内容首尾相接 + translateX(-50%)，
 * 动画时长按文本长度缩放，长名字不至于快得看不清。
 */
const ScrollText: React.FC<ScrollTextProps> = (props) => {
  const { text, plain, active, className } = props;
  const outerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(false);

  useEffect(() => {
    const outer = outerRef.current;
    const measure = measureRef.current;
    if (!outer || !measure) {
      return;
    }
    // 用隐藏测量节点比对：静态态的省略号截断不影响 scrollWidth 判定
    setOverflow(measure.scrollWidth > outer.clientWidth + 1);
  }, [text, plain, active]);

  const scrolling = !!active && overflow;
  // 8s 起步，每 10 个字符加 1s，滚动速度随长度平缓
  const duration = `${Math.max(8, Math.ceil((plain?.length ?? 20) / 10) + 6)}s`;

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
        <div className={styles.track} style={{ animationDuration: duration }}>
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
