import React, { useEffect, useState } from "react";

import { Tooltip } from "antd";
import {
  CheckSquareOutlined,
  PartitionOutlined,
  ThunderboltOutlined,
  WarningFilled,
} from "@ant-design/icons";

import { PortalMessage } from "@/services/apis/portal";
import {
  aliveBgCommands,
  BG_LABEL,
  BgTask,
  fmtElapsed,
  isBgFailed,
  parseLast,
  TodoItem,
  visibleSubAgents,
} from "../../_utils/sessionState";
import styles from "./index.module.scss";

interface SessionPanelsProps {
  messages: PortalMessage[];
  /** 会话是否正在运行：非运行时清单里的「进行中」降级为「未完成」，
      不再显示会动的进行态（会话都停了就没有正在做的任务）。 */
  running?: boolean;
}

interface CardProps {
  icon: React.ReactNode;
  title: string;
  meta: React.ReactNode;
  children: React.ReactNode;
}

/**
 * 一张状态卡。形态照 VitaAgent 的任务卡（`TaskCard/index.module.scss:14-56`）：
 * 28×28 描边色块放图标、右侧标题 + 一行 12px 次级色的元信息，
 * 下面是一列用发丝线分隔的条目。
 *
 * **没有收起态**。原来是悬浮在右上角、默认收起成一枚胶囊的浮层 ——
 * 那是因为它盖在对话上，不收起就挡内容。现在它就排在对话流末尾、跟着一起滚，
 * 不挡任何东西，也就没有理由再藏起来：要看会话在办什么，本来就该一眼看到。
 */
const StateCard: React.FC<CardProps> = ({ icon, title, meta, children }) => (
  <section className={styles.card}>
    <div className={styles.head}>
      <span className={styles.headTile}>{icon}</span>
      <span className={styles.headText}>
        <span className={styles.headTitle}>{title}</span>
        <span className={styles.headMeta}>{meta}</span>
      </span>
    </div>
    <ul className={styles.list}>{children}</ul>
  </section>
);

/**
 * 后台任务 / 子代理的一行。
 *
 * 三种收场分色（与 VitaAgent 一致，见 `TaskCard/index.module.scss:94-104`）：
 * 执行中走主色（墨黑）+ 脉冲，异常收场走 destructive，其余中性。
 * 右侧那枚小胶囊写耗时 —— 「还在跑」与「卡死了」的唯一区别就是它。
 */
const TaskRow: React.FC<{ task: BgTask; now: number }> = ({ task, now }) => {
  const failed = isBgFailed(task.status);
  const elapsed = fmtElapsed(task.startedAt, now);
  return (
    <li className={`${styles.item} ${failed ? styles.failed : ""}`}>
      <span className={styles.itemHead}>
        {failed ? (
          <WarningFilled className={styles.itemIcon} />
        ) : (
          <span className={`${styles.dot} ${styles[task.status] ?? ""}`} />
        )}
        <Tooltip title={task.label}>
          <span className={styles.itemName}>{task.label}</span>
        </Tooltip>
        <span className={styles.itemStatus}>
          {BG_LABEL[task.status] ?? task.status}
        </span>
        {/* 耗时做成胶囊而不是裸字：与名字同处一行，裸字会连成一片
            （VitaAgent `TaskCard/index.module.scss:376-388` 的 `.cellMeta`） */}
        {elapsed ? <span className={styles.itemMeta}>{elapsed}</span> : null}
      </span>
    </li>
  );
};

/**
 * 会话的「当前状态」：任务清单、后台任务、子代理三块。
 *
 * **排在对话流末尾**，跟着对话一起滚，与正文同宽同侧。原先是悬浮在本格右上角的
 * 浮层（`position:absolute; top:68; right:12; width:250`）——浮层盖着对话，
 * 只好默认收起成胶囊，于是「这个会话正在办什么」这件最该被看到的事，
 * 反倒需要先点一下才看得见。
 */
const SessionPanels: React.FC<SessionPanelsProps> = (props) => {
  const { messages, running } = props;

  const allTodos = parseLast<TodoItem>(messages, "todos");

  // 只看还没做完的：做完的条目没有关注价值，全做完时整块也就不显示了
  const todos = allTodos.filter((t) => t.status !== "completed");
  // 后台任务同理，只看还没正常跑完的 —— 跑砸/被终止的**要留着**（见 sessionState 的
  // BG_DONE 说明）。按种类拆成「子代理」与「后台命令」两块。
  const subAgents = visibleSubAgents(messages);
  const bgTasks = aliveBgCommands(messages);

  // 耗时要走字：只在真有后台任务时上表，且 tick 只驱动本组件重渲染。
  const ticking = bgTasks.length > 0 || subAgents.length > 0;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);

  if (!todos.length && !bgTasks.length && !subAgents.length) {
    return null;
  }

  const countMeta = (list: BgTask[]) => {
    const bad = list.filter((t) => isBgFailed(t.status)).length;
    const alive = list.length - bad;
    const parts: string[] = [];
    if (alive) {
      parts.push(`${alive} 个进行中`);
    }
    if (bad) {
      parts.push(`${bad} 个未跑成`);
    }
    return parts.join(" · ");
  };

  return (
    <div className={styles.SessionPanels}>
      {todos.length > 0 && (
        <StateCard
          icon={<CheckSquareOutlined />}
          title="任务清单"
          // 计数只报「还剩几条」：列表里已经不显示做完的了，
          // 再写成 22/23 会与眼前只有 1 条的列表对不上。
          meta={`还剩 ${todos.length} 条`}
        >
          {todos.map((t) => {
            // 会话停了就没有「正在进行」的任务，进行中降级为未完成
            const eff =
              !running && t.status === "in_progress" ? "pending" : t.status;
            return (
              <li key={t.id} className={`${styles.item} ${styles[eff] ?? ""}`}>
                <span className={styles.itemHead}>
                  <span className={styles.todoBox} aria-hidden />
                  <span className={styles.todoText}>{t.subject}</span>
                </span>
              </li>
            );
          })}
        </StateCard>
      )}

      {bgTasks.length > 0 && (
        <StateCard
          icon={<ThunderboltOutlined />}
          title="后台任务"
          meta={countMeta(bgTasks)}
        >
          {bgTasks.map((t) => (
            <TaskRow key={t.id} task={t} now={now} />
          ))}
        </StateCard>
      )}

      {subAgents.length > 0 && (
        <StateCard
          icon={<PartitionOutlined />}
          title="子代理"
          meta={countMeta(subAgents)}
        >
          {subAgents.map((t) => (
            <TaskRow key={t.id} task={t} now={now} />
          ))}
        </StateCard>
      )}
    </div>
  );
};

export default SessionPanels;
