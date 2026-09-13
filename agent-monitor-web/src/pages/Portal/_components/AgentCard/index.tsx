import React, { useEffect, useState } from "react";

import {
  CheckCircleFilled,
  CloseCircleFilled,
  DownOutlined,
  LoadingOutlined,
  MinusCircleOutlined,
  PartitionOutlined,
  WarningFilled,
} from "@ant-design/icons";

import { SubTask, SubTaskOutcome } from "@/services/apis/portal";
import {
  SUB_OUTCOME_LABEL,
  fmtElapsed,
  fmtSubTaskElapsed,
  isSubTaskFailed,
  isSubTaskRunning,
} from "../../_utils/sessionState";
import styles from "./index.module.scss";

/** 一个子代理的收场对应的图标 */
const OUTCOME_ICON: Record<SubTaskOutcome, React.ReactNode> = {
  running: <LoadingOutlined />,
  completed: <CheckCircleFilled />,
  failed: <CloseCircleFilled />,
  // 被父会话连带终止 —— 不是它的错，走中性的减号圈，不走失败的叉
  interrupted: <MinusCircleOutlined />,
};

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
 * （纯 useState），换一条会话就回到默认。
 */
const AgentCard: React.FC<AgentCardProps> = ({
  goal,
  agents,
  picked,
  onPick,
}) => {
  /**
   * 手动展开态。`undefined` = 还没动过，跟着「在不在跑」走；
   * 一旦点过就以用户的选择为准，否则跑完的那一刻会把他刚展开的东西合上。
   */
  const [manual, setManual] = useState<boolean | undefined>();

  const running = agents.filter(isSubTaskRunning);
  const isRunning = running.length > 0;
  /**
   * **有小卡被点开就必然是展开的**，这一条压过手动收起态。
   *
   * 侧栏点一条子代理会从外面把 `picked` 设上（见 `PortalStore.focusAgent`）——
   * 那时这张卡若还收着（用户先前手动收过），子链就挂在一张看不见的卡下面，
   * 定位也会因为小卡不在 DOM 里而误报「找不到」。收起这个动作本身不会被这条
   * 压住：下面那个头按钮收起时先 `onPick("")` 把小卡松开，`picked` 一空，
   * 展开与否就交还给 `manual`。
   */
  const expanded = !!picked || (manual ?? isRunning);

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

  /** 这一格整体是什么收场。三种异常分开说，别把「被中断」并进「失败」 */
  const statusText = isRunning
    ? "执行中"
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
        onClick={() => {
          // 收起时把点开的那张小卡一并松开，否则子链会挂在一张看不见的卡下面
          if (expanded) {
            onPick("");
          }
          setManual(!expanded);
        }}
      >
        <span className={styles.headTile}>
          <PartitionOutlined
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
            <span className={`${styles.status} ${styles[statusKind]}`}>
              {statusText}
            </span>
            <span>
              {isRunning
                ? `已完成 ${done} / ${agents.length} 个`
                : `${agents.length} 个子代理`}
            </span>
            {elapsed ? <span>{elapsed}</span> : null}
          </span>
        </span>
        <DownOutlined
          className={`${styles.caret} ${expanded ? styles.caretOpen : ""}`}
        />
      </button>

      {/* 失败原因**不藏在展开区里**：一个子代理跑砸时它是最需要被看到的东西，
          折叠着等于没说。原文照登 —— 那句话是上游写的，里面的退出码、
          request id 才是「我该怎么办」的依据，翻译或改写都会把它弄丢 */}
      {failed.map((a) =>
        a.summary?.trim() ? (
          <div key={`e${a.id}`} className={styles.cardError}>
            <WarningFilled className={styles.cardErrorIcon} />
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
                    <span className={styles.cellIcon}>
                      {OUTCOME_ICON[a.outcome]}
                    </span>
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
                  <span className={styles.cellMeta}>
                    {SUB_OUTCOME_LABEL[a.outcome] ?? a.status}
                    {` · ${fmtSubTaskElapsed(a, now) || "—"}`}
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
