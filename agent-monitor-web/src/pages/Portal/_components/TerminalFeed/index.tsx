import React, { useEffect, useState } from "react";

import dayjs from "dayjs";
import { Icon } from "@hsu-react/ui";
import SessionMarkdown from "../SessionMarkdown";
import { SessionImageCtx } from "@/utils/sessionImages";

import { PortalMessage, SelectPayload } from "@/services/apis/portal";
import { fmtElapsed } from "../../_utils/sessionState";
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
      p.includes(n) ? p.filter((x) => x !== n) : [...p, n].sort((a, b) => a - b)
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
 * 一条 `role=tool` 的内容拆成「工具名 ＋ 这次拿它干了什么」。
 *
 * 不是猜的格式：后端就是 `format!("{name}: {hint}")` 拼出来的
 * （am-core `scanner.rs:1806-1811` 的 claude 链路、`:1699-1705` 的 codex 链路），
 * 而工具名里不可能出现 `": "` —— 所以按**第一个** `": "` 切一刀就是原样还原。
 *
 * 切不到就整条当工具名（没有入参提示的调用，如 `ListAgents`）。
 *
 * **不按 `" | "` 再切成多次调用**：后端确实用它连接同一条记录里的多次调用
 * （`scanner.rs:1841` 的 `tools.join(" | ")`），但入参提示本身可以带管道
 * （`Bash: ls | grep x`），按字面切会把一条命令劈成两步。实测最近 8 个会话的
 * 891 次调用**全部**是「一条记录一次调用」，所以这里不冒那个险 ——
 * 真要拆开，该由后端出结构化数组（见文件末尾的 TODO）。
 */
const splitToolLine = (content: string) => {
  const i = content.indexOf(": ");
  if (i < 0) return { name: content.trim(), hint: "" };
  return {
    name: content.slice(0, i).trim(),
    hint: content.slice(i + 2).trim(),
  };
};

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

/** 执行链里的一步：一次工具调用（带它的输出），或模型干活途中说的一段话 */
interface ChainCall {
  kind: "call";
  key: string;
  /** 原始工具名，展开后照原样给 */
  name: string;
  /** 入参提示（命令 / 路径 / 描述），后端 `tool_input_hint` 挑出来的那一个 */
  hint: string;
  /** 这次调用的输出。一条调用可能跟着多条输出记录 */
  results: { key: string; text: string; bad?: boolean }[];
}
interface ChainNote {
  kind: "note";
  key: string;
  msg: PortalMessage;
}
type ChainItem = ChainCall | ChainNote;

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

  const clauses: string[] = [];
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
 * 一轮执行中的秒表：「正在处理… · 7秒」。
 *
 * 原先只有「执行中… + 已 N 步」——步数在两次工具调用之间是不动的，一段长
 * 推理里它能十几秒纹丝不动，看着和卡死没有区别。**耗时是「还在跑」与
 * 「卡住了」的唯一区别**，所以它每秒走字。
 *
 * 形态与链上的步骤行统一（`.step`）：它就是这条链的最后一行 ——
 * 照 VitaAgent `MessageList/index.tsx:158-186` 的 `Working`，
 * 那边同样是「转圈图标 ＋ 正在处理… ＋ 耗时」这一套步骤行，
 * 挂在链与正文的下面（`:1068`）。
 *
 * 「最近动作」不再单列：链上摊着的最后三步已经把它说得更清楚，
 * 再在这里印一遍就是同一句话说两回。
 *
 * 口径与 SessionPanels / SubAgentChip 共用 `fmtElapsed`：起点取这一轮的
 * 起始时间戳（会话记录里的时刻），全项目只有这一套算法。
 */
const Working: React.FC<{
  since?: string;
  steps: number;
}> = ({ since, steps }) => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const elapsed = fmtElapsed(since, now);

  return (
    <div className={styles.step}>
      <div className={styles.stepHead}>
        <Icon
          icon="LoadingOutlined"
          className={`${styles.stepIcon} ${styles.stepIconLive}`}
        />
        <span className={styles.stepNameLive}>正在处理…</span>
        {elapsed ? (
          <span className={styles.stepStatus}>{elapsed}</span>
        ) : null}
        {steps > 0 ? (
          <span className={styles.stepStatus}>已 {steps} 步</span>
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
  const shown = clamped
    ? lines.slice(0, RESULT_CLAMP_LINES).join("\n")
    : text;

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
 * 右端的状态**只在真有依据时才写**：这一步还在跑（跑着的那一轮里、最后一次
 * 调用还没有任何输出）写「执行中…」。耗时与成功/失败后端没下发 ——
 * 详见文件末尾的 TODO，宁可空着也不拿两条记录的时间戳相减冒充。
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
  /* 跑砸的那一步整行走 destructive（与 SessionPanels 的失败行同一套色）：
     一列灰扑扑的步骤里，只有它是「这里出过事」—— 图标、名字、状态一起变色，
     扫一眼就能定位到出错的位置，不用逐条展开找。 */
  const bad = stepFailed(step);

  return (
    <div className={`${styles.step} ${bad ? styles.stepBad : ""}`}>
      <button
        type="button"
        className={styles.stepHead}
        aria-expanded={open}
        onClick={onToggle}
      >
        <Icon
          icon={
            running
              ? "LoadingOutlined"
              : bad
                ? "CloseCircleFilled"
                : "ToolOutlined"
          }
          className={`${styles.stepIcon} ${running ? styles.stepIconLive : ""}`}
        />
        {/* 工具名挂成一枚淡底小标：一列 `Browser click` / `Bash` 里，
            「这是哪个工具」比它这次的入参更先要回答。
            没有入参提示时它就是这一行的全部内容，不再另摆一枚重复的标 */}
        {step.hint ? (
          <span className={styles.stepFrom}>{human}</span>
        ) : null}
        <span className={styles.stepName}>{step.hint || human}</span>
        {running ? <span className={styles.stepStatus}>执行中…</span> : null}
        {bad ? <span className={styles.stepStatus}>失败</span> : null}
        <Icon
          icon={open ? "UpOutlined" : "DownOutlined"}
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
 * 一轮的执行链：一条竖线时间轴，**结论不在里面**。
 *
 * 折叠策略照 VitaAgent `MessageList/index.tsx:316-360`：
 *
 *   跑完了 → 整条链收成一行摘要（`跑了 6 条命令，查了 3 次资料`），
 *            默认收起。那时人要读的是结论，过程该让位。
 *   跑着呢 → **最近三步摊在外面**、更早的收进摘要行。那会儿用户盯的正是
 *            「现在到哪一步了」，全折起来等于把它在干什么藏了。
 *
 * 摘要行为空（一次工具都没调）时整条链不成立，调用方直接按正文渲染 ——
 * 否则跑完之后这一轮会渲染成一个空的折叠块，内容凭空消失。
 */
const ExecChain: React.FC<{
  items: ChainItem[];
  live?: boolean;
  open: boolean;
  onToggle: () => void;
  expanded: Record<string, boolean>;
  toggleExpand: (key: string) => void;
  renderNote: (m: PortalMessage, key: string) => React.ReactNode;
}> = ({
  items,
  live,
  open,
  onToggle,
  expanded,
  toggleExpand,
  renderNote,
}) => {
  /* 留在外面的按「步」数，不按「链项」数：旁白是围着某一步说的话，
     跟着它一起留在外面。按链项切的话，一段长旁白就能把窗口占满，
     屏幕上只剩一条步骤 —— VitaAgent `:336-341` 记的就是这个实测 */
  const stepAt = items
    .map((it, i) => (it.kind === "call" ? i : -1))
    .filter((i) => i >= 0);
  const firstKeep =
    stepAt.length > RECENT_STEPS ? stepAt[stepAt.length - RECENT_STEPS] : 0;
  const hidden = live && !open ? firstKeep : 0;
  const shown = open ? items : live ? items.slice(hidden) : [];
  /** 头一行概括谁：跑的时候是收进去的那些，展开或跑完了是整条链 */
  const headText = summarizeChain(open || !live ? items : items.slice(0, hidden));

  /* 「还在跑的那一步」：跑着的这一轮里，最后一次调用还没有任何输出。
     判据是结构上的（调用与它的输出成对出现），不是拿时间戳猜的 */
  const lastCall = [...items]
    .reverse()
    .find((i): i is ChainCall => i.kind === "call");
  const runningKey =
    live && lastCall && !lastCall.results.length ? lastCall.key : "";

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
            icon={open ? "UpOutlined" : "DownOutlined"}
            className={styles.stepCaret}
          />
        </button>
      ) : null}
      {/* 跑的时候「还在外面的那几条」要和上面那句摘要拉开距离：它们是两种东西 ——
          上面那行是「已经收进去的」，下面这几条是「还在外面的」。
          贴着排的话看上去就成了「摘要展开后的内容」，正好是反的 */}
      <div className={live && headText && !open ? styles.chainLive : undefined}>
        {shown.map((it) =>
          it.kind === "note" ? (
            <div key={it.key} className={styles.chainNote}>
              {renderNote(it.msg, it.key)}
            </div>
          ) : (
            <StepRow
              key={it.key}
              step={it}
              running={it.key === runningKey}
              open={!!expanded[it.key]}
              onToggle={() => toggleExpand(it.key)}
              expanded={expanded}
              toggleExpand={toggleExpand}
            />
          ),
        )}
      </div>
    </div>
  );
};

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
        // （清单与后台任务是「当前状态」，已由 ChatPane 抽成单独的状态卡，
        // 挂在对话流末尾；选择卡同理挂在输入框上方，都不进内容流。）
        const visibleItems = keyed;
        // 执行中时报一下已走的步数。「最近动作」不再单列 ——
        // 链上摊着的最后三步已经把它说得更清楚（见 ExecChain）
        const runSteps = inProgress
          ? turn.items.filter((m) => m.role === "tool").length
          : 0;

        return (
          <div key={turn.key} className={styles.turn}>
            {/* 对话流只管「我说了什么」。排队状态与撤回一律交给对话流末尾的排队卡 ——
                两处都摆一份的话，同一条任务在正文和排队卡各显示一遍，还得为了去重
                把正文里的消息藏起来，于是「我发的内容在对话流里不见了」。
                职责分开之后，正文永远是完整的对话记录。 */}
            {turn.user ? (
              (() => {
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
                    <div className={styles.userTime}>{fmtTime(turn.user.timestamp)}</div>
                  </div>
                );
              })()
            ) : null}

            {(visibleItems.length > 0 || inProgress) && (
              <div className={styles.agent}>
                {(() => {
                    /* 一轮拆成两半：**过程进链，结论进正文**。
                       这是这一版的全部：工具调用不再是平铺的一行原始文本，
                       而是链上的一步；最终答案不再是链里的一个节点，
                       而是普通正文（比例字 ＋ markdown 基线）。

                       分界线照 VitaAgent `MessageList/index.tsx:601-637`：

                         跑完了 → **最后一段产出（assistant / plan）就是结论**，
                                  留在正文；它之前的一切都是过程，收进链。
                                  不能按「最后一次工具调用」切：一轮若以工具结尾
                                  （问路那种），它前面那段正文恰恰是说给人听的话，
                                  按那个判据整条回答会一个字都不剩。
                         跑着呢 → **最后一步过程之后的正文才是「正在写的那段」**，
                                  之前的每段正文都是旁白，按发生顺序收进链。
                                  沿用跑完那条的话，旁白会被留在链的**下面**、
                                  而它引出的那几步却在链里 —— 顺序整个反了。 */
                    const isProc = (m: PortalMessage) =>
                      m.role === "tool" || m.role === "tool_result";
                    let lastOut = -1;
                    let lastProc = -1;
                    visibleItems.forEach(({ m }, i) => {
                      if (
                        (m.role === "assistant" || m.role === "plan") &&
                        m.content.trim()
                      ) {
                        lastOut = i;
                      }
                      if (isProc(m)) lastProc = i;
                    });

                    const chain: ChainItem[] = [];
                    const body: { m: PortalMessage; k: string }[] = [];

                    visibleItems.forEach((it, i) => {
                      const { m, k } = it;
                      if (m.role === "tool") {
                        const { name, hint } = splitToolLine(m.content);
                        chain.push({
                          kind: "call",
                          key: k,
                          name,
                          hint,
                          results: [],
                        });
                        return;
                      }
                      if (m.role === "tool_result") {
                        /* 输出挂到**它前面那次调用**上：协议顺序就是先调用后回结果，
                           所以「紧挨着的上一步」就是它的归属，不用再猜。
                           一次调用可能跟着多条输出记录，全挂上去。
                           找不到上一步（历史裁剪把调用那条丢了）就自成一步，
                           内容一条都不丢。 */
                        const last = chain[chain.length - 1];
                        const out = { key: k, text: m.content, bad: m.isError };
                        if (last && last.kind === "call") {
                          last.results.push(out);
                        } else {
                          chain.push({
                            kind: "call",
                            key: k,
                            name: "",
                            hint: "",
                            results: [out],
                          });
                        }
                        return;
                      }
                      // 待批准的方案在执行中永远留在正文：它在等你点头，
                      // 收进链里就等于把要办的事藏了
                      if (inProgress && m.role === "plan") {
                        body.push(it);
                        return;
                      }
                      const isBody = inProgress ? i > lastProc : i >= lastOut;
                      if (isBody) body.push(it);
                      else chain.push({ kind: "note", key: k, msg: m });
                    });

                    /* 一次工具都没调：整轮都是正文，不摆链。
                       摆的话摘要行是空字符串、跑完之后 `shown` 又是空数组，
                       这一轮会渲染成一个什么都没有的空块 —— 内容凭空消失。 */
                    if (!chain.some((c) => c.kind === "call")) {
                      return visibleItems.map(({ m, k }) => renderItem(m, k));
                    }

                    const ckey = `chain|${turn.key}`;
                    return [
                      <ExecChain
                        key="chain"
                        items={chain}
                        live={inProgress}
                        open={!!expanded[ckey]}
                        onToggle={() => toggleExpand(ckey)}
                        expanded={expanded}
                        toggleExpand={toggleExpand}
                        renderNote={renderItem}
                      />,
                      ...body.map(({ m, k }) => renderItem(m, k)),
                    ];
                })()}
                {inProgress ? (
                  <Working
                    // 起点取这一轮的起始时刻：有用户消息就用它，否则退回首条产出
                    since={turn.user?.timestamp ?? turn.items[0]?.timestamp}
                    steps={runSteps}
                  />
                ) : null}
                {/* 落款：来源代理 + 时间。原先挂在终端卡的标题栏上，卡片撤掉之后
                    这两样仍要有地方待着 —— 时间是回看时定位用的。 */}
                {visibleItems.length > 0 ? (
                  <div className={styles.turnMeta}>
                    <span className={styles.turnProvider}>
                      {providerDsr || "终端"}
                    </span>
                    <span>
                      {fmtTime(
                        turn.items[turn.items.length - 1]?.timestamp ??
                          turn.user?.timestamp
                      )}
                    </span>
                  </div>
                ) : null}
              </div>
            )}
          </div>
        );
        });
      })()}
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
