import React, { useMemo } from "react";

import { highlightLines } from "./highlight";
import styles from "./index.module.scss";

interface CodeLinesProps {
  text: string;
  lang: string;
}

/**
 * 源码态：**行号 ＋ 逐行高亮**。
 *
 * 不用 `<pre>`：一行就是一个 flex 行（左边 40px 的行号列、右边代码），代码列
 * `pre-wrap; break-all` —— **长行折行，不横滚**。围栏 `<pre>` 那套要么横滚、
 * 要么把行号与内容折错位。形制照 VitaAgent `components/FileViewer/CodeLines.tsx`。
 *
 * 大文件按行渲染的代价是实打实的，所以调用方先夹过行数（见 `MAX_LINES`）。
 */
const CodeLines: React.FC<CodeLinesProps> = ({ text, lang }) => {
  const lines = useMemo(() => highlightLines(text, lang), [text, lang]);
  return (
    <div className={styles.code}>
      {lines.map((html, i) => (
        <div key={i} className={styles.line}>
          <span className={styles.lineNo}>{i + 1}</span>
          {/* highlight.js 的输出里只有带 class 的 span 与已转义的文本 */}
          <span
            className={styles.lineCode}
            dangerouslySetInnerHTML={{ __html: html || " " }}
          />
        </div>
      ))}
    </div>
  );
};

export default CodeLines;
