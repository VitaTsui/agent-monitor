import React, { useState } from "react";

import dayjs from "dayjs";
import SessionMarkdown from "../SessionMarkdown";
import { SessionImageCtx } from "@/utils/sessionImages";

import { PortalMessage, SelectPayload } from "@/services/apis/portal";
import styles from "./index.module.scss";

interface TerminalFeedProps {
  messages: PortalMessage[];
  /** 会话是否执行中（末尾显示工作指示） */
  running?: boolean;
  /** 终端卡标题：来源代理名（Claude Code / Codex / Gemini CLI …） */
  providerDsr?: string;
  /** 会话上下文：内容里的本地图片路径靠它解析（见 utils/sessionImages） */
  imageCtx?: SessionImageCtx;
  // 撤回不在这里：排队状态与撤回统一由输入框上方的排队条负责，
  // 对话流只呈现「我说了什么、它回了什么」。
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
 *
 * **一次只问一题**。AskUserQuestion 可以带多道题，终端里也是逐题问的：答完第一题
 * 才轮到第二题。而答案注入回去就是一行文本（选项序号或自定义内容），本身不带
 * 「这是第几题」的信息 —— 所以只能按终端的节奏一题一题发，顺序即身份。
 *
 * 早先把所有题平铺出来是错的：每题的选项都发同一个序号，终端无从分辨；
 * 更糟的是答完任意一题整张卡就收起，后面的题根本没机会回答。
 */
export const SelectCard: React.FC<{
  data: SelectPayload;
  onAnswer?: (text: string) => void;
  /** 所有题都答完了（父组件据此收起卡片） */
  onDone?: () => void;
}> = ({ data, onAnswer, onDone }) => {
  const [custom, setCustom] = useState("");
  const [step, setStep] = useState(0);
  /** 多选题当前已勾选的选项序号（1 起，升序） */
  const [picked, setPicked] = useState<number[]>([]);
  // 刚提交的那一下：防手机上连点两次。不能只靠 step 变化后的重渲染 ——
  // 两次点击可能落在同一帧里，那时第二下打的已经是**下一题**的同位置选项了。
  const busy = React.useRef(false);
  React.useEffect(() => {
    // 换题时清空勾选，否则上一题的选择会带进下一题
    setPicked([]);
    // 解锁必须**延后**：原先在 step 变化的这一帧立刻置回 false，而一次点击常常产生
    // 两个事件（触屏的 touch + click、误触的双击）。第二个事件赶在解锁之后落下，
    // 打中的已经是下一题同位置的选项 —— 表现就是「还没看清第二题就被替我答了」。
    // 隔一拍再解锁，把这类连发挡在门外；真人看清题目再点，远不止 400ms。
    const t = setTimeout(() => {
      busy.current = false;
    }, 400);
    return () => clearTimeout(t);
  }, [step]);
  // 输入法是否正在组合。除了 nativeEvent.isComposing，这里再自己记一份：
  // 有的输入法在窗口失焦时会先结束组合、再补发一个 Enter，那时 isComposing
  // 已经是 false，只认它就会把没打完的半截内容当答案发出去。
  const composing = React.useRef(false);

  const questions = data.questions ?? [];
  const total = questions.length;
  const q = questions[step];

  const submit = (text: string) => {
    if (busy.current || !onAnswer || !text) return;
    busy.current = true;
    onAnswer(text);
    setCustom("");
    if (step + 1 < total) {
      setStep(step + 1);
    } else {
      onDone?.();
    }
  };

  const submitCustom = () => submit(custom.trim());

  const multi = !!q?.multiSelect;

  const toggle = (n: number) =>
    setPicked((p) =>
      p.includes(n) ? p.filter((x) => x !== n) : [...p, n].sort((a, b) => a - b)
    );

  /**
   * 多选的「Submit」在终端选择卡里的按键编号。
   *
   * 编号排布：N 个选项占 1..N，其后「其它/自定义」占 N+1，「chat about」占 N+2，
   * **Submit 是 N+3**。多选要先逐个勾选、最后落在 Submit 上才提交。
   *
   * 曾经按 N+2 算，那时选项后只有「其它」一项；终端后来在它之后又加了「chat about」，
   * 于是原来的 N+2 正好落在 chat about 上 —— 多选点了提交没反应，就是这么来的。
   *
   * **单选不用它**：数字键按下即落定并翻页。曾经单选也补这一下，理由是「数字键只是
   * 移动高亮、不翻页」—— 那个判断是错的。当年观察到的「终端停在本题等确认」，真凶
   * 是客户端两秒后自动补的回车（它翻了页，让人以为数字键没翻），而那道补回车早已按
   * from_select 关掉了。
   *
   * 多发的这一下不是无害的：3 个选项时（当时还按 N+2 算）它是「5」，而单选那张卡根本
   * 没有 5 号键，整条作答就此卡住 —— 卡片停在原地，点了等于没点。（08-17 抓到：钉钉回
   *「4」1 秒即落定，网页点同一张卡发出「15」，53 秒毫无动静，最后是人跑去终端手动选的。）
   */
  const nextKey = () => String((q?.options?.length ?? 0) + 3);

  /**
   * 多选提交：勾选序号 + 末尾补一个 [`nextKey`]，连成一串发出去。
   * 4 选项里勾 1、3 就发 "137" —— 逐个数字键勾选，最后一下落在 Submit 上。
   *
   * 序号连写不加分隔符，因为终端认的是按键而不是文本。AskUserQuestion 每题至多 4 个
   * 选项，编号最大到 7，不会出现两位数带来的歧义。
   */
  const submitPicked = () => submit(picked.join("") + nextKey());

  /** 单选提交：一个序号就够 —— 数字键按下即落定并翻页（见 [`nextKey`] 为何不补）。 */
  const submitOne = (n: number) => submit(String(n));

  if (!q) return null;

  return (
    <div className={styles.selectCard}>
      <div className={styles.selectHead}>
        <span>⌨︎ 终端等待选择</span>
        {/* 多题时把进度说清楚，否则答完一题卡片换了内容，会以为是出了什么岔子 */}
        {total > 1 ? (
          <span className={styles.selectStep}>
            第 {step + 1} / {total} 题
          </span>
        ) : null}
      </div>
      <div className={styles.selectQ}>
        {q.question ? (
          <div className={styles.selectQuestion}>{q.question}</div>
        ) : null}
        <div className={styles.selectOpts}>
          {(q.options ?? []).map((o, oi) => (
            <div
              key={oi}
              className={`${styles.selectOpt} ${onAnswer ? styles.clickable : ""} ${
                picked.includes(oi + 1) ? styles.selectOptPicked : ""
              }`}
              role={onAnswer ? "button" : undefined}
              tabIndex={onAnswer ? 0 : undefined}
              aria-pressed={multi ? picked.includes(oi + 1) : undefined}
              // 单选点一下即落定（序号 + 下一题键）；多选只切换勾选，攒齐了再由下方按钮一次性提交
              onClick={() => (multi ? toggle(oi + 1) : submitOne(oi + 1))}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  if (multi) {
                    toggle(oi + 1);
                  } else {
                    submitOne(oi + 1);
                  }
                }
              }}
            >
              <span className={styles.selectOptIdx}>
                {multi ? (picked.includes(oi + 1) ? "✓" : oi + 1) : oi + 1}
              </span>
              <span className={styles.selectOptBody}>
                <span className={styles.selectOptLabel}>{o.label}</span>
                {o.description ? (
                  <span className={styles.selectOptDesc}>{o.description}</span>
                ) : null}
              </span>
            </div>
          ))}
          {/* 自行输入：AskUserQuestion 隐含的「其它」，输入后回车 / 点发送提交。
              它只回答**当前这一题**，发完照样进入下一题。 */}
          <div className={`${styles.selectOpt} ${styles.selectOptCustom}`}>
            <span className={styles.selectOptIdx}>✎</span>
            <span className={styles.selectOptBody}>
              <input
                className={styles.selectCustomInput}
                placeholder="自行输入答案…"
                value={custom}
                disabled={!onAnswer}
                onChange={(e) => setCustom(e.target.value)}
                onCompositionStart={() => {
                  composing.current = true;
                }}
                onCompositionEnd={() => {
                  // 延后一拍再解除：失焦收尾时「结束组合」与补发的 Enter 常常
                  // 挨在同一轮里，立刻置回 false 就等于没挡
                  setTimeout(() => {
                    composing.current = false;
                  }, 0);
                }}
                onKeyDown={(e) => {
                  // 输入法组合中的 Enter 是「确认候选词」，不是「发送」。
                  // 不挡的话，中文还没打完就被当成答案发进终端了 —— 而且这一发
                  // 就推进到下一题，回不去。（Composer 早有这道防护，这里漏了。）
                  if (
                    e.key === "Enter" &&
                    !e.nativeEvent.isComposing &&
                    !composing.current
                  ) {
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
          {/* 多选专用的提交行：对应终端选择卡底部那个 Submit。单选没有这一步（点一下即落定），
              所以只在 multiSelect 时出现，免得单选也要多点一次。 */}
          {multi ? (
            <div
              className={`${styles.selectSubmit} ${
                onAnswer && picked.length ? styles.clickable : styles.disabled
              }`}
              role="button"
              tabIndex={0}
              onClick={submitPicked}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  submitPicked();
                }
              }}
            >
              {picked.length ? `提交所选（${picked.length} 项）` : "请先勾选选项"}
            </div>
          ) : null}
        </div>
      </div>
      {/* 「回车」在手机虚拟键盘上不成立（那是「换行/完成」），所以窄屏改说「点发送」。
          两条文案同时渲染、靠 CSS 择一显示 —— 免得为一句提示引一套设备判断。 */}
      <div className={styles.selectHint}>
        <span className={styles.hintDesktop}>
          点选项直接回应；或在「✎ 自行输入」里敲自定义答案后回车
        </span>
        <span className={styles.hintMobile}>
          点选项直接回应；或在「✎ 自行输入」里写答案后点发送
        </span>
      </div>
    </div>
  );
};

/** 工具结果超过该行数时折叠 */
const RESULT_CLAMP_LINES = 4;
// 下发的用户内容过长时，正文里先折起来：超过这些行数、或字数（应对单行超长
// 粘贴）就夹断，给个「展开全部 / 收起」。阈值取「一屏能顺手扫完」的量。
const USER_CLAMP_LINES = 12;
const USER_CLAMP_CHARS = 600;

const TerminalFeed: React.FC<TerminalFeedProps> = (props) => {
  const { messages, running, providerDsr, imageCtx } = props;
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
            <SessionMarkdown content={m.content} imageCtx={imageCtx} />
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
          <SessionMarkdown content={m.content} imageCtx={imageCtx} />
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
        // 执行中也铺，但**过程一律折叠着**（下面的分组逻辑会把它并成一行
        // 「执行过程 · N 步」）—— 一条条冒出来是噪音、还不停把视图往下推，
        // 但整段藏掉又会让人不知道它在干什么。折叠着实时长，想看点开即可。
        //
        // （清单与后台任务是「当前状态」，已由 ChatPane 抽成单独面板；
        // 选择卡同理挂在输入框上方，都不进内容流。）
        const visibleItems = keyed;
        // 执行中时给一条「最近动作」预览（最后一条工具调用）
        const lastTool = inProgress
          ? [...turn.items].reverse().find((m) => m.role === "tool")
          : undefined;

        return (
          <div key={turn.key} className={styles.turn}>
            {/* 对话流只管「我说了什么」。排队状态与撤回一律交给输入框上方的排队条 ——
                两处都摆一份的话，同一条任务在正文和排队条各显示一遍，还得为了去重
                把正文里的消息藏起来，于是「我发的内容在对话流里不见了」。
                职责分开之后，正文永远是完整的对话记录。 */}
            {turn.user ? (
              (() => {
                const uKey = `u|${turn.key}`;
                const uContent = turn.user.content;
                const uLines = uContent.split("\n");
                const uLong =
                  uLines.length > USER_CLAMP_LINES ||
                  uContent.length > USER_CLAMP_CHARS;
                const uClamped = uLong && !expanded[uKey];
                let uShown = uContent;
                if (uClamped) {
                  uShown = uLines.slice(0, USER_CLAMP_LINES).join("\n");
                  if (uShown.length > USER_CLAMP_CHARS) {
                    uShown = uShown.slice(0, USER_CLAMP_CHARS);
                  }
                }
                return (
                  <div className={styles.userRow}>
                    <div className={styles.userBubble}>
                      {uClamped ? `${uShown}…` : uShown}
                      {uLong && (
                        <span
                          className={styles.userExpandBtn}
                          role="button"
                          tabIndex={0}
                          aria-expanded={!uClamped}
                          onClick={() => toggleExpand(uKey)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              toggleExpand(uKey);
                            }
                          }}
                        >
                          {uClamped ? "展开全部" : "收起"}
                        </span>
                      )}
                    </div>
                    <div className={styles.userTime}>{fmtTime(turn.user.timestamp)}</div>
                  </div>
                );
              })()
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
                    const out: React.ReactNode[] = [];
                    let group: { m: PortalMessage; k: string }[] = [];
                    const flush = () => {
                      if (!group.length) {
                        return;
                      }
                      const gkey = `tg-${group[0].k}`;
                      const openG = !!expanded[gkey];
                      // 有工具就按工具步数报数（「查了 6 步」比「6 条消息」更贴近直觉），
                      // 纯文字过程才退回条数
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

                    // 回合已结束：结论已经有了，中间的摸索过程就不该再一条条占屏 ——
                    // 把「最后一条产出」之前的**全部**内容（工具流水、中途的助手说明、
                    // 已被后续结论取代的方案卡）并成一行折叠，只留结论展开。
                    //
                    // 执行中：还没有结论可留，于是**全部**进折叠组 —— 折叠行会随着
                    // 步数实时增长，既不刷屏也看得见在动，想看点开即可。
                    const lastOutIdx = inProgress
                      ? visibleItems.length
                      : visibleItems.reduce(
                          (acc, { m }, i) =>
                            ["assistant", "plan"].includes(m.role) ? i : acc,
                          -1,
                        );

                    visibleItems.forEach((it, i) => {
                      const { m, k } = it;
                      // 待批准的方案在执行中永远展开：它在等你点头，折起来
                      // 就等于把要办的事藏了。（回合结束后它已被结论取代，照折。）
                      if (inProgress && m.role === "plan") {
                        flush();
                        out.push(renderItem(m, k));
                        return;
                      }
                      // 结论之前的一律进折叠组；结论及其之后的按原规则（工具仍折叠）
                      const isTool = m.role === "tool" || m.role === "tool_result";
                      if (isTool || (lastOutIdx >= 0 && i < lastOutIdx)) {
                        group.push(it);
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
