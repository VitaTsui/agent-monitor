import React, { useEffect, useState } from "react";

import { Popover } from "antd";
import { PartitionOutlined } from "@ant-design/icons";

import { PortalMessage } from "@/services/apis/portal";
import {
  BG_LABEL,
  fmtElapsed,
  runningSubAgents,
} from "../../_utils/sessionState";
import styles from "./index.module.scss";

interface SubAgentChipProps {
  messages: PortalMessage[];
}

/**
 * 会话头部的「子会话」胶囊。
 *
 * 收起时只报运行中的条数（子会话 · 3），点开列出每个的名字 / 状态 / 耗时。
 * **只统计运行中的** —— 跑完的自动掉出去，一个不剩时整枚胶囊不渲染。
 *
 * 独立成块是刻意的：现在挂在 headMeta 末尾（与 hostname / token 同一行），
 * 因为 headActions 那排按钮已在塌缩临界（低于 FLAT_MIN_W 会整排折成 ⋯），
 * 再插非按钮元素会提前触发折叠。真要挪回图标同行，改 ChatPane 里那一行即可。
 */
const SubAgentChip: React.FC<SubAgentChipProps> = ({ messages }) => {
  const agents = runningSubAgents(messages);
  const count = agents.length;

  // 耗时要走字。只在有子会话时起表，且 tick 只驱动这枚胶囊重渲染，
  // 不牵动整个 ChatPane（对话流每秒重渲染代价太大）。
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!count) {
      return;
    }
    const timer = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(timer);
  }, [count]);

  if (!count) {
    return null;
  }

  const list = (
    <div className={styles.list}>
      {agents.map((a) => {
        const elapsed = fmtElapsed(a.startedAt);
        return (
          <div key={a.id} className={styles.item}>
            <span className={`${styles.dot} ${styles[a.status] ?? ""}`} />
            <span className={styles.name} title={a.label}>
              {a.label}
            </span>
            <span className={styles.status}>
              {BG_LABEL[a.status] ?? a.status}
            </span>
            {elapsed ? <span className={styles.elapsed}>{elapsed}</span> : null}
          </div>
        );
      })}
    </div>
  );

  return (
    <Popover
      content={list}
      title="运行中的子会话"
      trigger="click"
      placement="bottomRight"
      overlayClassName={styles.pop}
    >
      <span
        className={styles.chip}
        role="button"
        tabIndex={0}
        aria-label={`展开 ${count} 个运行中的子会话`}
        // 紧凑卡片整卡可点（换到主区），点胶囊不该顺带把卡片挪走
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            (e.currentTarget as HTMLElement).click();
          }
        }}
      >
        <PartitionOutlined />
        子会话 · {count}
      </span>
    </Popover>
  );
};

export default SubAgentChip;
