import React from "react";

import { Icon } from "@hsu-react/ui";

import styles from "./index.module.scss";

/**
 * 一件事此刻处在什么状态。**全项目只有这一份口径**：会话（`PortalTaskData.status`）、
 * 子代理（`SubTask.outcome`）、后台命令、执行链上的一步，全部先归到这几档，
 * 再由这里决定画什么图标、什么颜色、转不转。
 *
 * 从前是每处各写一张 `Record<状态, ReactNode>`（`SessionPanels`、`AgentCard`、
 * `TerminalFeed`、侧栏各一份），于是同一个「执行中」在四个地方是四种字形、
 * 三种字号、两种颜色 —— 用户为此提过三次。
 */
export type StatusKind =
  /** 正在跑 */
  | "running"
  /** 等人输入（会话 idle） */
  | "waiting"
  /** 被按停了 */
  | "paused"
  /** 排着还没轮到 */
  | "pending"
  /** 会话结束了。**中性**，不是「成功」—— 会话没有成败可言 */
  | "finished"
  /** 办成了 */
  | "completed"
  /** 跑砸了 */
  | "failed"
  /** 被连带终止 —— 不是它的错，走中性记号 */
  | "interrupted";

/**
 * 状态 → Phosphor 图标名，逐字照 VitaAgent
 * （`web/src/pages/chat/_components/TaskCard/index.tsx:80-86`）。
 *
 * **为什么不是 antd 图标**：VitaAgent 用的是 Iconify 的 Phosphor 集（`ph:*`），
 * 本项目一直在 antd 里挑「语义最近的那一个」—— 尺寸颜色对齐过三轮，形状始终对不上，
 * 因为两套图标集的字形本来就不同。现在直接引 Phosphor 本尊，形状一致是定义上的。
 */
export const STATUS_ICON: Record<StatusKind, string> = {
  running: "ph:circle-notch",
  waiting: "ph:circle-dashed",
  paused: "ph:pause-circle",
  pending: "ph:circle-dashed",
  finished: "ph:check-circle",
  completed: "ph:check-circle-fill",
  failed: "ph:x-circle-fill",
  interrupted: "ph:minus-circle",
};

/**
 * 执行链上那几枚**非状态**的字形，同样取 Phosphor —— 它们和状态图标并排落在同一列
 * 18×18 的槽里，混用两套图标集，一列扫下来就是两种笔画。
 * 放在这儿而不是各组件内部，理由与 `STATUS_ICON` 一样：一份定义。
 */
export const CHAIN_ICON = {
  /** 一次工具调用（VitaAgent 的 `builtin` 档） */
  tool: "ph:terminal-window",
  /** 模型一边干活一边说的那几句 */
  note: "ph:chat-teardrop-text",
  /** 智能体卡的卡头 */
  agents: "ph:tree-structure",
  /** 「还有 N 步，展开全部」 */
  more: "ph:dots-three",
} as const;

/** 会话状态（后端 `PortalTaskData.status`）归档。认不出来的按「排着」算 */
export const statusOfSession = (status?: string | null): StatusKind => {
  switch (status) {
    case "running":
      return "running";
    case "idle":
      return "waiting";
    case "paused":
      return "paused";
    case "finished":
      return "finished";
    default:
      return "pending";
  }
};

/** 子任务收场（`SubTask.outcome`）归档。它的四档与这里同名，直接过一遍防脏值 */
export const statusOfOutcome = (outcome?: string | null): StatusKind => {
  switch (outcome) {
    case "running":
      return "running";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    case "interrupted":
      return "interrupted";
    default:
      return "pending";
  }
};

interface StatusIconProps {
  kind: StatusKind;
  /**
   * 不给语义色，只继承父级的 `color`。
   *
   * 用在「整行统一变色」的地方：执行链里跑砸的那一步整行走 destructive，
   * 图标要跟着行走，不能自己钉一个色。
   */
  plain?: boolean;
  className?: string;
}

/**
 * 一枚状态图标：字形 ＋ 语义色 ＋（执行中的）旋转，三件事都在这里定死。
 *
 * 尺寸**不在这里定**：它落在多大的槽里由调用方说了算（执行链的槽是 18×18 / 字形 12，
 * 右栏那份跟着它自己的行高）。颜色与动画则必须统一 —— 那正是三轮没对齐的东西。
 */
const StatusIcon: React.FC<StatusIconProps> = ({ kind, plain, className }) => (
  <Icon
    icon={STATUS_ICON[kind]}
    className={`${styles.StatusIcon} ${plain ? "" : styles[kind]} ${
      kind === "running" ? styles.spin : ""
    } ${className ?? ""}`}
  />
);

export default StatusIcon;
