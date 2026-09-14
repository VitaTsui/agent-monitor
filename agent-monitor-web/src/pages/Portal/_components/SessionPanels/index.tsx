import React, { useEffect, useState } from "react";

import { Icon } from "@hsu-react/ui";
import { Tooltip } from "antd";
import { observer } from "mobx-react-lite";

import { PortalMessage, SubTask } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import StatusIcon, { CHAIN_ICON } from "../StatusIcon";
import {
  SUB_OUTCOME_LABEL,
  fmtSubTaskElapsed,
  isEmptySessionState,
  sessionStateOf,
} from "../../_utils/sessionState";
import styles from "./index.module.scss";

interface SessionPanelsProps {
  /**
   * 这一份状态说的是哪一条会话。
   *
   * **「正在执行的子代理」那一节点一条要滚的是本格的链**（`focusAgentCard(taskId,…)`
   * 认这个 id，见 ChatPane 的 `focusMine`）—— 拆分视图下四格各有各的右栏，
   * 不带主语就会滚错格。
   */
  taskId: string;
  messages: PortalMessage[];
  /**
   * 这条会话名下的子任务。取自 `PortalTaskData.subTasks`。
   *
   * **只用其中的后台命令**：子代理有自己的执行链节点（正文里的智能体卡），
   * 状态区再列一遍就是同一件事说两遍。筛选口径在 `runningBgCommands` 里，只有一份。
   */
  subTasks?: SubTask[];
  /** 会话是否正在运行：非运行时清单里的「进行中」降级为「未完成」，
      不再显示会动的进行态（会话都停了就没有正在做的任务）。 */
  running?: boolean;
  /** 外层追加的类名。 */
  className?: string;
  /**
   * **平铺模式**：不自带描边卡，改成一列 12 内边距、发丝线分隔的分区。
   *
   * 右栏里用它 —— 那儿外面已经是一张浮着的卡（见 `SessionStatePane`），
   * 里面再套两张带边框的小卡就是框中框。窄屏排在对话流末尾时不传：
   * 那儿没有外卡可依，每一块得自己是一张卡。
   *
   * **边框与圆角属于所处的位置，不属于内容** —— 所以这是一个外部传入的开关，
   * 而不是在组件里再判一次「我这会儿在哪儿」。
   */
  flat?: boolean;
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
 * 那是因为它盖在对话上，不收起就挡内容。现在它待在右栏（窄屏是对话流末尾），
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
 * 后台命令的一行。**这一节只有还在跑的**（见 `runningBgCommands`），
 * 所以这里不再有收场分色、也不再有那行收场原因 —— 收场了的条目根本走不到这儿。
 *
 * 字形与转速由 `_components/StatusIcon` 一处给（Phosphor `ph:circle-notch`
 * 1.1s linear ＋ 主色，与 VitaAgent `TaskCard/index.tsx:80-86` 逐字相同）。
 * 右侧那枚小胶囊写耗时 —— **「还在跑」与「卡死了」的唯一区别就是它动不动**。
 *
 * 上一版这里按 `task.outcome` 分色、并在收场不对时多画一行 `task.summary`。
 * 那套是给「跑砸的也留在清单里」配的，随那条口径一并撤掉，不留第二套判断：
 * `summary` 只有终态条目才有（后端对 `running` 恒给 `None`），留着必然是死代码。
 */
const TaskRow: React.FC<{ task: SubTask; now: number }> = ({ task, now }) => {
  const elapsed = fmtSubTaskElapsed(task, now);
  return (
    <li className={`${styles.item} ${styles.running}`}>
      <span className={styles.itemHead}>
        <StatusIcon kind="running" className={styles.itemIcon} />
        <Tooltip title={task.label}>
          <span className={styles.itemName}>{task.label}</span>
        </Tooltip>
        <span className={styles.itemStatus}>{SUB_OUTCOME_LABEL.running}</span>
        {/* 耗时做成胶囊而不是裸字：与名字同处一行，裸字会连成一片
            （VitaAgent `TaskCard/index.module.scss:376-388` 的 `.cellMeta`） */}
        {elapsed ? <span className={styles.itemMeta}>{elapsed}</span> : null}
      </span>
    </li>
  );
};

/**
 * 「正在执行的子代理」的一行。**它是个入口，不是一份内容。**
 *
 * 点它 = 让本格的执行链滚到派出它的那张智能体卡上并展开（`focusAgentCard`）——
 * 正文仍然只在链里渲染一次。不在右栏里就地展开：同一份内容画两处是这一版
 * 反复踩过的坑（子会话树那一套正是因此被推翻的）。
 *
 * **点了必定找得到。** 子代理的卡片现在由子任务清单决定、不再依赖正文窗口
 * （见 `TerminalFeed` 的 `attachLooseAgents`）—— 配不上派活记录的那些也照样在链上
 * 有位置（按 `startedAt` 落位）。于是「定位不到」只剩**一种**触发条件：
 * **这一格连一条正文都还没读到**（设备离线 / 记录读不出来），那时链本身就不存在。
 * 那一句仍然说在这一行上（`PortalStore.focusMissId`，与旧那套同一份机制）：
 * 不许点了没反应，也不弹一条飘过去的全局提示 —— 那得让人回头找刚才点的是哪条。
 */
const AgentRow: React.FC<{ taskId: string; task: SubTask; now: number }> =
  observer(({ taskId, task, now }) => {
    const elapsed = fmtSubTaskElapsed(task, now);
    const missed = PortalStore.focusMissId === task.id;
    return (
      <li className={`${styles.item} ${styles.running}`}>
        <button
          type="button"
          className={`${styles.itemHead} ${styles.itemLink}`}
          onClick={() => PortalStore.focusAgentCard(taskId, task.id)}
        >
          <StatusIcon kind="running" className={styles.itemIcon} />
          <Tooltip title={task.label}>
            <span className={styles.itemName}>{task.label}</span>
          </Tooltip>
          {/* 「执行中」三个字不写：左边那枚 `ph:circle-notch` 正在转，右边的耗时在
              走字，这一节的标题也写着「正在执行的子代理」—— 同一件事说三遍。
              耗时留着：**「还在跑」与「卡死了」的唯一区别就是它动不动。** */}
          {elapsed ? <span className={styles.itemMeta}>{elapsed}</span> : null}
        </button>
        {missed ? (
          <span className={styles.itemReason}>
            这条会话的正文还没读到，取回来之后再点
          </span>
        ) : null}
      </li>
    );
  });

/**
 * 会话的「当前状态」：任务清单、后台命令、正在执行的子代理三块。
 *
 * **子代理只给入口、不给内容**：正文是执行链上的一步，就地画在链里（见 `AgentCard`）；
 * 这儿只列**还在跑的那几个**，点一条滚过去。后台命令留在这儿是另一个理由 ——
 * 它**没有对应的执行链节点**，删了就没地方看了。
 *
 * **宽屏在右栏**（`SessionStatePane`）：与正文并排、不滚走、不挡任何东西。
 * **窄屏排在对话流末尾**：手机上摆不下第三栏，它跟着对话一起滚。
 *
 * 更早以前是悬浮在本格右上角的浮层（`position:absolute; top:68; right:12;
 * width:250`）—— 浮层盖着对话，只好默认收起成胶囊，于是「这个会话正在办什么」
 * 这件最该被看到的事，反倒需要先点一下才看得见。
 */
const SessionPanels: React.FC<SessionPanelsProps> = (props) => {
  const { taskId, messages, subTasks, running, className, flat } = props;

  // 「有哪些东西要展示」只有一处定义（见 sessionStateOf）：右栏要先问同一个问题
  // 才知道该不该给这条会话一个标题，两边各写一遍筛选条件迟早会对不上。
  const { todos, bgTasks, agents } = sessionStateOf(messages, subTasks);

  // 耗时要走字：后台任务与在跑的子代理都要，且 tick 只驱动本组件重渲染。
  const ticking = bgTasks.length > 0 || agents.length > 0;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);

  if (isEmptySessionState({ todos, bgTasks, agents })) {
    return null;
  }

  return (
    <div
      className={`${styles.SessionPanels} ${flat ? styles.flat : ""} ${
        className ?? ""
      }`}
    >
      {todos.length > 0 && (
        <StateCard
          icon={<Icon icon="ph:list-checks" />}
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
                  {/* 与全项目同一份状态图标：进行中是 `ph:circle-notch` 转圈 ＋ 主色，
                      没轮到的是 `ph:circle-dashed`。
                      清单里不会有「已完成」——那些在 `sessionStateOf` 就滤掉了。 */}
                  <StatusIcon
                    kind={eff === "in_progress" ? "running" : "pending"}
                    className={styles.itemIcon}
                  />
                  <span className={styles.todoText}>{t.subject}</span>
                </span>
              </li>
            );
          })}
        </StateCard>
      )}

      {/* 正在执行的子代理。**摆在后台任务前面**：它是这一栏里唯一「点了会动」的一节，
          而且用户找它的频率最高（一条长会话滚回去找那张卡是件苦差事）。
          字形与执行链上智能体卡的卡头同一枚（`CHAIN_ICON.agents`）—— 点过去看到的
          就是那张卡，两处必须是同一个记号。 */}
      {agents.length > 0 && (
        <StateCard
          icon={<Icon icon={CHAIN_ICON.agents} />}
          title="正在执行的子代理"
          // 只列在跑的，所以这一行就是个数；不写「N 个进行中」——「进行中」由标题说了
          meta={`${agents.length} 个`}
        >
          {agents.map((t) => (
            <AgentRow key={t.id} taskId={taskId} task={t} now={now} />
          ))}
        </StateCard>
      )}

      {bgTasks.length > 0 && (
        <StateCard
          icon={<Icon icon="ph:lightning" />}
          title="后台任务"
          // 只列还在跑的，所以这一行就是个数 —— 与上面那一节同一个口径。
          // 上一版这里分开数「N 个失败 / N 个已中断」，那是给「跑砸的也留在清单里」
          // 配的；口径改成「还在跑的」之后那两个数恒为 0，留着就是骗人的计数。
          meta={`${bgTasks.length} 个`}
        >
          {bgTasks.map((t) => (
            <TaskRow key={t.id} task={t} now={now} />
          ))}
        </StateCard>
      )}
    </div>
  );
};

export default SessionPanels;
