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
  /** 撤回「已入终端队列」的输入（注入 ↑ 到终端） */
  onRecallDelivered?: () => void;
  /** 回应终端里的交互式选择（点选项 = 发送对应序号到终端） */
  onAnswer?: (text: string) => void;
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

/**
 * 交互式选择卡（AskUserQuestion）。除了同步预设选项，补上一条「自行输入」——
 * AskUserQuestion 始终隐含一个「其它/自定义」项，终端里能自己敲答案，远端也要能。
 * 输入即走 onAnswer（等价在对话框里发一行自定义答案）。
 */
const SelectCard: React.FC<{
  content: string;
  onAnswer?: (text: string) => void;
}> = ({ content, onAnswer }) => {
  const [custom, setCustom] = useState("");
  let data: {
    questions?: {
      question?: string;
      header?: string;
      options?: { label?: string; description?: string }[];
    }[];
  } = {};
  try {
    data = JSON.parse(content);
  } catch {
    /* 半截 JSON：忽略，按空卡片处理 */
  }

  const submitCustom = () => {
    const t = custom.trim();
    if (t && onAnswer) {
      onAnswer(t);
      setCustom("");
    }
  };

  return (
    <div className={styles.selectCard}>
      <div className={styles.selectHead}>⌨︎ 终端等待选择</div>
      {(data.questions ?? []).map((q, qi) => (
        <div key={qi} className={styles.selectQ}>
          {q.question ? (
            <div className={styles.selectQuestion}>{q.question}</div>
          ) : null}
          <div className={styles.selectOpts}>
            {(q.options ?? []).map((o, oi) => (
              <div
                key={oi}
                className={`${styles.selectOpt} ${onAnswer ? styles.clickable : ""}`}
                role={onAnswer ? "button" : undefined}
                tabIndex={onAnswer ? 0 : undefined}
                onClick={onAnswer ? () => onAnswer(String(oi + 1)) : undefined}
                onKeyDown={
                  onAnswer
                    ? (e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          onAnswer(String(oi + 1));
                        }
                      }
                    : undefined
                }
              >
                <span className={styles.selectOptIdx}>{oi + 1}</span>
                <span className={styles.selectOptBody}>
                  <span className={styles.selectOptLabel}>{o.label}</span>
                  {o.description ? (
                    <span className={styles.selectOptDesc}>{o.description}</span>
                  ) : null}
                </span>
              </div>
            ))}
            {/* 自行输入：AskUserQuestion 隐含的「其它」，输入后回车 / 点发送提交 */}
            <div className={`${styles.selectOpt} ${styles.selectOptCustom}`}>
              <span className={styles.selectOptIdx}>✎</span>
              <span className={styles.selectOptBody}>
                <input
                  className={styles.selectCustomInput}
                  placeholder="自行输入答案…"
                  value={custom}
                  disabled={!onAnswer}
                  onChange={(e) => setCustom(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      submitCustom();
                    }
                  }}
                />
                <span
                  className={`${styles.selectCustomSend} ${
                    onAnswer && custom.trim() ? styles.clickable : styles.disabled
                  }`}
                  role="button"
                  tabIndex={0}
                  onClick={submitCustom}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      submitCustom();
                    }
                  }}
                >
                  发送
                </span>
              </span>
            </div>
          </div>
        </div>
      ))}
      <div className={styles.selectHint}>
        点选项直接回应；或在「✎ 自行输入」里敲自定义答案后回车
      </div>
    </div>
  );
};

/** 工具结果超过该行数时折叠 */
const RESULT_CLAMP_LINES = 4;

const TerminalFeed: React.FC<TerminalFeedProps> = (props) => {
  const { messages, running, providerDsr, onRecall, onRecallDelivered, onAnswer } = props;
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const turns = toTurns(messages);

  const toggleExpand = (key: string) => {
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  const renderItem = (m: PortalMessage, key: string) => {
    // 交互式选择/权限确认（AskUserQuestion）：同步问题与选项，成卡片展示。
    // 终端里需要用户在 TUI 里选；这里让远程也能看到「在等你选什么」并能回应。
    if (m.role === "select") {
      return <SelectCard key={key} content={m.content} onAnswer={onAnswer} />;
    }
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
      {(() => {
        // 「最后一个有实际活动的轮次」：本地回显（发送排队）新开的轮次还没有
        // 任何助手/工具消息，执行中的收敛逻辑必须仍作用于它前面真正在跑的
        // 那一轮 —— 否则一发送，跑着的轮次就不再是最后一轮，整段 Bash 工具
        // 流水会提前铺出来。
        let lastActive = turns.length - 1;
        while (
          lastActive > 0 &&
          turns[lastActive].items.length === 0 &&
          turns[lastActive].user?.local
        ) {
          lastActive--;
        }
        return turns.map((turn, ti) => {
        // 执行中的活动轮：不铺工具流水，只同步 Q&A（助手文本），
        // 工具过程收进折叠块，需要时再展开。
        const inProgress = !!running && ti === lastActive;
        // 用内容指纹做 key：执行中 → 完成态切换时 key 不变，避免整块重挂载闪烁；
        // 且不随消息裁剪而漂移（下标会）。
        const keyed = turn.items.map((m) => ({ m, k: msgKey(m) }));
        // 执行中只铺 Q&A，不铺工具流水（工具过程等回合结束后一次性完整呈现）；
        // 方案是待批准的计划而非过程噪音，需随时同步显示。
        // （清单与后台任务是「当前状态」，已由 ChatPane 抽出去单独成面板。）
        const visibleItems = inProgress
          ? keyed.filter(({ m }) => ["assistant", "plan", "select"].includes(m.role))
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
                    turn.user.local && (turn.user.queued || turn.user.delivered)
                      ? styles.queued
                      : ""
                  }`}
                >
                  {turn.user.content}
                </div>
                {turn.user.local && (turn.user.queued || turn.user.delivered) ? (
                  <div className={styles.queuedRow}>
                    <span className={styles.queuedTag}>
                      <span className={styles.queuedDot} />
                      {turn.user.queued ? "排队中" : "已入终端队列 · 等待执行"}
                    </span>
                    {turn.user.queued && onRecall && turn.user.cmdId ? (
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
                    ) : turn.user.delivered && onRecallDelivered ? (
                      <span
                        className={styles.recallBtn}
                        role="button"
                        tabIndex={0}
                        onClick={onRecallDelivered}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.preventDefault();
                            onRecallDelivered();
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
                  {(() => {
                    // 工具流水（tool/tool_result）折叠成摘要行，默认收起 ——
                    // 连续的一段工具消息归成一组，助手文本/方案保持原位展开。
                    const out: React.ReactNode[] = [];
                    let group: { m: PortalMessage; k: string }[] = [];
                    const flush = () => {
                      if (!group.length) {
                        return;
                      }
                      const gkey = `tg-${group[0].k}`;
                      const openG = !!expanded[gkey];
                      const steps = group.filter((x) => x.m.role === "tool").length;
                      out.push(
                        <div key={gkey} className={styles.toolGroup}>
                          <span
                            className={styles.toolGroupHead}
                            role="button"
                            tabIndex={0}
                            aria-expanded={openG}
                            onClick={() => toggleExpand(gkey)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                toggleExpand(gkey);
                              }
                            }}
                          >
                            <span className={styles.toolGroupArrow}>
                              {openG ? "▾" : "▸"}
                            </span>
                            执行过程 · {steps || group.length} 步
                          </span>
                          {openG ? (
                            <div className={styles.toolGroupBody}>
                              {group.map(({ m, k }) => renderItem(m, k))}
                            </div>
                          ) : null}
                        </div>,
                      );
                      group = [];
                    };
                    visibleItems.forEach(({ m, k }) => {
                      if (m.role === "tool" || m.role === "tool_result") {
                        group.push({ m, k });
                      } else {
                        flush();
                        out.push(renderItem(m, k));
                      }
                    });
                    flush();
                    return out;
                  })()}
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
        });
      })()}
    </div>
  );
};

export default TerminalFeed;
