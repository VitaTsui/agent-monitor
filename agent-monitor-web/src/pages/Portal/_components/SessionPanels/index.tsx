import React, { useState } from "react";

import { Tooltip } from "antd";
import {
  CheckSquareOutlined,
  CloseOutlined,
  PartitionOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";

import { PortalMessage } from "@/services/apis/portal";
import styles from "./index.module.scss";

/** 任务清单里的一项（后端 todos 消息的 JSON 结构） */
interface TodoItem {
  id: string;
  subject: string;
  status: string;
}

/** 后台运行的任务（后端 bgtasks 消息的 JSON 结构） */
interface BgTask {
  id: string;
  label: string;
  status: string;
  /** "agent"=子代理 Task / "bg"=后台命令（缺省按后台命令） */
  kind?: string;
}

interface SessionPanelsProps {
  messages: PortalMessage[];
  /** 会话是否正在运行：非运行时清单里的「进行中」降级为「未完成」，
      不再显示会动的进行态（会话都停了就没有正在做的任务）。 */
  running?: boolean;
}

const BG_LABEL: Record<string, string> = {
  running: "执行中",
  pending: "等待中",
  queued: "等待中",
};

/** 已结束的后台任务状态：不再有关注价值，不展示 */
const BG_DONE = ["completed", "failed", "killed", "stopped"];

/**
 * 取某个 role 的最后一条并解析成数组。
 * 后端对这两类「状态快照」只产出最终一份，所以这里取到的就是当前状态 ——
 * 状态更新表现为原地刷新，而不是往对话流里堆一条新的。
 */
function parseLast<T>(messages: PortalMessage[], role: string): T[] {
  const hit = [...messages].reverse().find((m) => m.role === role);
  if (!hit) {
    return [];
  }
  try {
    const parsed = JSON.parse(hit.content);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    // 解析不了就当没有，别把整个面板带崩
    return [];
  }
}

interface DockProps {
  title: string;
  count: React.ReactNode;
  pill: React.ReactNode;
  children: React.ReactNode;
}


/** 单个可收起的悬浮面板（收起后缩成一枚胶囊） */
const Dock: React.FC<DockProps> = ({ title, count, pill, children }) => {
  // 默认收起（收起态胶囊仍带进度/数量，信息不丢，需要时点开即可）：面板占地大，
  // 尤其拆分多格时会盖住对话区，默认缩成胶囊挂着更清爽。
  const [open, setOpen] = useState(false);

  if (!open) {
    return (
      <Tooltip title={`展开${title}`} placement="left">
        <span
          className={styles.pill}
          role="button"
          tabIndex={0}
          aria-label={`展开${title}`}
          aria-expanded={false}
          onClick={() => setOpen(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setOpen(true);
            }
          }}
        >
          {pill}
        </span>
      </Tooltip>
    );
  }

  return (
    <section className={styles.dock}>
      <div className={styles.dockHead}>
        <span className={styles.dockTitle}>{title}</span>
        <span className={styles.dockCount}>{count}</span>
        <span
          className={styles.closeBtn}
          role="button"
          tabIndex={0}
          aria-label={`收起${title}`}
          aria-expanded
          onClick={() => setOpen(false)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setOpen(false);
            }
          }}
        >
          <CloseOutlined />
        </span>
      </div>
      <div className={styles.dockBody}>{children}</div>
    </section>
  );
};

/**
 * 会话的「当前状态」浮层：任务清单与后台任务两块互相独立，各自可收起。
 *
 * 两者都不是时序事件，故不混在对话流里，而是悬浮在本格右侧。
 * 定位锚点是各自的 ChatPane（见其 position: relative），
 * 多个会话并排时每格各带各的浮层。
 */
const SessionPanels: React.FC<SessionPanelsProps> = (props) => {
  const { messages, running } = props;

  const allTodos = parseLast<TodoItem>(messages, "todos");
  const allBg = parseLast<BgTask>(messages, "bgtasks");

  // 只看还没做完的：做完的条目没有关注价值，全做完时整块也就不显示了
  const todos = allTodos.filter((t) => t.status !== "completed");
  // 后台任务同理，只看还没结束的：执行中 / 等待中。再按种类拆成「子代理」与「后台命令」
  const aliveBg = allBg.filter((t) => !BG_DONE.includes(t.status));
  const subAgents = aliveBg.filter((t) => t.kind === "agent");
  const bgTasks = aliveBg.filter((t) => t.kind !== "agent");

  if (!todos.length && !bgTasks.length && !subAgents.length) {
    return null;
  }

  const runningCount = bgTasks.filter((t) => t.status === "running").length;
  const agentRunning = subAgents.filter((t) => t.status === "running").length;

  return (
    <div className={styles.SessionPanels}>
      {/* 计数只报「还剩几条」：列表里已经不显示做完的了，
          再写成 22/23 会与眼前只有 1 条的列表对不上。 */}
      {todos.length > 0 && (
        <Dock
          title="任务清单"
          count={`剩 ${todos.length}`}
          pill={
            <>
              <CheckSquareOutlined />
              {todos.length}
            </>
          }
        >
          {todos.map((t) => {
            // 会话停了就没有「正在进行」的任务，进行中降级为未完成
            const eff = !running && t.status === "in_progress" ? "pending" : t.status;
            return (
              <div key={t.id} className={`${styles.todoItem} ${styles[eff] ?? ""}`}>
                <span className={styles.todoBox} aria-hidden />
                <span className={styles.todoText}>{t.subject}</span>
              </div>
            );
          })}
        </Dock>
      )}

      {bgTasks.length > 0 && (
        <Dock
          title="后台任务"
          count={runningCount ? `${runningCount} 执行中` : `${bgTasks.length} 个`}
          pill={
            <>
              <ThunderboltOutlined />
              {runningCount || bgTasks.length}
            </>
          }
        >
          {bgTasks.map((t) => (
            <div key={t.id} className={styles.bgItem}>
              <span className={`${styles.bgDot} ${styles[t.status] ?? ""}`} />
              <Tooltip title={t.label} placement="left">
                <span className={styles.bgText}>{t.label}</span>
              </Tooltip>
              <span className={styles.bgStatus}>
                {BG_LABEL[t.status] ?? t.status}
              </span>
            </div>
          ))}
        </Dock>
      )}

      {subAgents.length > 0 && (
        <Dock
          title="子代理"
          count={agentRunning ? `${agentRunning} 执行中` : `${subAgents.length} 个`}
          pill={
            <>
              <PartitionOutlined />
              {agentRunning || subAgents.length}
            </>
          }
        >
          {subAgents.map((t) => (
            <div key={t.id} className={styles.bgItem}>
              <span className={`${styles.bgDot} ${styles[t.status] ?? ""}`} />
              <Tooltip title={t.label} placement="left">
                <span className={styles.bgText}>{t.label}</span>
              </Tooltip>
              <span className={styles.bgStatus}>
                {BG_LABEL[t.status] ?? t.status}
              </span>
            </div>
          ))}
        </Dock>
      )}
    </div>
  );
};

export default SessionPanels;
