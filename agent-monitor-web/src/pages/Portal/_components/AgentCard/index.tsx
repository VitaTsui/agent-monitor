import React, { useEffect, useState } from "react";

import { Icon } from "@hsu-react/ui";

import { SubTask, SubTaskOutcome } from "@/services/apis/portal";
import StatusIcon, {
  CHAIN_ICON,
  statusOfOutcome,
} from "../StatusIcon";
import {
  SUB_OUTCOME_LABEL,
  fmtElapsed,
  fmtSubTaskElapsed,
  isSubTaskFailed,
  isSubTaskRunning,
} from "../../_utils/sessionState";
import styles from "./index.module.scss";

interface AgentCardProps {
  /** 这一批子代理的标题：一个时是它的派活说明，多个时是「起了 N 个子代理」 */
  goal: string;
  /** 同一处并行起的那几个子代理（按发生顺序） */
  agents: SubTask[];
  /** 此刻点开的是哪一个（空串 = 一个都没点开）。**放在链那一层**，理由见下 */
  picked: string;
  /** 点某张小卡 / 卡片整个收起（传空串）。链据此决定要不要在卡片下面接一条子链 */
  onPick: (agentId: string) => void;
}

/**
 * 执行链上的**智能体卡**：模型在这一步派了子代理出去。
 *
 * 形态照 VitaAgent 的任务卡（`web/src/pages/chat/_components/TaskCard`）：一块浅灰底
 * 直接嵌在链里（无描边），头行是「目标 ＋ 状态·个数·耗时」，展开是一片子代理小卡网格。
 * 点某张小卡，**那个子代理自己走过的链就作为兄弟节点原地接在卡片下面继续往下排**
 * —— 不跳转、不抽屉、不弹层（子链由 `TerminalFeed` 渲染，见那边的 `SubAgentChain`）。
 *
 * 所以「点开的是哪一个」必须存在**链那一层**而不是这里：卡片与子链是两个平级的
 * 链节点，那根贯穿的竖线才穿得过去；存在卡片里的话子链只能画在卡片内部，就成了
 * 「卡内滚动面板」—— VitaAgent 那边正是从这个形态改过来的，三个毛病写在
 * `MessageList/index.tsx:451-467`，不重蹈。
 *
 * **跑中默认展开、跑完默认收起**：跑的时候人要看它到哪一步了；跑完要看的是结论，
 * 而结论已经并回主对话了，过程再摊着只会把它推走一屏。展开态**不持久化**
 * （纯 useState），换一条会话就回到默认。手动收起过之后又有新的活跑起来时，
 * 手动态作废、回到默认展开 —— 见下面的 `staleManual`。
 *
 * 卡片自身收起来**只收下面那片小卡网格**，卡头（转着的图标 ＋ 走字的耗时）始终在。
 * 「在跑的东西看得见」由卡头担保，用户手动收起的是明细，不是那个事实。
 */
const AgentCard: React.FC<AgentCardProps> = ({
  goal,
  agents,
  picked,
  onPick,
}) => {
  /**
   * 手动展开态：用户的选择 ＋ **他做这个选择时，这张卡里在跑的是哪几次活**。
   *
   * `undefined` = 还没动过，跟着「在不在跑」走；点过之后以用户的选择为准，
   * 否则跑完的那一刻会把他刚展开的东西合上。
   */
  const [manual, setManual] = useState<{ open: boolean; seen: string[] }>();

  const running = agents.filter(isSubTaskRunning);
  const isRunning = running.length > 0;
  /** 此刻在跑的那几**次**活。同一个子代理被重新派一次（`runs`）算新的一次 */
  const runningKeys = running.map((a) => `${a.id}#${a.runs ?? 1}`);
  /**
   * **有小卡被点开就必然是展开的**，这一条压过手动收起态。
   *
   * 侧栏点一条子代理会从外面把 `picked` 设上（见 `PortalStore.focusAgent`）——
   * 那时这张卡若还收着（用户先前手动收过），子链就挂在一张看不见的卡下面，
   * 定位也会因为小卡不在 DOM 里而误报「找不到」。收起这个动作本身不会被这条
   * 压住：下面那个头按钮收起时先 `onPick("")` 把小卡松开，`picked` 一空，
   * 展开与否就交还给 `manual`。
   */
  /**
   * 手动态**只对用户当时看到的那批活有效**。
   *
   * 他收起的是「这几个我不看了」，不是「这张卡以后永远收着」——之后又有活跑起来
   * （新的子代理翻成 running，或同一个被重新派了一次）是个**新事件**，那时该回到
   * 「在跑就展开」。判据是结构性的：比对在跑的那几次活，没见过的就算新的；
   * 原来那些陆续跑完（集合只是缩小）不算，手动收起继续生效。
   */
  const staleManual =
    !!manual && runningKeys.some((k) => !manual.seen.includes(k));
  const expanded =
    !!picked || (manual && !staleManual ? manual.open : isRunning);

  // 耗时要走字：只在真有子代理跑着时上表，且 tick 只驱动这张卡重渲染
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!isRunning) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [isRunning]);

  const done = agents.filter((a) => a.outcome === "completed").length;
  const failed = agents.filter(isSubTaskFailed);
  const interrupted = agents.filter((a) => a.outcome === "interrupted");

  /* 这一格整体是什么收场。三种异常分开说，别把「被中断」并进「失败」。
     **跑着的时候是空串**：卡头那枚树状图标已经在转，右边还写着
     `已完成 1 / 3 个` ＋ 走字的耗时 —— 再补「执行中」三个字，同一件事在一行里
     说了三遍。收场（失败 / 已中断 / 已完成）仍然写字：图标说不出这些。 */
  const statusText = isRunning
    ? ""
    : failed.length
      ? `${failed.length} 个失败`
      : interrupted.length
        ? `${interrupted.length} 个已中断`
        : "已完成";
  const statusKind: SubTaskOutcome = isRunning
    ? "running"
    : failed.length
      ? "failed"
      : interrupted.length
        ? "interrupted"
        : "completed";

  /* 整批的耗时：最早那个起跑到最晚那个收尾（还在跑的算到此刻）。
     一个一个各报各的耗时在头行里排不下，而人在这一行要的是「这一步花了多久」。 */
  const startedAt = agents
    .map((a) => a.startedAt)
    .filter(Boolean)
    .sort()[0];
  const endedMs = isRunning
    ? now
    : Math.max(0, ...agents.map((a) => a.endedMs || 0)) || now;
  const elapsed = fmtElapsed(startedAt, endedMs);

  return (
    <div className={styles.AgentCard}>
      {/* 整个头是一个按钮：卡片上除了展开没有别的可点，
          单独放一个小箭头反而增加瞄准成本 */}
      <button
        type="button"
        className={styles.head}
        aria-expanded={expanded}
        {...(isRunning ? { role: "status", "aria-label": "执行中" } : {})}
        onClick={() => {
          // 收起时把点开的那张小卡一并松开，否则子链会挂在一张看不见的卡下面
          if (expanded) {
            onPick("");
          }
          // 记下「他是在哪几次活跑着的时候做的这个决定」，理由见 staleManual
          setManual({ open: !expanded, seen: runningKeys });
        }}
      >
        <span className={styles.headTile}>
          {/* 卡头是那枚树状结构图标（VitaAgent `TaskCard` 同款 `ph:tree-structure`）：
              这一格说的是「模型在这一步派了活出去」，不是一个状态 */}
          <Icon
            icon={CHAIN_ICON.agents}
            className={`${styles.headIcon} ${isRunning ? styles.headIconLive : ""}`}
          />
        </span>
        <span className={styles.headText}>
          {/* 派活说明是模型写的，常常是一整句。限两行 ——
              不截的话卡片标题能占掉四五行，而它只是个标题 */}
          <span className={styles.goal} title={goal}>
            {goal}
          </span>
          <span className={styles.meta}>
            {statusText ? (
              <span className={`${styles.status} ${styles[statusKind]}`}>
                {statusText}
              </span>
            ) : null}
            <span>
              {isRunning
                ? `已完成 ${done} / ${agents.length} 个`
                : `${agents.length} 个子代理`}
            </span>
            {elapsed ? <span>{elapsed}</span> : null}
          </span>
        </span>
        {/* 折叠箭头换 Phosphor，与链上其余箭头同一副字形。
            **不是尺寸问题**：它一直是 12px，但 antd 的 `DownOutlined` 把 em 框
            填得更满 —— 同样 12px，antd 画出来的实体比 `ph:caret-down` 大一圈，
            看着就「太大」。改的是字形，不是数字。
            展开态**换字形**而不是把它转 180°（参照 `MessageList/index.tsx:747`
            就是 `expanded ? "ph:caret-up" : "ph:caret-down"`）—— caret 的上下两式
            本来就各画一枚，转出来的那枚三角重心是反的。 */}
        <Icon
          icon={expanded ? "ph:caret-up" : "ph:caret-down"}
          className={styles.caret}
        />
      </button>

      {/* 失败原因**不藏在展开区里**：一个子代理跑砸时它是最需要被看到的东西，
          折叠着等于没说。原文照登 —— 那句话是上游写的，里面的退出码、
          request id 才是「我该怎么办」的依据，翻译或改写都会把它弄丢 */}
      {failed.map((a) =>
        a.summary?.trim() ? (
          <div key={`e${a.id}`} className={styles.cardError}>
            <Icon icon="ph:warning-circle-fill" className={styles.cardErrorIcon} />
            <span>{a.summary.trim()}</span>
          </div>
        ) : null,
      )}

      {expanded ? (
        <div className={styles.body}>
          <div className={styles.grid}>
            {agents.map((a) => {
              const on = picked === a.id;
              // 没留下会话记录的（罕见）点开也没有正文，置灰不可点
              const openable = a.hasBody;
              return (
                <button
                  key={a.id}
                  type="button"
                  /* 侧栏点一条子代理时按它定位到这张小卡（见 TerminalFeed 的
                     定位副作用）。**判据是 agentId 这个结构化字段**，
                     不拿 label / 文案去凑 */
                  data-agent-id={a.id}
                  className={`${styles.cell} ${styles[a.outcome]} ${
                    on ? styles.cellOn : ""
                  } ${openable ? "" : styles.cellDisabled}`}
                  aria-pressed={on}
                  disabled={!openable}
                  title={
                    openable
                      ? a.label
                      : "这个子代理没有留下会话记录，看不到它走过的链"
                  }
                  onClick={() => onPick(on ? "" : a.id)}
                >
                  <span className={styles.cellHead}>
                    <StatusIcon
                      kind={statusOfOutcome(a.outcome)}
                      className={styles.cellIcon}
                    />
                    <span className={styles.cellNm}>{a.label}</span>
                  </span>
                  {/* 第二行是这一个的「量」：状态 ＋ 耗时（＋ 被派过几次）。
                      「还在跑」与「卡死了」的唯一区别就是耗时在不在走字。

                      **被重新派过活就要说出来**：同一个子代理可以被再派一次
                      （`SubTask.runs`），那时它的收场从「已完成」翻回「执行中」是
                      **对的**。但界面上要是不写这一句，用户看到的就是一个跑完的
                      子代理忽然又动起来，没有任何解释 —— 跟这一版一直在修的
                      「界面别骗人」是同一件事。
                      `runs` 缺失或等于 1 时什么都不写：「第 1 次派活」是噪音。 */}
                  {/* 跑着的那张小卡不写「执行中」：它左边那枚 `ph:circle-notch`
                      正在转，而这一行的位置该留给走字的耗时 —— 「还在跑」与
                      「卡死了」的唯一区别就是它在不在动。 */}
                  <span className={styles.cellMeta}>
                    {a.outcome === "running"
                      ? ""
                      : (SUB_OUTCOME_LABEL[a.outcome] ?? a.status)}
                    {`${a.outcome === "running" ? "" : " · "}${fmtSubTaskElapsed(a, now) || "—"}`}
                    {a.runs && a.runs > 1 ? ` · 第 ${a.runs} 次派活` : ""}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
};

export default AgentCard;
