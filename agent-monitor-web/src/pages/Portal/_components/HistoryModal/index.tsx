import React, { useEffect, useRef, useState } from "react";

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

/** epoch 秒 → 「14:23」/「08-01 14:23」，今天的省掉日期 */
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
  if (sameDay) return hm;
  const md = `${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
  return `${md} ${hm}`;
};

/** 下发来源的可读标签，让你回看时知道当时是在哪儿发的 */
const SOURCE_LABEL: Record<string, string> = {
  dingtalk: "钉钉",
  web: "网页",
  mcp: "MCP",
};

/**
 * 远程交互历史：把「我发了什么 → 它回了什么」排成一条对话流。
 *
 * 读法与聊天记录一致：旧的在上、新的在下，打开默认滚到底部。
 * 我发的靠右，它回的靠左；切换会话时插一条分隔，避免多个终端的往来混在一起分不清。
 */
const HistoryModal: React.FC<HistoryModalProps> = ({ open, onClose }) => {
  const [list, setList] = useState<SessionHistoryItem[]>([]);
  const [loading, setLoading] = useState(false);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    getSessionHistory(200)
      .then((res) => setList(res?.data?.list ?? []))
      .catch(() => message.error("获取历史失败"))
      .finally(() => setLoading(false));
  }, [open]);

  // 数据到位后滚到底 —— 聊天记录最该先看到的是最新那条
  useEffect(() => {
    if (!loading && list.length > 0) {
      bottomRef.current?.scrollIntoView({ block: "end" });
    }
  }, [loading, list]);

  return (
    <Modal
      title="远程往来"
      open={open}
      onCancel={onClose}
      footer={null}
      width={720}
      centered
    >
      <div className={styles.hint}>
        经钉钉 / 网页 / MCP 下发的任务与其结果。终端关掉、机器关机后仍可在此回看。
      </div>
      <Spin spinning={loading}>
        <div className={styles.stream}>
          {!loading && list.length === 0 && (
            <Empty description="还没有远程往来记录" />
          )}
          {list.map((it, idx) => {
            // 换会话时插分隔条：多个终端的往来混在一起时，得知道这段是谁的
            const prev = idx > 0 ? list[idx - 1] : null;
            const newSession = !prev || prev.sessionId !== it.sessionId;
            const isUser = it.role === "user";
            return (
              <React.Fragment key={it.id}>
                {newSession && (
                  <div className={styles.divider}>
                    {it.slot != null && (
                      <span className={styles.slot}>{it.slot}</span>
                    )}
                    <span className={styles.dividerText}>
                      {it.project}
                      {it.title ? ` · ${it.title}` : ""}
                    </span>
                    <span className={styles.dividerMeta}>{it.hostname}</span>
                  </div>
                )}
                <div className={isUser ? styles.rowUser : styles.rowAgent}>
                  <div className={isUser ? styles.bubbleUser : styles.bubbleAgent}>
                    {it.content}
                  </div>
                  <div className={styles.time}>
                    {fmtTime(it.at)}
                    {isUser && SOURCE_LABEL[it.source]
                      ? ` · ${SOURCE_LABEL[it.source]}`
                      : ""}
                  </div>
                </div>
              </React.Fragment>
            );
          })}
          <div ref={bottomRef} />
        </div>
      </Spin>
    </Modal>
  );
};

export default HistoryModal;
