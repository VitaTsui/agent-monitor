import React, { useEffect, useState } from "react";

import { Modal } from "@hsu-react/ui";
import { Empty, Spin, message } from "antd";

import {
  SessionHistoryItem,
  getSessionHistory,
} from "@/services/apis/portal";
import styles from "./index.module.scss";

interface HistoryModalProps {
  open: boolean;
  onClose: () => void;
}

/** epoch 秒 → 「今天 14:23」/「08-01 14:23」，今年的省掉年份 */
const fmtTime = (sec: number) => {
  const d = new Date(sec * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes()
  ).padStart(2, "0")}`;
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  if (sameDay) return `今天 ${hm}`;
  const md = `${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
  return d.getFullYear() === now.getFullYear()
    ? `${md} ${hm}`
    : `${d.getFullYear()}-${md} ${hm}`;
};

/**
 * 会话历史：每个会话结束时留下的最终产出。
 *
 * 用途是「我派出去的那些活，最后都出了什么结果」——终端关了、机器关机之后仍能回看，
 * 这正是人不在电脑前时最需要的。列表默认收起结果，点标题展开全文。
 */
const HistoryModal: React.FC<HistoryModalProps> = ({ open, onClose }) => {
  const [list, setList] = useState<SessionHistoryItem[]>([]);
  const [loading, setLoading] = useState(false);
  // 展开查看完整产出的记录 id（默认都收起，避免长结果把列表撑爆）
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    getSessionHistory(50)
      .then((res) => setList(res?.data?.list ?? []))
      .catch(() => message.error("获取历史失败"))
      .finally(() => setLoading(false));
  }, [open]);

  const toggle = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <Modal
      title="会话历史"
      open={open}
      onCancel={onClose}
      footer={null}
      width={720}
      centered
    >
      <div className={styles.hint}>
        每个会话结束时留一条最终产出。终端关掉、机器关机后仍可在此回看。
      </div>
      <Spin spinning={loading}>
        <div className={styles.list}>
          {!loading && list.length === 0 && (
            <Empty description="还没有已结束的会话记录" />
          )}
          {list.map((it) => {
            const isOpen = expanded.has(it.id);
            const title = it.title || it.provider || "（无标题）";
            return (
              <div key={it.id} className={styles.item}>
                <div
                  className={styles.head}
                  onClick={() => toggle(it.id)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") toggle(it.id);
                  }}
                >
                  <div className={styles.titleRow}>
                    {/* 号位与钉钉里的「@N」同源，方便对上是哪个终端做的 */}
                    {it.slot != null && (
                      <span className={styles.slot}>{it.slot}</span>
                    )}
                    <span className={styles.title} title={title}>
                      {title}
                    </span>
                    <span className={styles.time}>{fmtTime(it.endedAt)}</span>
                  </div>
                  <div className={styles.meta}>
                    <span>{it.hostname}</span>
                    <span className={styles.dot}>·</span>
                    <span>{it.project}</span>
                    <span className={styles.dot}>·</span>
                    <span>{it.provider}</span>
                  </div>
                </div>
                {it.result && (
                  <div
                    className={isOpen ? styles.resultOpen : styles.result}
                    onClick={() => toggle(it.id)}
                    role="button"
                    tabIndex={-1}
                  >
                    {it.result}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </Spin>
    </Modal>
  );
};

export default HistoryModal;
