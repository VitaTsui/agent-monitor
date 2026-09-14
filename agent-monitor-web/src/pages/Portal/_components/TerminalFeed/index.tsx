import React, { useEffect, useRef, useState } from "react";

import dayjs from "dayjs";
import { Icon } from "@hsu-react/ui";
import SessionMarkdown from "../SessionMarkdown";
import { SessionImageCtx } from "@/utils/sessionImages";

import { observer } from "mobx-react-lite";

import {
  PortalMessage,
  SelectPayload,
  SubTask,
  SubTaskOutcome,
} from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import {
  SUB_OUTCOME_LABEL,
  fmtElapsed,
  isSubTaskRunning,
} from "../../_utils/sessionState";
import AgentCard from "../AgentCard";
import StatusIcon, { CHAIN_ICON, statusOfOutcome } from "../StatusIcon";
import styles from "./index.module.scss";

interface TerminalFeedProps {
  /** 这一格装的是哪条会话。拉子代理正文要用它 */
  taskId: string;
  messages: PortalMessage[];
  /**
   * 这条会话名下的子任务。执行链靠它把「派子代理那次工具调用」认出来：
   * `tool.id === subTask.toolUseId` 即为同一次派活（见 buildChain）。
   *
   * **后台命令也认这个键**：那次 `Bash` 调用的返回只说「已经放到后台了」，
   * 它后来跑成什么样只有这份清单知道 —— 配上之后画在链上那一步的右端。
   */
  subTasks?: SubTask[];
  /** 会话是否执行中（末尾显示工作指示） */
  running?: boolean;
  /** 终端卡标题：来源代理名（Claude Code / Codex / Gemini CLI …） */
  providerDsr?: string;
  /** 会话上下文：内容里的本地图片路径靠它解析（见 utils/sessionImages） */
  imageCtx?: SessionImageCtx;
  /**
   * 「把执行链滚到这个子代理的卡片上并展开它」的一次请求（侧栏子会话树点的）。
   * 带 `seq` 是因为定位是个**动作**：同一个子代理连点两次也要能再滚一次。
   */
  focusAgent?: { agentId: string; seq: number } | null;
  /**
   * 定位的结果。`el` 有值 = 卡片已经展开、这是要滚到的那个元素；
   * `null` = 这条链上找不到它（起跑那次工具调用不在已加载的正文里）。
   *
   * **滚动不在这里做**：对话流的滚动容器与「钉底」那套开关都在 `ChatPane` 手上，
   * 这边自己滚一下会被它下一帧钉回底部。找谁、怎么滚，各管各的。
   */
  onFocusAgent?: (el: HTMLElement | null, seq: number) => void;
  // 撤回不在这里：排队状态与撤回统一由输入框上方的排队条负责，
  // 对话流只呈现「我说了什么、它回了什么」。
}

/**
 * 链上每个节点左边那枚**方形图标**：外层是 18×18 的不透明色块，里层才是字形。
 *
 * **必须分两层。** 执行中那枚要转圈（`StatusIcon` 的 `.spin`），动画挂在**图标
 * 自己的那个元素**上；色块与字形合成一个元素时，
 * 转的就是这枚色块本身 —— 实测外接盒被从 20 转到 25.18，方块的四个角一圈圈扫过
 * 身后那根竖线，看着就是「图标在抖」。色块得钉住不动，只让字形转。
 * （`AgentCard` 的 `.headTile` / `.headIcon` 早就是这个结构，这里补齐。）
 *
 * 色块**底色不透明**：那根贯穿整条链的竖线是从节点背后穿过去的
 * （`.step::before`），靠它在图标处遮断，才有「线上挂着一个个格子」的样子。
 * 四类节点（工具调用 / 旁白 / 子代理 / 提示行）共用这一副规格 —— 规格统一是
 * 这条时间轴读得出「一节点一格」的前提。
 */
const StepTile: React.FC<{
  children: React.ReactNode;
  className?: string;
}> = ({ children, className }) => (
  <span className={`${styles.stepTile} ${className ?? ""}`}>{children}</span>
);

/** 槽里那枚**非状态**的字形（工具 / 旁白 / 省略号）。状态一律走 `StatusIcon` */
const StepGlyph: React.FC<{ icon: string }> = ({ icon }) => (
  <Icon icon={icon} className={`${styles.stepIcon} ${styles.stepGlyph}`} />
);

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
  /** 已答过的各题答案，按题序攒着，答满了一次性发出（见 submit） */
  const [answers, setAnswers] = useState<string[]>([]);
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

  /**
   * 逐题记下答案，**答满了才一次性下发**（多题用逗号分隔，如 "1,2"）。
   *
   * 曾经是答一题发一条。那样 hub 永远只看到一题的答案，判不出「题已答满」，也就补不上
   * 多题答完后那层 Review（"Ready to submit your answers?"）的确认回车 —— 卡片一直挂在
   * 终端上等人，远端答完还得有人跑去终端手动 submit。
   *
   * 逐题发还有第二重害处：两题的按键前后脚送进终端，前一题若因故没落定，后一题的答案
   * 就落回它身上。攒齐再发，整串由 hub 一次翻译成有序的按键序列。
   */
  const submit = (text: string) => {
    if (busy.current || !onAnswer || !text) return;
    busy.current = true;
    const all = [...answers, text];
    setAnswers(all);
    setCustom("");
    if (step + 1 < total) {
      setStep(step + 1);
    } else {
      onAnswer(all.join(","));
      onDone?.();
    }
  };

  const submitCustom = () => submit(custom.trim());

  const multi = !!q?.multiSelect;

  const toggle = (n: number) =>
    setPicked((p) =>
      p.includes(n)
        ? p.filter((x) => x !== n)
        : [...p, n].sort((a, b) => a - b),
    );

  /**
   * 多选提交：只发勾选的序号，**不再自己补 Submit 键**。
   *
   * 曾经这里按 N+3 算 Submit（选项 N 个 +「其它」+「chat about」+ Submit），那条规则
   * 是错的：终端选择卡的数字键只能索引到「N 个选项 + Other」之内，Submit 压根不在列表
   * 里 —— 它是组件内一个独立的聚焦态，只能 Tab 过去再回车。N+2 / N+3 都越界并被静默
   * 丢弃，于是多选**从来就没提交过**。
   *
   * 更糟的是卡片因此停在原地：紧接着下一题的答案发过来，落回前一题把已勾选的项
   * toggle 掉 —— 表现成「明明选的 2，终端选成了 1」。（08-25 现场：4 选项的多选题
   * 发出「127」，那个 7 越界，2 秒后第二题的「1」把第 1 项取消了。）
   *
   * 现在只负责说「勾了哪几项」，怎么落到按键上由 hub 按题型翻译（plan_select_answer），
   * 与钉钉、MCP 共用同一份 —— 各算各的正是这个 bug 的由来。
   */
  const submitPicked = () => submit(picked.join(""));

  /** 单选提交：一个序号就够 —— 数字键按下即落定并翻页。 */
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
              {picked.length
                ? `提交所选（${picked.length} 项）`
                : "请先勾选选项"}
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
/** 长内容折叠的高度阈值（px）。超过它才夹断并给渐隐遮罩 */
const CLAMP_MAX_H = 200;

/**
 * 超高就夹断的容器：限高 200px ＋ 底部 48px 渐隐，点「展开全部」放开。
 *
 * **按实际渲染高度判断，不按字数估、更不切字符串**。原先是
 * `lines.slice(0, 12).join("\n").slice(0, 600)` —— 两个问题：
 *   - `slice(0, 600)` 按 UTF-16 码元切，落在一个汉字或表情的两个码元中间
 *     就会切出半个字符（emoji 直接变成乱码方块）；
 *   - 12 行 / 600 字与「屏幕上占多高」没有关系：一行 200 字的粘贴算 1 行，
 *     12 行短句却可能只有三行高。该不该折叠本来就是个高度问题。
 * 量一次真实高度，这两件事一起没了。
 */
const ClampBox: React.FC<{
  children: React.ReactNode;
  /** 展开态由父级统一持有（跨刷新稳定，见 msgKey 的说明） */
  open: boolean;
  onToggle: () => void;
}> = ({ children, open, onToggle }) => {
  const innerRef = React.useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(false);

  // 内容是异步长出来的（markdown、代码块、字体、图片），一次性测量会偏小，
  // 所以挂 ResizeObserver 跟着内容走。
  useEffect(() => {
    const el = innerRef.current;
    if (!el) {
      return;
    }
    const measure = () => setOverflow(el.scrollHeight > CLAMP_MAX_H + 8);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [children]);

  const clamped = overflow && !open;

  return (
    <div className={`${styles.clampBox} ${clamped ? styles.clamped : ""}`}>
      <div className={styles.clampInner} ref={innerRef}>
        {children}
      </div>
      {overflow ? (
        <span
          className={styles.clampBtn}
          role="button"
          tabIndex={0}
          aria-expanded={open}
          onClick={onToggle}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onToggle();
            }
          }}
        >
          {clamped ? "展开全部" : "收起"}
        </span>
      ) : null}
    </div>
  );
};

/* ---------- 执行链：把一轮的过程排成一条竖线时间轴 ---------- */

/**
 * `mcp__playwright__browser_click` → `Browser click`。
 *
 * 照 VitaAgent `MessageList/index.tsx:71-79`：链上写的是「一句人话」，
 * 不是原始函数名 —— 原始名在展开后仍看得到，排查时要的是它，
 * 扫一眼「它刚做了什么」时要的是这句。`server__` 前缀去掉，来源另有一枚标。
 */
const humanTool = (name: string) => {
  const bare = name.includes("__")
    ? name.slice(name.lastIndexOf("__") + 2)
    : name;
  const words = bare.replace(/[_-]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : name;
};

/** 执行链里的一步：一次工具调用（带它的输出）、一段旁白，或一批子代理 */
interface ChainCall {
  kind: "call";
  key: string;
  /** 这一格发生的时刻（源消息的 timestamp）。按时间往链里插东西要用它，见 `attachLooseAgents` */
  at?: string;
  /** 这次调用的 `tool_use_id`。老记录可能没有 */
  id?: string;
  /** 原始工具名，展开后照原样给 */
  name: string;
  /** 入参提示（命令 / 路径 / 描述），后端 `tool_input_hint` 挑出来的那一个 */
  hint: string;
  /** 这次调用的输出。一条调用可能跟着多条输出记录 */
  results: { key: string; text: string; bad?: boolean }[];
  /**
   * 这一步起的是个**后台命令**（`tool.id === subTask.toolUseId` 且 `kind === "bg"`）。
   *
   * 后台命令的收场**不在这次调用的返回里**：那条返回只说「已经放到后台了」，
   * `isError` 恒为 false，于是链上这一步永远显得「办妥了」，哪怕它十秒后就挂了。
   * 真正的收场在子任务清单的 `outcome` 上（后端按完成通知 / `TaskStop` 结果 /
   * 父会话是否已结束算出来的事实）—— 挂在这里，让它在**发生的位置**说出来。
   *
   * 这是「后台任务」那一节只列还在跑的之后，跑砸的唯一去处
   * （见 `_utils/sessionState.ts` 的 `runningBgCommands`）：那一节是「此刻在办什么」，
   * 收场了的属于历史，而历史就是这条链。
   */
  bg?: SubTask;
}
interface ChainNote {
  kind: "note";
  key: string;
  at?: string;
  msg: PortalMessage;
}
/**
 * **一批子代理**：模型在这一步派了活出去。
 *
 * 一次派活 ＝ 一次 `Agent` 工具调用，靠 `tool.id === subTask.toolUseId` 认出来。
 * **相邻**的几次派活（中间没有别的步骤、也没有旁白）归成一格 —— 那就是并行起的
 * 那一批，界面上是同一张卡里的一片小卡网格。
 */
interface ChainAgents {
  kind: "agents";
  key: string;
  at?: string;
  agents: SubTask[];
  /** 各自那次调用的入参提示（比 `SubTask.label` 长一截：120 字 vs 80 字） */
  hints: string[];
  /**
   * 这张卡是**按时间落位**的，不是由某次派活记录带出来的。
   *
   * 为真时说明「派出它的那条记录不在已加载的正文里」（或父记录里压根没有），
   * 卡片仍然要画 —— 子代理存不存在由子任务清单说了算，消息流只决定它插在哪儿。
   */
  loose?: boolean;
}
type ChainItem = ChainCall | ChainNote | ChainAgents;

/** 这一格的标题：一个时写它的派活说明，多个时写「起了 N 个子代理」 */
const agentsGoal = (it: ChainAgents) =>
  it.agents.length === 1
    ? it.hints[0] || it.agents[0].label
    : `起了 ${it.agents.length} 个子代理`;

/** 链项是不是「一步」（工具调用 / 一批子代理）。旁白不算：它是围着某一步说的话 */
const isStep = (it: ChainItem) => it.kind === "call" || it.kind === "agents";

/**
 * 这一格里**还有子代理在跑**。
 *
 * 折叠的全部判据都从这一条来：折叠是给「已经结束、可以不看」的内容用的，
 * 还在跑的东西必须一直看得见。判据是**结构性**的 —— 清单里那个子任务的
 * `outcome` 是不是 `running`，不看时间戳、不看「最近多少秒有没有动静」。
 */
const hasLiveAgent = (it: ChainItem) =>
  it.kind === "agents" && it.agents.some(isSubTaskRunning);

/** 时刻 → 毫秒。解析不了给 `NaN`，调用方据此退回「排最后」 */
const atMs = (iso?: string): number => (iso ? Date.parse(iso) : NaN);

/**
 * **把「配不上任何一次派活记录」的子代理按时间插回链里。**
 *
 * 为什么必须有这一步（正确性问题，不是体验优化）：链上画不画一张卡，此前的唯一判据是
 * 「正文里有没有那次 `Agent` 调用」，而正文是**被三层窗口截断过**的 ——
 * 客户端每轮只捎带 80 条（`client/src/agent.rs:891`）、hub 对活跃会话直接回这份缓存、
 * 前端再留最多 `MAX_MESSAGES_PER_TASK` 条，而且**没有 offset/游标，更早的根本取不到**。
 * 实测真实会话 `9168ec90`（789 条消息 / 28 个子代理）：留 500 条时链上只配得上 8 个，
 * **20 个凭空消失**。还有一类抬多高的窗口都救不回来 —— 父记录里压根没有那条
 * `tool_use`（`toolUseId` 取自 `agent-*.meta.json`），实测 2 个。
 *
 * 所以判据改成两件事解耦：
 *   **画不画卡** → 由子任务清单说了算（`/subtasks` 是全量、不受窗口限制）；
 *   **插在哪儿** → 由消息流说了算（配得上就插在那一步，配不上就按 `startedAt` 落位）。
 * 这样「子代理凭空消失」在结构上不可能再发生，不依赖任何一层窗口调到多大。
 *
 * 落位规则（按时间，不另开一个「找不到的那些」区域）：
 *   1. 落到**它起跑那一刻所属的那一轮**（最后一个「开始时刻 ≤ startedAt」的轮次；
 *      比所有轮次都早 = 它起跑于已经被截掉的那段对话，落到最早那一轮的**开头**）；
 *   2. 轮内插在第一个「发生时刻 > startedAt」的链项之前，因此不会出现后起的排在先起的前面；
 *   3. `startedAt` 完全相同的几个并成一张卡 —— 那就是同一批并行派出去的；
 *   4. `startedAt` 缺失的排到最后（没有时间可依，至少不谎报位置）。
 *
 * **不另开区域**是有理由的：链本身就是时间轴，按时间插回去仍然回答得了「它属于哪个阶段」；
 * 另开一块则会出现第二个放子代理的地方，而「同一份东西画两处」是这一版反复推翻的东西。
 * **同一个子代理只出现一次**：配上的在那一步，配不上的按时间落位，两条路互斥（`placed`）。
 */
const attachLooseAgents = (
  turnChains: { chain: ChainItem[]; startMs: number }[],
  agents: SubTask[],
): void => {
  if (!turnChains.length || !agents.length) {
    return;
  }
  /* 已经就地插好的那些：链上任何一张卡里出现过的 id。
     判据是「渲染结果里有没有它」，不是「配没配上 toolUseId」—— 两者迟早分叉 */
  const placed = new Set<string>();
  turnChains.forEach(({ chain }) =>
    chain.forEach((it) => {
      if (it.kind === "agents") {
        it.agents.forEach((a) => placed.add(a.id));
      }
    }),
  );

  const loose = agents
    .filter((a) => !placed.has(a.id))
    .sort((a, b) => {
      const x = atMs(a.startedAt);
      const y = atMs(b.startedAt);
      if (Number.isNaN(x)) return 1;
      if (Number.isNaN(y)) return -1;
      return x - y;
    });
  if (!loose.length) {
    return;
  }

  // 起跑时刻完全相同的并成一张卡：那是同一批并行派出去的
  const batches: SubTask[][] = [];
  loose.forEach((a) => {
    const last = batches[batches.length - 1];
    if (last && last[0].startedAt && last[0].startedAt === a.startedAt) {
      last.push(a);
      return;
    }
    batches.push([a]);
  });

  batches.forEach((batch) => {
    const ms = atMs(batch[0].startedAt);
    /* 落到哪一轮：最后一个「开始时刻 ≤ 它」的轮次。比所有轮次都早（起跑于已被截掉
       的那段对话）就落到最早那一轮；时刻缺失就落到最后一轮的末尾。 */
    let ti = 0;
    if (Number.isNaN(ms)) {
      ti = turnChains.length - 1;
    } else {
      for (let i = 0; i < turnChains.length; i += 1) {
        if (turnChains[i].startMs <= ms) {
          ti = i;
        }
      }
    }
    const chain = turnChains[ti].chain;
    const item: ChainAgents = {
      kind: "agents",
      key: `loose|${batch.map((a) => a.id).join(",")}`,
      at: batch[0].startedAt,
      agents: batch,
      // 没有派活记录就没有入参提示，卡片标题退回子代理自己的名字（见 agentsGoal）
      hints: batch.map(() => ""),
      loose: true,
    };
    const at = Number.isNaN(ms)
      ? chain.length
      : chain.findIndex((x) => {
          const t = atMs(x.at);
          return !Number.isNaN(t) && t > ms;
        });
    chain.splice(at < 0 ? chain.length : at, 0, item);
  });
};

/**
 * 这条链上**此刻挂起着的那一步**的 key（没有就是空串）。
 *
 * 判据是结构上的：调用与它的输出成对出现，最后一次调用还没有任何输出 = 它正跑着。
 * 不拿时间戳猜「多久没动静了」—— 一条跑了十分钟的命令和一条卡死的命令，
 * 时间戳上一模一样。
 *
 * **两处共用这一份**：链里据此让那一步走流光 ＋ 转圈，轮次那一层据此决定要不要在
 * 末尾顶「正在思考…/正在处理…」那一行。两边各算一遍的话，迟早出现「两处都在动」
 * 或者「两处都不动」。
 */
const pendingCallKey = (items: ChainItem[]): string => {
  const lastCall = [...items]
    .reverse()
    .find((i): i is ChainCall => i.kind === "call");
  return lastCall && !lastCall.results.length ? lastCall.key : "";
};

/**
 * 把一段消息排成执行链。
 *
 * 主会话与子代理**共用这一份**：后端下发的子代理正文与主会话结构完全一致
 * （`PortalMessage[]`），所以链的组装只有一套，不存在「子代理那边再写一遍」。
 *
 * 三件事在这里定下：
 *
 *   1. **工具调用读 `m.tools`，不读 `m.content`。** 后端已把 `role: "tool"` 的
 *      content 置空、改下发结构化数组（一次调用一个元素）。此前那套「按第一个
 *      `": "` 切开、多次调用用 `" | "` 拼」的消费方式整条删掉，不留过渡期 ——
 *      留着的结果是那一行在界面上**完全空白**。
 *   2. **输出按 `m.toolUseId` 贴回对应那一步**，不再按先后顺序猜。实测顺序猜是
 *      会错的：AskUserQuestion 的答复没有对应的 tool 记录（它另走 select 卡），
 *      按顺序会挂到它前面那次 `Agent` 调用上。
 *   3. **派子代理那一步画成智能体卡**，判据只有 `tool.id === subTask.toolUseId`
 *      这一条。配不上就照普通工具调用画 —— 不退回按 label / 中文文案匹配：
 *      两边截断长度不同（120 vs 80），必然错配。
 */
const buildChain = (
  items: { m: PortalMessage; k: string }[],
  opts: {
    /** `tool_use_id` → 它派出去的那个子代理。空表 = 这段里不画智能体卡 */
    subByToolUse: Map<string, SubTask>;
    /**
     * `tool_use_id` → 它起的那个后台命令。空表 = 这段里的 Bash 步不带收场。
     *
     * 与 `subByToolUse` **分开两张表**：子代理那张决定「这一格画成智能体卡还是
     * 普通一步」，这张只往普通一步上补一个收场，两件事合成一张表就得在取值处
     * 再判一次 `kind`，判漏了就会把后台命令画成智能体卡（它没有正文可展开）。
     */
    bgByToolUse: Map<string, SubTask>;
    /**
     * 「结论留在链外」的分界线怎么画。跑着的那一轮与跑完的那一轮判据不同，
     * 见调用处。`flat` = 全都进链，一个字都不留到链外（子代理那条链就是这样：
     * 它整段都是过程，结论已经由父会话并回主对话了）。
     */
    split: { flat: true } | { flat: false; inProgress: boolean };
  },
): { chain: ChainItem[]; body: { m: PortalMessage; k: string }[] } => {
  const { subByToolUse, bgByToolUse, split } = opts;
  const chain: ChainItem[] = [];
  const body: { m: PortalMessage; k: string }[] = [];

  /* 分界线：跑完了 → 最后一段产出（assistant / plan）就是结论，它之前的一切是过程；
     跑着呢 → 最后一步过程之后的正文才是「正在写的那段」，之前的每段正文都是旁白。
     沿用跑完那条的话，旁白会被留在链的**下面**、而它引出的那几步却在链里。 */
  let lastOut = -1;
  let lastProc = -1;
  items.forEach(({ m }, i) => {
    if ((m.role === "assistant" || m.role === "plan") && m.content.trim()) {
      lastOut = i;
    }
    if (m.role === "tool" || m.role === "tool_result") {
      lastProc = i;
    }
  });

  /** 按 tool_use_id 索引已经落到链上的那几步，供 tool_result 精确归位 */
  const callById = new Map<string, ChainCall>();
  /** 最后一次调用 —— 老记录没有 toolUseId 时退回「紧挨着的上一步」 */
  let lastCall: ChainCall | null = null;
  /** 正在累积的那一批并行子代理。遇到别的链项就收口 */
  let openAgents: ChainAgents | null = null;

  items.forEach((it, i) => {
    const { m, k } = it;

    if (m.role === "tool") {
      (m.tools ?? []).forEach((t, ti) => {
        const sub = t.id ? subByToolUse.get(t.id) : undefined;
        if (sub) {
          if (!openAgents) {
            openAgents = {
              kind: "agents",
              key: `${k}#a${ti}`,
              at: m.timestamp,
              agents: [],
              hints: [],
            };
            chain.push(openAgents);
          }
          openAgents.agents.push(sub);
          openAgents.hints.push(t.hint ?? "");
          return;
        }
        openAgents = null;
        const call: ChainCall = {
          kind: "call",
          key: `${k}#${ti}`,
          at: m.timestamp,
          id: t.id,
          name: t.name,
          hint: t.hint ?? "",
          results: [],
          bg: t.id ? bgByToolUse.get(t.id) : undefined,
        };
        chain.push(call);
        lastCall = call;
        if (t.id) {
          callById.set(t.id, call);
        }
      });
      return;
    }

    if (m.role === "tool_result") {
      /* 派子代理那次调用的返回是一段**内部元数据**（原文 "Async agent launched
         successfully. (This tool result is internal metadata — never quote…)"），
         不是子代理交回来的结论 —— 它的结论在它自己那条链的末尾。画成一步只会在
         智能体卡下面多出一行没有信息量的「命令输出」 */
      if (m.toolUseId && subByToolUse.has(m.toolUseId)) {
        return;
      }
      const out = { key: k, text: m.content, bad: m.isError };
      const owner = m.toolUseId ? callById.get(m.toolUseId) : lastCall;
      if (owner) {
        owner.results.push(out);
        return;
      }
      // 配不上（历史裁剪把调用那条丢了 / 那次调用另走别的卡）就自成一步，
      // 内容一条都不丢
      openAgents = null;
      const orphan: ChainCall = {
        kind: "call",
        key: k,
        at: m.timestamp,
        name: "",
        hint: "",
        results: [out],
      };
      chain.push(orphan);
      lastCall = orphan;
      return;
    }

    if (split.flat) {
      openAgents = null;
      chain.push({ kind: "note", key: k, at: m.timestamp, msg: m });
      return;
    }

    // 待批准的方案在执行中永远留在正文：它在等你点头，收进链里就等于把要办的事藏了
    if (split.inProgress && m.role === "plan") {
      body.push(it);
      return;
    }
    const isBody = split.inProgress ? i > lastProc : i >= lastOut;
    if (isBody) {
      body.push(it);
      return;
    }
    openAgents = null;
    chain.push({ kind: "note", key: k, at: m.timestamp, msg: m });
  });

  return { chain, body };
};

/**
 * 这一步跑砸了没有。
 *
 * 判据只认后端下发的 `isError`（`tool_result` 块上的 `is_error` 原样带出来），
 * 不去输出文本里找 `error` / `失败` 之类的字眼 —— 那是拿字面量当接口用，
 * 一条打印了 "0 errors" 的成功输出就会被判成失败。
 *
 * 一次调用可能跟着多条输出记录，**有一条报错就算这步跑砸了**。
 */
const stepFailed = (c: ChainCall) => c.results.some((r) => r.bad);

/**
 * 一串链项概括成一句话：`跑了 6 条命令，查了 3 次资料`。
 *
 * 照 VitaAgent `MessageList/index.tsx:260-297`：按类别数数，不逐条列。
 * 分类只按工具名的形态，不做「哪个工具叫什么」的字面表；名字对不上就落到
 * 「调用了 N 次工具」这一档 —— 少说一句总好过说错。
 *
 * 接受**任意子集**而不是整条链：跑的时候这一行概括的是「已经收进去的那些」，
 * 跑完了才是整条链，两处同一套措辞。
 */
const summarizeChain = (items: ChainItem[]): string => {
  const calls = items.filter((i): i is ChainCall => i.kind === "call");
  const ran = calls.filter((c) => /^bash/i.test(c.name));
  const rest1 = calls.filter((c) => !ran.includes(c));
  const looked = rest1.filter((c) =>
    /^(read|glob|grep|websearch|webfetch|notebookread|ls)$|search|query|fetch|list|read/i.test(
      c.name,
    ),
  );
  const rest2 = rest1.filter((c) => !looked.includes(c));
  const wrote = rest2.filter((c) => /write|edit|patch/i.test(c.name));
  const others = rest2.filter((c) => !wrote.includes(c));

  /* 子代理单独数一句，放最前：这一行里「它把活派给了谁」比「调了几次工具」
     更先要回答（照 VitaAgent `MessageList/index.tsx:260-297` 里连接器排最前的那条）。
     头部那枚「子会话 · N」胶囊撤掉之后，这句话就是计数唯一的去处 */
  const agents = items
    .filter((i): i is ChainAgents => i.kind === "agents")
    .reduce((n, i) => n + i.agents.length, 0);

  const clauses: string[] = [];
  if (agents) clauses.push(`起了 ${agents} 个子代理`);
  if (ran.length) clauses.push(`跑了 ${ran.length} 条命令`);
  if (looked.length) clauses.push(`查了 ${looked.length} 次资料`);
  if (wrote.length) clauses.push(`改了 ${wrote.length} 个文件`);
  if (others.length) clauses.push(`调用了 ${others.length} 次工具`);
  /* 失败要在**收起状态**下就说得出来：跑完的那一轮整条链默认是折起来的
     （`shown` 在非 live 且未展开时是空数组），只在步骤行上标红等于把它藏在
     一次点击后面 —— 而「哪一轮出过错」恰恰是回看时最先想知道的事。 */
  const bad = calls.filter(stepFailed).length;
  const head = clauses.join("，");
  if (!bad) return head;
  return head ? `${head}，其中 ${bad} 步没跑成` : `${bad} 步没跑成`;
};


/**
 * **空当里那一行**：这一轮跑着、但此刻链上没有任何一步挂起时，顶在末尾的
 * `⟳ 正在思考… · 耗时` / `⟳ 正在处理… · 耗时`。
 *
 * 它与「跑着的那一步」是**互斥**的两档，合起来保证这一轮跑着的每一刻屏幕上
 * 都恰好有一处在动（判据见调用处的 `pendingCallKey`）：
 *
 *   有一步挂起着（最后一次调用还没有输出）→ 不摆这一行，
 *       由那一步自己的名字走流光 ＋ 图标转圈。**不加状态词** —— 那一行已经写着
 *       具体在干什么，再补三个字是同一件事说两遍（用户为此提过两次）。
 *   一步都没挂起（刚发下去、或上一步已返回、下一步还没发起）→ 摆这一行。
 *       此刻屏幕上**没有别的东西在动**，不存在「重复别人已经说过的话」的问题；
 *       没有它，用户看到的就是一列跑完的步骤，分不清「还在想」与「卡死了」。
 *
 * 两句文案的分界也是结构性的，不是修辞：
 *   `thinking`（这一轮一个节点都还没有）→ 「正在思考…」。模型收到任务、还没有
 *       任何产出，此刻它就是在想；这一档是我们能对上 VitaAgent `ThinkingBlock`
 *       （`MessageList/index.tsx:278`）的**唯一**一档 —— 它那边有流式的 thinking
 *       正文，我们的 jsonl 里 99% 的 thinking 块是空串（详见文件末尾的 TODO），
 *       写不出「它在想什么」，只说得出「它在想」。
 *   `working`（已经有产出，此刻没有挂起的步）→ 「正在处理…」。
 *
 * 耗时每秒走字：**「还在跑」与「卡住了」的唯一区别**就是它动不动。
 * 口径与 SessionPanels / SubAgentChip 共用 `fmtElapsed`，全项目只有这一套算法。
 *
 * **它待在「到此为止发生的最后一件事」后面**，位置由调用方给（见那边的 `tail`）：
 *   链就是最后一件事 → 它作为链的**最后一格**渲染进 `.chainBody`，那根贯穿的竖线
 *       直接穿下来、在它的图标槽处收住，与其它步骤一个节奏（间距 0）。
 *   模型之后又写了话 → 它落在那段话后面。那里已经不是链了，`loose` 把竖线撤掉 ——
 *       不撤的话就是一截连不到任何地方的线头（用户原话：「看着有点太割裂了」）。
 */
const Working: React.FC<{
  since?: string;
  thinking?: boolean;
  /** 不在链里（后面没有链可接）：撤掉竖线，其余形制不变 */
  loose?: boolean;
}> = ({ since, thinking, loose }) => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const elapsed = fmtElapsed(since, now);
  const text = thinking ? "正在思考…" : "正在处理…";

  return (
    <div className={`${styles.step} ${loose ? styles.stepLoose : ""}`}>
      <div className={styles.stepHead} role="status" aria-label={text}>
        <StepTile>
          <StatusIcon kind="running" className={styles.stepIcon} />
        </StepTile>
        <span className={`${styles.stepNameLive} ${styles.liveText}`}>
          {text}
        </span>
        {/* 耗时**紧跟着话**，用间隔点连起来 —— 与失败那一步的 `失败` 同一枚
            `.stepStatus`。原来它走右对齐的 `.stepMeta`（照参照
            `MessageList/index.module.scss:455`）：参照那边 `stepMeta` 是**每一步都有**
            的一列量（步数、耗时），右对齐是在排一张表；我们后端不下发每步耗时
            （见文件末尾 TODO），这一列全项目只有这一行用得上 —— 一列只有一个元素，
            右对齐就不是对齐，是把「29秒」和「正在处理…」甩到 768 宽正文列的两端
            （实测间距 604px），读起来像两件无关的事。
            链上其余的量早就是这个口径了，见 `.stepStatus` 那段注释。 */}
        {elapsed ? (
          <span className={styles.stepStatus}>{elapsed}</span>
        ) : null}
      </div>
    </div>
  );
};

/**
 * 命令的原始输出。**这一块仍然是终端形态**：等宽字 ＋ 深色终端面。
 *
 * 它和模型正文是两类东西，改成执行链之后这条界线一点没松：正文走比例字
 * 16/1.5 ＋ markdown 基线（`styles/_chatMarkdown.scss`），输出走 `--term-*`
 * 那块深底 —— 按列对齐才读得懂的东西，换成比例字就散了。
 */
const ResultBlock: React.FC<{
  text: string;
  open: boolean;
  onToggle: () => void;
}> = ({ text, open, onToggle }) => {
  const lines = text.split("\n");
  const long = lines.length > RESULT_CLAMP_LINES;
  const clamped = long && !open;
  const shown = clamped ? lines.slice(0, RESULT_CLAMP_LINES).join("\n") : text;

  return (
    <div className={styles.resultLine}>
      <div className={styles.resultBody}>
        <pre className={styles.resultText}>{shown}</pre>
        {long ? (
          <span
            className={styles.expandBtn}
            role="button"
            tabIndex={0}
            aria-expanded={!clamped}
            onClick={onToggle}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onToggle();
              }
            }}
          >
            {clamped
              ? `展开其余 ${lines.length - RESULT_CLAMP_LINES} 行`
              : "收起"}
          </span>
        ) : null}
      </div>
    </div>
  );
};

/**
 * 链上的一步。**不是卡片**：图标 ＋ 名称直接落在背景上，
 * 相邻两步之间由一小段竖线连起来（`.step + .step::before`）。
 *
 * 照 VitaAgent `MessageList/index.tsx:81-148` 的 `ToolCard`：
 * 图标列 20 宽、行高 28、名称与状态之间用间隔点连读、箭头紧跟内容，
 * 展开后是「工具 / 入参 / 返回」三段。
 *
 * 右端的状态**只在真有依据时才写**，依据一律是结构化字段：普通调用看返回里的
 * `isError`，后台命令看子任务清单算出来的 `outcome`（见 `ChainCall.bg`）。
 * 「还在跑」与「正常跑完」都不写字（左边那枚图标已经说完了）。
 * 每步耗时后端没下发 —— 详见文件末尾的 TODO，宁可空着也不拿两条记录的
 * 时间戳相减冒充。
 */
const StepRow: React.FC<{
  step: ChainCall;
  running: boolean;
  open: boolean;
  onToggle: () => void;
  expanded: Record<string, boolean>;
  toggleExpand: (key: string) => void;
}> = ({ step, running, open, onToggle, expanded, toggleExpand }) => {
  const human = step.name ? humanTool(step.name) : "命令输出";
  /* 这一步的收场。两个来源，**语义互补不重叠**：
       - 普通调用：只有「返回里报没报错」（`stepFailed` 认后端的 `isError`）；
       - 后台命令：那条返回只说「已经放到后台了」，`isError` 恒 false，收场得看
         子任务清单算出来的 `outcome`（见 `ChainCall.bg`）。
     一律走结构化字段，不去输出文本里找「失败」两个字。 */
  const outcome: SubTaskOutcome | undefined =
    step.bg?.outcome ?? (stepFailed(step) ? "failed" : undefined);
  /* 跑砸的那一步整行走 destructive（与 SessionPanels 的失败行同一套色）：
     一列灰扑扑的步骤里，只有它是「这里出过事」—— 图标、名字、状态一起变色，
     扫一眼就能定位到出错的位置，不用逐条展开找。
     **`interrupted` 不进这一档**：被父会话连带终止不是它自己跑砸的，标红是冤枉
     （与 `SUB_OUTCOME_LABEL` 那段、`AgentCard` 同一条规矩）。 */
  const bad = outcome === "failed";
  /* 「还在跑」也有两个来源：这一轮最后一次调用还没有输出（`running`），
     或者它是个此刻仍挂在后台的命令。后者是本项目独有的一档 —— 长命的
     dev server 起在半小时前，链上这一步应当仍然转着。 */
  const live = running || outcome === "running";

  return (
    <div className={`${styles.step} ${bad ? styles.stepBad : ""}`}>
      <button
        type="button"
        className={styles.stepHead}
        aria-expanded={open}
        {...(live ? { role: "status", "aria-label": "执行中" } : {})}
        onClick={onToggle}
      >
        <StepTile>
          {/* 正常跑完的仍然是工具字形 —— 链上每一步默认都是「办妥了」，
              给成功的后台命令另配一枚勾会让它在一列步骤里莫名其妙地扎眼。
              只有「还在跑 / 跑砸了 / 被中断」这三档才换成状态字形。 */}
          {live ? (
            <StatusIcon kind="running" className={styles.stepIcon} />
          ) : outcome && outcome !== "completed" ? (
            <StatusIcon
              kind={statusOfOutcome(outcome)}
              className={styles.stepIcon}
            />
          ) : (
            <StepGlyph icon={CHAIN_ICON.tool} />
          )}
        </StepTile>
        {/* 工具名挂成一枚淡底小标：一列 `Browser click` / `Bash` 里，
            「这是哪个工具」比它这次的入参更先要回答。
            没有入参提示时它就是这一行的全部内容，不再另摆一枚重复的标 */}
        {step.hint ? <span className={styles.stepFrom}>{human}</span> : null}
        {/* 正跑着的那一步，名字走流光（见 `.liveText`）。**效果代替状态词**：
            「它还在动」这件事由光走过去说，而这一行的字仍然写的是「它在干什么」 */}
        <span className={`${styles.stepName} ${live ? styles.liveText : ""}`}>
          {step.hint || human}
        </span>
        {/* **跑着的时候不写「执行中…」**：这一行左边那枚 `ph:circle-notch` 正在转，
            状态已经由它说完了；再补三个字，是同一件事在 20px 内说两遍，
            而且它占的正是「这一步在干什么」该待的位置。
            语义不丢：整行带 `role="status"` ＋ `aria-label`（见上）。
            **收场不对的那两档仍然写字** —— 一个红叉说不出「跑砸了」，读屏更读不出来。
            措辞取自 `SUB_OUTCOME_LABEL`（与右栏、智能体卡同一张表），不另起一套。 */}
        {!live && outcome && outcome !== "completed" ? (
          <span className={styles.stepStatus}>{SUB_OUTCOME_LABEL[outcome]}</span>
        ) : null}
        <Icon
          icon={open ? "ph:caret-up" : "ph:caret-down"}
          className={styles.stepCaret}
        />
      </button>

      {open ? (
        <div className={styles.stepBody}>
          {/* 原始工具名只在展开后给 —— 收起时那一行要读起来像句话 */}
          <div className={styles.stepLabel}>工具</div>
          <pre className={styles.stepCode}>{step.name || "—"}</pre>
          {step.hint ? (
            <>
              <div className={styles.stepLabel}>入参</div>
              <pre className={styles.stepCode}>{step.hint}</pre>
            </>
          ) : null}
          {step.results.length ? (
            <>
              <div className={styles.stepLabel}>返回</div>
              {step.results.map((r) => (
                <ResultBlock
                  key={r.key}
                  text={r.text}
                  open={!!expanded[r.key]}
                  onToggle={() => toggleExpand(r.key)}
                />
              ))}
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
};

/** 跑的时候留在链外的步数（在跑的那一条 ＋ 它前面两条），照 VitaAgent `:342` */
const RECENT_STEPS = 3;

/**
 * 一个子代理的链默认摊开多少步。
 *
 * 一个子代理跑一两百步很常见（本机实测单条会话 149 / 132 条子任务）。全渲染出来是
 * 几百个节点，这一屏会明显卡。40 步足够看清它在干什么，其余收在一颗「展开全部」
 * 后面 —— 分页/虚拟滚动在这儿是杀鸡用牛刀：展开全部是低频动作，点了才付那份代价。
 * 数目照 VitaAgent `MessageList/index.tsx:444`。
 *
 * **留头还是留尾不一样**（这一条是 VitaAgent 没有的）：跑完的那条留头 40 步，
 * 跑着的那条留**尾** 40 步 —— 理由见下面那段截断代码。
 */
const SUB_STEP_CAP = 40;

/**
 * 跑着的子代理，展开的那条子链多久自动刷一次。
 *
 * 这是个**监控工具**，用户这一轮最主要的抱怨就是「执行中看不到正在执行的内容」——
 * 点开一个正在跑的子代理却只看到一张静止快照，等于把那个问题又演一遍。
 *
 * 但也只刷这一种：正文是**现去那台机器读磁盘**取回来的，跑完的那些内容不会再变，
 * 跟着刷等于让客户端反复从头读 jsonl。所以三个条件缺一不可 ——
 * 子代理 `outcome === "running"`、它的子链**正展开着**、5 秒一次。
 */
const SUB_REFRESH_MS = 5000;

/** 子代理正文没取到的原因 → 那一行怎么说。与 ChatPane 的空态同一套措辞 */
const SUB_FAIL_TEXT: Record<string, string> = {
  offline: "设备离线，读不到这个子代理的内容",
  missing: "找不到这个子代理的会话记录",
  network: "读取失败，请检查网络",
};

/** 链上一串节点的渲染。主链与子代理的链共用这一份 —— 形制必须一模一样 */
const ChainNodes: React.FC<{
  items: ChainItem[];
  /** 这一批节点属于哪条会话（拉子代理正文要用） */
  taskId: string;
  /** 还在跑的那一步（只有主链有） */
  runningKey?: string;
  /** 点开的那张子代理小卡：链项 key → agentId。**存在链这一层**，理由见 AgentCard */
  picked: Record<string, string>;
  onPick: (itemKey: string, agentId: string) => void;
  expanded: Record<string, boolean>;
  toggleExpand: (key: string) => void;
  renderNote: (m: PortalMessage, key: string) => React.ReactNode;
}> = ({
  items,
  taskId,
  runningKey,
  picked,
  onPick,
  expanded,
  toggleExpand,
  renderNote,
}) => (
  <>
    {items.map((it) => {
      if (it.kind === "note") {
        return (
          <div key={it.key} className={styles.chainNote}>
            {/* 旁白也是链上的一格，也得有自己的记号 —— 原来它只有一个 28px 的
                左缩进、图标位空着，于是那根贯穿的竖线在这一格没有色块遮断，
                直接从空白里穿过去，一列节点里只有它像掉了一格。
                字形就是 VitaAgent `NoteBlock` 那一枚 `ph:chat-teardrop-text` ——
                这一格说的是「模型一边干活一边说的话」，不是一步操作。 */}
            <StepTile className={styles.noteTile}>
              <StepGlyph icon={CHAIN_ICON.note} />
            </StepTile>
            {renderNote(it.msg, it.key)}
          </div>
        );
      }
      if (it.kind === "agents") {
        const on = picked[it.key] ?? "";
        /* 卡片那一格与它点开的那条子链是**兄弟节点**（同为链体的直接子元素），
           那根贯穿的竖线因此一路穿下去不断口 —— 而不是在卡片内部另起一块
           带内滚的明细面板（VitaAgent 正是从那个形态改过来的，
           三个毛病记在 `MessageList/index.tsx:451-467`） */
        return (
          <React.Fragment key={it.key}>
            <div className={`${styles.step} ${styles.stepTask}`}>
              <AgentCard
                goal={agentsGoal(it)}
                agents={it.agents}
                picked={on}
                onPick={(agentId) => onPick(it.key, agentId)}
              />
            </div>
            {on ? (
              <SubAgentChain
                parentId={taskId}
                agentId={on}
                live={
                  it.agents.find((a) => a.id === on)?.outcome === "running"
                }
                expanded={expanded}
                toggleExpand={toggleExpand}
                renderNote={renderNote}
              />
            ) : null}
          </React.Fragment>
        );
      }
      return (
        <StepRow
          key={it.key}
          step={it}
          running={it.key === runningKey}
          open={!!expanded[it.key]}
          onToggle={() => toggleExpand(it.key)}
          expanded={expanded}
          toggleExpand={toggleExpand}
        />
      );
    })}
  </>
);

/**
 * 某个子代理**自己走过的那条链**，作为链上的节点接在智能体卡之后。
 *
 * 它渲染出来的就是 `.step`，与卡片那一格同为链体的直接子元素 —— 不是抽屉、
 * 不是弹层、也不是卡内滚动面板。长内容进正常文档流，跟着整页滚。
 *
 * 正文要 hub 点名让那台机器现读磁盘，一次往返两轮上报：`pending` 期间显示
 * 「读取中」并自动重试，**不许当成空**（见 PortalStore.loadSubAgentMessages）。
 */
const SubAgentChain: React.FC<{
  parentId: string;
  agentId: string;
  /** 这个子代理**还在跑**。只有它为真时才自动刷新（见 SUB_REFRESH_MS） */
  live: boolean;
  expanded: Record<string, boolean>;
  toggleExpand: (key: string) => void;
  renderNote: (m: PortalMessage, key: string) => React.ReactNode;
}> = observer(({ parentId, agentId, live, expanded, toggleExpand, renderNote }) => {
  const {
    subAgentMessagesOf,
    isSubAgentLoading,
    isSubAgentPending,
    subAgentFailOf,
    loadSubAgentMessages,
  } = PortalStore;
  /** 「展开全部」是纯视图态，换一张卡就回到默认（组件随 picked 变化挂载/卸载） */
  const [all, setAll] = useState(false);

  useEffect(() => {
    loadSubAgentMessages(parentId, agentId);
  }, [parentId, agentId, loadSubAgentMessages]);

  /* 跑着的时候自动刷新。
     **定时器只挂在这个组件上**：子链一收起（`picked` 清空）这个组件就卸载，
     cleanup 把定时器清掉；换会话、关格子同理。一个展开的子链一个定时器，
     不会留在后台空转。
     `live` 翻成 false（子代理收尾了）时 cleanup 先清定时器，然后**再拉最后一次**
     —— 最后几步与它交回的结论就是在那一刻落盘的，不补这一次会永远停在倒数第二步。 */
  const wasLive = useRef(false);
  useEffect(() => {
    if (!live) {
      if (wasLive.current) {
        wasLive.current = false;
        loadSubAgentMessages(parentId, agentId, true);
      }
      return;
    }
    wasLive.current = true;
    const timer = window.setInterval(
      () => loadSubAgentMessages(parentId, agentId, true),
      SUB_REFRESH_MS,
    );
    return () => window.clearInterval(timer);
  }, [live, parentId, agentId, loadSubAgentMessages]);

  const msgs = subAgentMessagesOf(parentId, agentId);
  const loading = isSubAgentLoading(parentId, agentId);
  const pending = isSubAgentPending(parentId, agentId);
  const fail = subAgentFailOf(parentId, agentId);

  const hint = (icon: React.ReactNode, text: string, retry?: boolean) => (
    <div className={styles.step}>
      <div className={styles.stepHead}>
        {icon}
        <span className={styles.stepName}>{text}</span>
        {retry ? (
          <span
            className={styles.stepRetry}
            role="button"
            tabIndex={0}
            onClick={() => loadSubAgentMessages(parentId, agentId, true)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                loadSubAgentMessages(parentId, agentId, true);
              }
            }}
          >
            重试
          </span>
        ) : null}
      </div>
    </div>
  );

  if (!msgs.length) {
    if (loading || pending) {
      return hint(
        <StepTile>
          <StatusIcon kind="running" className={styles.stepIcon} />
        </StepTile>,
        pending ? "正在从那台机器读取这个子代理的内容…" : "读取中…",
      );
    }
    if (fail) {
      // 离线是可恢复的，给一条出路；不自动轮询（那台机器可能关了一整晚）
      return hint(
        <StepTile>
          <StatusIcon kind="failed" className={styles.stepIcon} />
        </StepTile>,
        SUB_FAIL_TEXT[fail] ?? "读取失败",
        true,
      );
    }
    return hint(
      <StepTile>
        <StepGlyph icon={CHAIN_ICON.tool} />
      </StepTile>,
      "这个子代理没有留下可展示的内容",
    );
  }

  /* 子代理的链**整段都是过程**：结论已经由父会话并回主对话了，
     所以这里不再切「链外的结论」那一刀（flat）。
     key 前缀带上 agentId：两条链的消息时间戳可能撞，展开态会串到别的行上 */
  const { chain } = buildChain(
    msgs.map((m) => ({ m, k: `${agentId}|${msgKey(m)}` })),
    {
      subByToolUse: EMPTY_SUB_MAP,
      // 子代理那条链上的后台命令属于**它自己的**子任务清单，这里没有那份数据
      // （按需只拉了正文），所以不补收场 —— 空着比错标一个强。
      bgByToolUse: EMPTY_SUB_MAP,
      split: { flat: true },
    },
  );

  /* 按「步」截断，不按链项：旁白是围着某一步说的话，跟着它一起留下。
     **留头还是留尾，看它还在不在跑**：
       跑完了 → 留头 40 步。那条链不会再长，「它是怎么开的头」才是要看的。
       跑着呢 → **留尾 40 步**，被截掉的是更早的那些。新步骤是从尾巴上长出来的，
                留头等于把最新进展永久挡在按钮后面 —— 而这条子链每 5 秒刷一次
                （见 SUB_REFRESH_MS）就是为了看它此刻到哪一步了，两者是一回事。
     两档都保留「展开全部」，折叠能力没有被删掉。 */
  const stepAt = chain
    .map((it, i) => (isStep(it) ? i : -1))
    .filter((i) => i >= 0);
  const over = !all && stepAt.length > SUB_STEP_CAP;
  /** 截掉的那几步在头上（跑着的那条）还是在尾上（跑完的那条） */
  const cutHead = over && live;
  const from = cutHead ? stepAt[stepAt.length - SUB_STEP_CAP] : 0;
  const to = over && !live ? stepAt[SUB_STEP_CAP] : chain.length;
  const shown = chain.slice(from, to);
  const rest = over ? stepAt.length - SUB_STEP_CAP : 0;

  /** 「还有 N 步」那一行。跑着的那条摆在**上面**（截掉的是更早的那些） */
  const more =
    rest > 0 ? (
      <div className={styles.step}>
        <button
          type="button"
          className={styles.stepHead}
          onClick={() => setAll(true)}
        >
          <StepTile>
            <StepGlyph icon={CHAIN_ICON.more} />
          </StepTile>
          <span className={styles.stepName}>
            {cutHead ? `更早还有 ${rest} 步，展开全部` : `还有 ${rest} 步，展开全部`}
          </span>
        </button>
      </div>
    ) : null;

  return (
    <>
      {cutHead ? more : null}
      <ChainNodes
        items={shown}
        taskId={parentId}
        picked={EMPTY_PICKED}
        onPick={noop}
        expanded={expanded}
        toggleExpand={toggleExpand}
        renderNote={renderNote}
      />
      {cutHead ? null : more}
    </>
  );
});

/** 子代理那条链里不再画嵌套的智能体卡：子任务清单只覆盖父会话这一层 */
const EMPTY_SUB_MAP: Map<string, SubTask> = new Map();
const EMPTY_PICKED: Record<string, string> = {};
const noop = () => undefined;

/**
 * 一轮的执行链：一条竖线时间轴，**结论不在里面**。
 *
 * 折叠策略照 VitaAgent `MessageList/index.tsx:316-360`：
 *
 *   跑完了 → 整条链收成一行摘要（`起了 3 个子代理，跑了 6 条命令`），默认收起。
 *            那时人要读的是结论，过程该让位。
 *   跑着呢 → **最近三步摊在外面**、更早的收进摘要行。那会儿用户盯的正是
 *            「现在到哪一步了」，全折起来等于把它在干什么藏了。
 *
 * 再压一条，它盖过上面两条：**链里还有子代理在跑，就从它那一格起一律摊在外面**。
 * 子代理是异步派出去的，父会话写完结论收工时它照样在跑 —— 只按「这一轮跑完没有」
 * 收链的话，结论一出来那张转着的卡就被埋进摘要行里。折叠是给「已经结束、可以不看」
 * 的内容用的，在跑的东西必须一直看得见。判据见 `hasLiveAgent`（结构性，不看时间）。
 *
 * 与 VitaAgent 的偏离（`MessageList/index.tsx:655-693`）：那边的判据只有一条
 * 「链外有没有结论」（`bodiless`），编排任务在后台接着跑那段全靠「结论还没落库」
 * 顺带盖住，结论一落库链照样收起。那条通路上任务卡另有入口，这里没有 ——
 * 子代理只能从链里看到，所以这条必须自己成立，不能搭结论的便车。
 *
 * 摘要行为空（一次工具都没调）时整条链不成立，调用方直接按正文渲染 ——
 * 否则跑完之后这一轮会渲染成一个空的折叠块，内容凭空消失。
 */
const ExecChain: React.FC<{
  items: ChainItem[];
  taskId: string;
  live?: boolean;
  open: boolean;
  onToggle: () => void;
  expanded: Record<string, boolean>;
  toggleExpand: (key: string) => void;
  /**
   * 点开的是哪张子代理小卡：链项 key → agentId。
   *
   * 卡片与它点开的子链是两个平级的链节点，位置关系在 `ChainNodes` 里排；
   * 但**这份状态住在 `TerminalFeed`**：侧栏点一条子代理要跨轮次地
   * 「找到那张卡 → 展开它」，状态留在每一轮各自的 ExecChain 里，外面够不着。
   * 仍然是纯 useState，不落盘。
   */
  picked: Record<string, string>;
  onPick: (itemKey: string, agentId: string) => void;
  renderNote: (m: PortalMessage, key: string) => React.ReactNode;
  /**
   * 接在链尾的那一格（「正在思考…/正在处理…」那一行）。
   *
   * **必须渲染在 `.chainBody` 里面**，它才是链上的一格：那根贯穿的竖线从上一格穿
   * 下来、在它的图标槽处收住，间距与步骤之间一样是 0。此前它由调用方渲染在链的
   * **外面**（`.agent` 的直接子元素），于是 `.agent` 的 gap 10 ＋ `.chain + *` 的
   * margin-top 10 在中间撑出 20px 空当，线也接不上 —— 看着像另起的一块东西。
   *
   * **不进 `items`**：它不是链上的一步（没有调用、没有输出、不参与摘要计数与
   * 「最近三步」窗口），只是钉在尾巴上的一个当前状态。混进 `items` 会把
   * `summarizeChain` 与折叠窗口一起带偏。
   */
  tail?: React.ReactNode;
}> = ({
  items,
  taskId,
  live,
  open,
  onToggle,
  expanded,
  toggleExpand,
  picked,
  onPick,
  renderNote,
  tail,
}) => {
  /* 留在外面的按「步」数，不按「链项」数：旁白是围着某一步说的话，
     跟着它一起留在外面。按链项切的话，一段长旁白就能把窗口占满，
     屏幕上只剩一条步骤 —— VitaAgent `:336-341` 记的就是这个实测 */
  const stepAt = items
    .map((it, i) => (isStep(it) ? i : -1))
    .filter((i) => i >= 0);
  const firstKeep =
    stepAt.length > RECENT_STEPS ? stepAt[stepAt.length - RECENT_STEPS] : 0;

  /* **还有子代理在跑 → 从它那一格起一律摊在外面。**
     这一轮跑完（`live` 翻假）就把整条链收成一行，是这一版特意要的行为 ——
     但它此前只看「这一轮有没有结论」，不看链里有没有东西还在跑：子代理是异步派出去的，
     父会话写完结论收工时它照样在跑，于是结论一落库整条链收起、那张转着的卡跟着被埋掉
     （用户原话：「结果发出来了，但是子会话还在执行，现在执行链会被收起」）。
     判据因此改成**复合**的：有结论就收起**仍然成立**，但只收到「最早那个还在跑的
     子代理」为止，它和它之后的一切留在外面。跑完的那些照旧收进摘要行。 */
  const liveAgentAt = items.findIndex(hasLiveAgent);
  /** 自动摊开的起点（`open` = 用户手动展开，整条都摊开，不走这里） */
  const autoFrom = live
    ? liveAgentAt >= 0
      ? Math.min(firstKeep, liveAgentAt)
      : firstKeep
    : liveAgentAt >= 0
      ? liveAgentAt
      : items.length; // 跑完了、也没有在跑的子代理 → 整条收起
  const hidden = open ? 0 : autoFrom;
  const shown = open ? items : items.slice(hidden);
  /** 头一行概括谁：收起时是收进去的那些，展开了是整条链 */
  const headText = summarizeChain(open ? items : items.slice(0, hidden));
  /** 摘要行下面还留着东西 —— 那两块要拉开距离，理由见下面的 `chainLive` */
  const someOutside = !open && hidden > 0 && shown.length > 0;

  /** 还在跑的那一步（判据见 `pendingCallKey`；`Working` 那一行与它互斥） */
  const runningKey = live ? pendingCallKey(items) : "";

  return (
    <div className={styles.chain}>
      {/* 刚开跑、还没有东西可收的时候不摆这一行 */}
      {headText ? (
        <button
          type="button"
          className={styles.chainHead}
          aria-expanded={open}
          onClick={onToggle}
        >
          <span className={styles.chainText}>{headText}</span>
          <Icon
            icon={open ? "ph:caret-up" : "ph:caret-down"}
            className={styles.stepCaret}
          />
        </button>
      ) : null}
      {/* 跑的时候「还在外面的那几条」要和上面那句摘要拉开距离：它们是两种东西 ——
          上面那行是「已经收进去的」，下面这几条是「还在外面的」。
          贴着排的话看上去就成了「摘要展开后的内容」，正好是反的 */}
      <div
        className={`${styles.chainBody} ${
          someOutside && headText ? styles.chainLive : ""
        }`}
      >
        <ChainNodes
          items={shown}
          taskId={taskId}
          runningKey={runningKey}
          picked={picked}
          onPick={onPick}
          expanded={expanded}
          toggleExpand={toggleExpand}
          renderNote={renderNote}
        />
        {/* 链尾那一格（理由见 `tail`）。摆在 `ChainNodes` **之后、同一个
            `.chainBody` 里面** —— 它是链上的最后一格，不是链外面的另一块 */}
        {tail}
      </div>
    </div>
  );
};

const TerminalFeed: React.FC<TerminalFeedProps> = (props) => {
  const {
    taskId,
    messages,
    subTasks,
    running,
    providerDsr,
    imageCtx,
    focusAgent,
    onFocusAgent,
  } = props;
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  /** 点开的是哪张子代理小卡：链项 key → agentId（理由见 ExecChain 的同名 prop） */
  const [picked, setPicked] = useState<Record<string, string>>({});
  const rootRef = useRef<HTMLDivElement>(null);

  const turns = toTurns(messages);

  /* 「哪次工具调用派出了哪个子代理」的索引。**只认 `toolUseId`**：
     拿不到的（老记录、或起跑记录落在重放窗口之外）就配不上，那一步照普通
     工具调用画 —— 不退回按 label / 中文文案凑，两边截断长度不同必然错配。

     **不套 useMemo**：`subTasksOf` 现在是两份合并出来的新数组（见 PortalStore），
     引用每次都变，memo 只会每帧重算一遍再多存一份；清单最多一两百条，直接建。 */
  const subByToolUse = new Map<string, SubTask>();
  /* 「哪次 Bash 调用起了哪个后台命令」。同样只认 `toolUseId`。
     后台命令的收场只能在这儿说 —— 右栏那一节只列还在跑的（见 `runningBgCommands`），
     跑砸的、被中断的就落在链上它发生的那一步。 */
  const bgByToolUse = new Map<string, SubTask>();
  (subTasks ?? []).forEach((t) => {
    if (!t.toolUseId) {
      return;
    }
    if (t.kind === "agent") {
      subByToolUse.set(t.toolUseId, t);
    } else {
      bgByToolUse.set(t.toolUseId, t);
    }
  });

  const toggleExpand = (key: string) => {
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));
  };
  const onPick = (itemKey: string, agentId: string) =>
    setPicked((prev) => ({ ...prev, [itemKey]: agentId }));

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

  /**
   * 每一轮拆成「过程进链、结论进正文」的结果。
   *
   * **算在 render 里、两处共用**：一处是下面的渲染，另一处是侧栏定位那个副作用
   * （它要知道某个子代理落在哪一轮的哪一格上）。两边各算一遍的话，
   * 「哪一步是哪一格」这个判据迟早分叉。
   *
   * 分界线照 VitaAgent `MessageList/index.tsx:601-637`：
   *
   *   跑完了 → **最后一段产出（assistant / plan）就是结论**，留在正文；
   *            它之前的一切都是过程，收进链。不能按「最后一次工具调用」切：
   *            一轮若以工具结尾（问路那种），它前面那段正文恰恰是说给人听的话，
   *            按那个判据整条回答会一个字都不剩。
   *   跑着呢 → **最后一步过程之后的正文才是「正在写的那段」**，之前的每段正文
   *            都是旁白，按发生顺序收进链。沿用跑完那条的话，旁白会被留在链的
   *            **下面**、而它引出的那几步却在链里 —— 顺序整个反了。
   */
  const turnChains = turns.map((turn, ti) => {
    // 执行中的活动轮：不铺工具流水，只同步 Q&A（助手文本），
    // 工具过程收进折叠块，需要时再展开。
    const inProgress = !!running && ti === lastActive;
    // 用内容指纹做 key：执行中 → 完成态切换时 key 不变，避免整块重挂载闪烁；
    // 且不随消息裁剪而漂移（下标会）。
    const keyed = turn.items.map((m) => ({ m, k: msgKey(m) }));
    const { chain, body } = buildChain(keyed, {
      subByToolUse,
      bgByToolUse,
      split: { flat: false, inProgress },
    });
    return {
      turn,
      keyed,
      inProgress,
      chain,
      body,
      ckey: `chain|${turn.key}`,
      /** 这一轮从什么时候开始：有用户消息就用它，否则用首条产出。落位要用（见下） */
      startMs: atMs(turn.user?.timestamp ?? turn.items[0]?.timestamp),
    };
  });

  /* **配不上任何一次派活记录的子代理，按时间插回链里。**
     子代理存不存在由清单说了算，消息流只决定它插在哪一步 —— 理由与落位规则见
     `attachLooseAgents`。就地改上面那几条链（都是这一帧现建的数组，没有别人看着）。 */
  attachLooseAgents(
    turnChains,
    (subTasks ?? []).filter((t) => t.kind === "agent"),
  );

  /* ---------- 侧栏「点一条子代理 → 滚到它那张卡」的落地 ---------- */

  /** 卡片已经展开、等这一帧提交完去取元素的那次请求 */
  const [focusHit, setFocusHit] = useState<{
    agentId: string;
    seq: number;
  } | null>(null);
  const focusSeq = focusAgent?.seq ?? 0;
  const focusId = focusAgent?.agentId ?? "";

  /* 第一步：把那张卡所在的链展开、把那张小卡点上。
     依赖里带 `messages`：正文是异步拉回来的，头一次跑的时候链可能还是空的，
     消息一到这个副作用就再试一次 —— 不靠定时器轮询猜「加载好了没有」。 */
  useEffect(() => {
    if (!focusId) {
      return;
    }
    let hit: { ckey: string; itemKey: string } | null = null;
    for (const tc of turnChains) {
      for (const it of tc.chain) {
        if (it.kind === "agents" && it.agents.some((a) => a.id === focusId)) {
          hit = { ckey: tc.ckey, itemKey: it.key };
          break;
        }
      }
      if (hit) {
        break;
      }
    }
    if (!hit) {
      /* 找不到。`attachLooseAgents` 之后**只剩一种可能**：这一格连一条正文都还没有
         （因此一条链都没有，无处可插）。派活记录掉出窗口那一类已经不会再走到这儿。
         **怎么说由调用方决定** —— 正文还在路上的时候不该报「找不到」。 */
      onFocusAgent?.(null, focusSeq);
      return;
    }
    const { ckey, itemKey } = hit;
    // 跑完的那一轮整条链默认收着（`shown` 是空数组），不展开就什么都渲染不出来
    setExpanded((prev) => (prev[ckey] ? prev : { ...prev, [ckey]: true }));
    setPicked((prev) =>
      prev[itemKey] === focusId ? prev : { ...prev, [itemKey]: focusId },
    );
    setFocusHit({ agentId: focusId, seq: focusSeq });
    // turnChains 每次渲染都是新数组，不能进依赖；它由 messages / subTasks 决定
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusId, focusSeq, messages, subTasks]);

  /* 第二步：上面那几个 setState 提交之后，卡片一定已经在 DOM 里了，这时才去取元素。
     用副作用的提交顺序保证「渲染好了」，不靠 setTimeout 猜。 */
  useEffect(() => {
    if (!focusHit) {
      return;
    }
    const el = rootRef.current?.querySelector<HTMLElement>(
      `[data-agent-id="${CSS.escape(focusHit.agentId)}"]`,
    );
    onFocusAgent?.(el ?? null, focusHit.seq);
    setFocusHit(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusHit]);

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

    // 工具调用与命令输出不走这里：它们是过程，一律进执行链（见 ExecChain）。
    // 只有链才认得「一次调用 ＋ 它的输出」这对关系，摊成两行平铺是上一版的形态。

    // 模型正文：比例字 + 共享 markdown 基线，平铺，不带卡片
    return (
      <div key={key} className={styles.assistantText}>
        <SessionMarkdown content={m.content} imageCtx={imageCtx} />
      </div>
    );
  };

  return (
    <div className={styles.TerminalFeed} ref={rootRef}>
      {turnChains.map((tc) => {
        const { turn, keyed, inProgress, chain, body, ckey } = tc;
        /** 此刻挂起着的那一步。与末尾那一行互斥 —— 同一份判据，见 pendingCallKey */
        const pendingKey = inProgress ? pendingCallKey(chain) : "";
        /* 「正在思考…/正在处理…」那一行**只有一个**，位置两选一（理由见下面那段注释
           与 `Working` 的说明）：链是这一轮此刻的最后一件事 → 进链当最后一格；
           否则（模型之后又写了话 / 这一轮压根没有链）→ 落在正文后面，不画竖线。 */
        const liveInChain = chain.some(isStep) && body.length === 0;
        const live =
          inProgress && !pendingKey ? (
            <Working
              // 起点取这一轮的起始时刻：有用户消息就用它，否则退回首条产出
              since={turn.user?.timestamp ?? turn.items[0]?.timestamp}
              thinking={keyed.length === 0}
              loose={!liveInChain}
            />
          ) : null;
        // 执行中也铺，但**过程一律折叠着**（ExecChain 会把它并成一行
        // 「执行过程 · N 步」）—— 一条条冒出来是噪音、还不停把视图往下推，
        // 但整段藏掉又会让人不知道它在干什么。折叠着实时长，想看点开即可。
        //
        // （清单与后台任务是「当前状态」，已由 ChatPane 抽成单独的状态卡，
        // 挂在对话流末尾；选择卡同理挂在输入框上方，都不进内容流。）
        return (
          <div key={turn.key} className={styles.turn}>
            {/* 对话流只管「我说了什么」。排队状态与撤回一律交给对话流末尾的排队卡 ——
                两处都摆一份的话，同一条任务在正文和排队卡各显示一遍，还得为了去重
                把正文里的消息藏起来，于是「我发的内容在对话流里不见了」。
                职责分开之后，正文永远是完整的对话记录。 */}
            {turn.user
              ? (() => {
                  const uKey = `u|${turn.key}`;
                  return (
                    <div className={styles.userRow}>
                      <div className={styles.userBubble}>
                        <ClampBox
                          open={!!expanded[uKey]}
                          onToggle={() => toggleExpand(uKey)}
                        >
                          {turn.user.content}
                        </ClampBox>
                      </div>
                      <div className={styles.userTime}>
                        {fmtTime(turn.user.timestamp)}
                      </div>
                    </div>
                  );
                })()
              : null}

            {(keyed.length > 0 || inProgress) && (
              <div className={styles.agent}>
                {/* 一次工具都没调、也没派过子代理：整轮都是正文，不摆链。
                    摆的话摘要行是空字符串、跑完之后 `shown` 又是空数组，
                    这一轮会渲染成一个什么都没有的空块 —— 内容凭空消失。 */}
                {chain.some(isStep)
                  ? [
                      <ExecChain
                        key="chain"
                        items={chain}
                        taskId={taskId}
                        live={inProgress}
                        open={!!expanded[ckey]}
                        onToggle={() => toggleExpand(ckey)}
                        expanded={expanded}
                        toggleExpand={toggleExpand}
                        picked={picked}
                        onPick={onPick}
                        renderNote={renderItem}
                        // 链就是这一轮此刻的最后一件事 → 那一行进链，当它的最后一格
                        tail={liveInChain ? live : null}
                      />,
                      ...body.map(({ m, k }) => renderItem(m, k)),
                    ]
                  : keyed.map(({ m, k }) => renderItem(m, k))}
                {/* **跑着的这一轮，屏幕上永远恰好有一处在动。**
                    判据是结构性的、两档互斥（见 `pendingCallKey` 与 `Working`）：
                      有一步挂起着 → 不摆这一行，由那一步的名字走流光 ＋ 图标转圈
                                     （不加状态词：那一行已经写着在干什么）；
                      一步都没挂起 → 摆这一行，此刻它是屏幕上唯一在动的东西。
                    文案同样按结构分：这一轮一个节点都还没有 = 模型还没开始动手，
                    写「正在思考…」；已经有产出 = 写「正在处理…」。
                    不看文本长度、不看时间阈值。
                    改前这一行只在 `keyed.length === 0` 时出现 —— 实测那个窗口通常
                    只有一两秒，而「上一步已返回、下一步还没发起」的空当里屏幕上
                    一个动的东西都没有，跑着的会话看起来和卡死的一模一样。

                    **位置**：它待在「到此为止发生的最后一件事」后面 —— 链是最后
                    一件事就由 `ExecChain` 的 `tail` 把它渲染进链里（竖线接上、
                    间距 0）；模型之后又写了话，或这一轮压根没有链，才落在这儿，
                    那时它不属于任何一条链，`loose` 把那截连不到任何地方的线头撤掉。 */}
                {liveInChain ? null : live}
                {/* 落款：来源代理 + 时间。原先挂在终端卡的标题栏上，卡片撤掉之后
                    这两样仍要有地方待着 —— 时间是回看时定位用的。 */}
                {keyed.length > 0 ? (
                  <div className={styles.turnMeta}>
                    <span className={styles.turnProvider}>
                      {providerDsr || "终端"}
                    </span>
                    <span>
                      {fmtTime(
                        turn.items[turn.items.length - 1]?.timestamp ??
                          turn.user?.timestamp,
                      )}
                    </span>
                  </div>
                ) : null}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};

/*
 * TODO(am-core): 执行链还缺两样后端此刻不下发的东西，都要改 `MessageBrief`
 * （`agent-task-monitor/core/src/model.rs:77`）＋ `entry_to_brief`
 * （`core/src/scanner.rs:1727` 起）才能补上（「每步成败」已经接通，
 * 走 `MessageBrief::is_error` → `PortalMessage.isError` → `.stepBad`）：
 *
 *   1. **每步耗时**。原始 jsonl 里根本没有：`durationMs` 只挂在
 *      `type=system, subtype=turn_duration` 上（**整轮**的耗时），
 *      `toolUseResult` 里一个时间字段都没有。拿「工具那条记录的时间戳」减
 *      「结果那条记录的时间戳」不成立 —— 实测最近 8 个会话 891 对里，
 *      `Agent` 全部落在 0.02–0.1s（子代理实际跑几分钟），`Bash` 中位数 0.08s
 *      而最小 0.01s（一个 shell 都起不来）。它量的是记录落盘的间隔，不是执行时间。
 *      要真耗时得后端在调用与结果配对时自己计时并下发 `durationMs`。
 *   2. **多次调用的拆分**。同一条 assistant 记录里的多次 tool_use 被
 *      `tools.join(" | ")` 拼成一条（`scanner.rs:1841`），前端没法安全拆开
 *      （命令自己就可能带管道）。该出成结构化数组而不是拼字符串。
 *
 * 思考过程（VitaAgent 单独一档）**这一版不做，也不建议做**：原始 jsonl 里
 * 有 `thinking` 块，但实测 25 个会话 1339 个块里 1326 个的 `thinking` 是空串
 * （只剩加密的 `signature`）—— 能显示内容的不到 1%，做出来的是一排空壳。
 */

export default TerminalFeed;
