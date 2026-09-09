import React, { useEffect, useRef, useState } from "react";

import { Markdown, message } from "@hsu-react/ui";
import { Empty, Spin } from "antd";
import { LeftOutlined } from "@ant-design/icons";
import { useNavigate, useParams } from "react-router-dom";

import { SessionHistoryItem, getSessionHistory } from "@/services/apis/portal";
import { PORTAL_BASE } from "../../_utils/portalNav";
import views from "../views.module.scss";
import styles from "./index.module.scss";

/** epoch 秒 → 「14:23」/「08-01 14:23」，今天的省掉日期 */
const fmtTime = (sec: number) => {
  const d = new Date(sec * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  if (sameDay) return hm;
  const md = `${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate(),
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
 * 远程往来（`/portal/history/:taskId`）：把「我发了什么 → 它回了什么」排成一条对话流。
 *
 * 从前是 720 宽的弹窗，现在是一页 —— 一条会话的往来动辄几十屏，弹窗里读不了，
 * 而且刷新就没了、链接也发不出去。
 *
 * 读法与聊天记录一致：旧的在上、新的在下，打开默认滚到底部。
 * 我发的靠右，它回的靠左；只看当前会话，不混别的终端。
 */
const HistoryView: React.FC = () => {
  const navigate = useNavigate();
  const { taskId } = useParams<{ taskId: string }>();
  const [list, setList] = useState<SessionHistoryItem[]>([]);
  const [loading, setLoading] = useState(false);
  // 直接持有滚动容器：用 scrollIntoView 会把整页往上顶，改成设容器的 scrollTop
  const streamRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!taskId) return;
    setLoading(true);
    getSessionHistory(200, taskId)
      .then((res) => setList(res?.data?.list ?? []))
      .catch(() => message.error("获取历史失败"))
      .finally(() => setLoading(false));
  }, [taskId]);

  // 数据到位后滚到底 —— 聊天记录最该先看到的是最新那条。
  // 放在下一帧：此刻 markdown 尚未完成布局，立即设 scrollTop 会按旧高度算而滚不到底。
  useEffect(() => {
    if (loading || list.length === 0) return;
    const el = streamRef.current;
    if (!el) return;
    const toBottom = () => {
      el.scrollTop = el.scrollHeight;
    };
    const raf = requestAnimationFrame(toBottom);
    // markdown / 代码块渲染完还会再撑高一次，补一拍兜底
    const timer = window.setTimeout(toBottom, 120);
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(timer);
    };
  }, [loading, list]);

  return (
    <div className={views.pageFixed}>
      <div>
        <div className={`${views.fixedHead} ${styles.headSafe}`}>
          <div className={views.headRow}>
            <span
              className={views.headBtn}
              role="button"
              tabIndex={0}
              aria-label="返回会话"
              onClick={() => navigate(PORTAL_BASE)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  navigate(PORTAL_BASE);
                }
              }}
            >
              <LeftOutlined />
            </span>
            <span className={views.headTitle}>远程往来</span>
          </div>
          <div className={styles.hint}>
            经钉钉 / 网页 / MCP 下发的任务与其结果。终端关掉、机器关机后仍可在此回看。
          </div>
        </div>

        <div className={views.fixedBody} ref={streamRef}>
          {loading && (
            <div className={styles.loading}>
              <Spin />
            </div>
          )}
          {!loading && list.length === 0 && (
            <Empty
              description={taskId ? "这个会话还没有远程往来记录" : "无法确定当前会话"}
            />
          )}
          {!loading &&
            list.map((it) => {
              const isUser = it.role === "user";
              return (
                <div
                  key={it.id}
                  className={isUser ? styles.rowUser : styles.rowAgent}
                >
                  <div className={isUser ? styles.bubbleUser : styles.bubbleAgent}>
                    {/* 结果里满是代码块/列表/表格，纯文本读不了，交给 markdown 渲染；
                        我发的指令通常是一句话，但也可能贴了代码，一并渲染保持一致 */}
                    <Markdown.Views>{it.content}</Markdown.Views>
                  </div>
                  <div className={styles.time}>
                    {fmtTime(it.at)}
                    {isUser && SOURCE_LABEL[it.source]
                      ? ` · ${SOURCE_LABEL[it.source]}`
                      : ""}
                  </div>
                </div>
              );
            })}
        </div>
      </div>
    </div>
  );
};

export default HistoryView;
