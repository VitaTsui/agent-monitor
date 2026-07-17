import React, { useState } from "react";

import dayjs from "dayjs";

import { PortalMessage } from "@/services/apis/portal";
import styles from "./index.module.scss";

interface TerminalFeedProps {
  messages: PortalMessage[];
  /** 会话是否执行中（末尾显示工作指示） */
  running?: boolean;
}

/** 一轮对话：一条用户消息 + 其后的助手/工具活动 */
interface Turn {
  user: PortalMessage | null;
  items: PortalMessage[];
}

function toTurns(messages: PortalMessage[]): Turn[] {
  const turns: Turn[] = [];
  let cur: Turn | null = null;
  messages.forEach((m) => {
    if (m.role === "user") {
      cur = { user: m, items: [] };
      turns.push(cur);
      return;
    }
    if (!cur) {
      cur = { user: null, items: [] };
      turns.push(cur);
    }
    cur.items.push(m);
  });
  return turns;
}

const fmtTime = (ts?: string) => (ts ? dayjs(ts).format("MM-DD HH:mm") : "");

/** 工具结果超过该行数时折叠 */
const RESULT_CLAMP_LINES = 4;

const TerminalFeed: React.FC<TerminalFeedProps> = (props) => {
  const { messages, running } = props;
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const turns = toTurns(messages);

  const toggleExpand = (key: string) => {
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  const renderItem = (m: PortalMessage, key: string) => {
    if (m.role === "tool") {
      return (
        <div key={key} className={styles.toolLine}>
          <span className={styles.toolDot}>⏺</span>
          <span className={styles.toolText}>{m.content}</span>
        </div>
      );
    }

    if (m.role === "tool_result") {
      const lines = m.content.split("\n");
      const clamped = lines.length > RESULT_CLAMP_LINES && !expanded[key];
      const shown = clamped
        ? lines.slice(0, RESULT_CLAMP_LINES).join("\n")
        : m.content;
      return (
        <div key={key} className={styles.resultLine}>
          <span className={styles.resultElbow}>⎿</span>
          <div className={styles.resultBody}>
            <pre className={styles.resultText}>{shown}</pre>
            {lines.length > RESULT_CLAMP_LINES && (
              <span
                className={styles.expandBtn}
                onClick={() => toggleExpand(key)}
              >
                {clamped ? `展开其余 ${lines.length - RESULT_CLAMP_LINES} 行` : "收起"}
              </span>
            )}
          </div>
        </div>
      );
    }

    // assistant 文本
    return (
      <div key={key} className={styles.assistantLine}>
        <span className={styles.assistantDot}>⏺</span>
        <div className={styles.assistantText}>{m.content}</div>
      </div>
    );
  };

  return (
    <div className={styles.TerminalFeed}>
      {turns.map((turn, ti) => {
        // 执行中的最后一轮：不铺工具流水，只同步 Q&A（助手文本），
        // 工具过程等回合结束后一次性完整呈现。
        const inProgress = !!running && ti === turns.length - 1;
        // 带上原始下标做 key：执行中 → 完成态切换时 key 不变，避免整块重挂载闪烁
        const indexed = turn.items.map((m, oi) => ({ m, oi }));
        const visibleItems = inProgress
          ? indexed.filter(({ m }) => m.role === "assistant")
          : indexed;
        // 执行中时给一条「最近动作」预览（最后一条工具调用）
        const lastTool = inProgress
          ? [...turn.items].reverse().find((m) => m.role === "tool")
          : undefined;

        return (
          <div key={ti} className={styles.turn}>
            {turn.user ? (
              <div className={styles.userRow}>
                <div className={styles.userBubble}>{turn.user.content}</div>
                <div className={styles.userTime}>{fmtTime(turn.user.timestamp)}</div>
              </div>
            ) : null}

            {(visibleItems.length > 0 || inProgress) && (
              <div className={styles.terminal}>
                <div className={styles.termHeader}>
                  <span className={styles.termDots}>
                    <i />
                    <i />
                    <i />
                  </span>
                  <span className={styles.termTitle}>Claude Code</span>
                  <span className={styles.termTime}>
                    {fmtTime(
                      turn.items[turn.items.length - 1]?.timestamp ??
                        turn.user?.timestamp
                    )}
                  </span>
                </div>
                <div className={styles.termBody}>
                  {visibleItems.map(({ m, oi }) => renderItem(m, `${ti}-${oi}`))}
                  {inProgress ? (
                    <div className={styles.working}>
                      <span className={styles.workingStar}>✳</span>
                      <span className={styles.workingText}>
                        执行中…
                        {lastTool ? (
                          <span className={styles.workingAction}>
                            {lastTool.content}
                          </span>
                        ) : null}
                      </span>
                    </div>
                  ) : null}
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};

export default TerminalFeed;
