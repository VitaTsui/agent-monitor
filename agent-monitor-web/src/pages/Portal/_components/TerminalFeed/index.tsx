import React, { useState } from "react";

import dayjs from "dayjs";
import { Markdown } from "@hsu-react/ui";

import { PortalMessage } from "@/services/apis/portal";
import styles from "./index.module.scss";

interface TerminalFeedProps {
  messages: PortalMessage[];
  /** 会话是否执行中（末尾显示工作指示） */
  running?: boolean;
  /** 终端卡标题：来源代理名（Claude Code / Codex / Gemini CLI …） */
  providerDsr?: string;
  /** 撤回仍在排队的输入（排队气泡上的撤回按钮） */
  onRecall?: (cmdId: string) => void;
}

/** 一轮对话：一条用户消息 + 其后的助手/工具活动 */
interface Turn {
  key: string;
  user: PortalMessage | null;
  items: PortalMessage[];
}

/**
 * 消息的稳定标识（与 PortalStore 合并去重同源）。
 * 不能用数组下标：单会话累积到上限后会从头部丢弃老消息（见 MAX_MESSAGES_PER_TASK），
 * 一旦开始丢弃，所有幸存消息的下标整体前移，按下标记的展开状态会错位到别的消息上。
 */
const msgKey = (m: PortalMessage) =>
  `${m.timestamp}|${m.role}|${m.content.length}|${m.content.slice(0, 60)}`;

function toTurns(messages: PortalMessage[]): Turn[] {
  const turns: Turn[] = [];
  let cur: Turn | null = null;
  messages.forEach((m) => {
    if (m.role === "user") {
      cur = { key: msgKey(m), user: m, items: [] };
      turns.push(cur);
      return;
    }
    if (!cur) {
      cur = { key: `orphan|${msgKey(m)}`, user: null, items: [] };
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
  const { messages, running, providerDsr, onRecall } = props;
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const turns = toTurns(messages);

  const toggleExpand = (key: string) => {
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  const renderItem = (m: PortalMessage, key: string) => {
    // plan 模式给出的待批准方案：正文是 markdown，单独成卡片
    if (m.role === "plan") {
      return (
        <div key={key} className={styles.planCard}>
          <div className={styles.planHead}>方案</div>
          <div className={styles.planBody}>
            <Markdown.Views>{m.content}</Markdown.Views>
          </div>
        </div>
      );
    }

    if (m.role === "tool") {
      return (
        <div key={key} className={styles.toolLine}>
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
                role="button"
                tabIndex={0}
                aria-expanded={!clamped}
                onClick={() => toggleExpand(key)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    toggleExpand(key);
                  }
                }}
              >
                {clamped ? `展开其余 ${lines.length - RESULT_CLAMP_LINES} 行` : "收起"}
              </span>
            )}
          </div>
        </div>
      );
    }

    // assistant 文本：按 Markdown 渲染（同步内容常带 **加粗**/代码块/列表）
    return (
      <div key={key} className={styles.assistantLine}>
        <div className={styles.assistantText}>
          <Markdown.Views>{m.content}</Markdown.Views>
        </div>
      </div>
    );
  };

  return (
    <div className={styles.TerminalFeed}>
      {turns.map((turn, ti) => {
        // 执行中的最后一轮：不铺工具流水，只同步 Q&A（助手文本），
        // 工具过程等回合结束后一次性完整呈现。
        const inProgress = !!running && ti === turns.length - 1;
        // 用内容指纹做 key：执行中 → 完成态切换时 key 不变，避免整块重挂载闪烁；
        // 且不随消息裁剪而漂移（下标会）。
        const keyed = turn.items.map((m) => ({ m, k: msgKey(m) }));
        // 执行中只铺 Q&A，不铺工具流水（工具过程等回合结束后一次性完整呈现）；
        // 方案是待批准的计划而非过程噪音，需随时同步显示。
        // （清单与后台任务是「当前状态」，已由 ChatPane 抽出去单独成面板。）
        const visibleItems = inProgress
          ? keyed.filter(({ m }) => ["assistant", "plan"].includes(m.role))
          : keyed;
        // 执行中时给一条「最近动作」预览（最后一条工具调用）
        const lastTool = inProgress
          ? [...turn.items].reverse().find((m) => m.role === "tool")
          : undefined;

        return (
          <div key={turn.key} className={styles.turn}>
            {turn.user ? (
              <div className={styles.userRow}>
                <div
                  className={`${styles.userBubble} ${
                    turn.user.local && turn.user.queued ? styles.queued : ""
                  }`}
                >
                  {turn.user.content}
                </div>
                {turn.user.local && turn.user.queued ? (
                  <div className={styles.queuedRow}>
                    <span className={styles.queuedTag}>
                      <span className={styles.queuedDot} />
                      排队中
                    </span>
                    {onRecall && turn.user.cmdId ? (
                      <span
                        className={styles.recallBtn}
                        role="button"
                        tabIndex={0}
                        onClick={() => onRecall(turn.user!.cmdId!)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.preventDefault();
                            onRecall(turn.user!.cmdId!);
                          }
                        }}
                      >
                        撤回
                      </span>
                    ) : null}
                  </div>
                ) : (
                  <div className={styles.userTime}>{fmtTime(turn.user.timestamp)}</div>
                )}
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
                  <span className={styles.termTitle}>{providerDsr || "终端"}</span>
                  <span className={styles.termTime}>
                    {fmtTime(
                      turn.items[turn.items.length - 1]?.timestamp ??
                        turn.user?.timestamp
                    )}
                  </span>
                </div>
                <div className={styles.termBody}>
                  {visibleItems.map(({ m, k }) => renderItem(m, k))}
                  {inProgress ? (
                    <div className={styles.working}>
                      {/* 只有正在执行的条目带（会动的）圆点，其余内容与终端一致不加点 */}
                      <span className={styles.workingDot} />
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
