import React, { useEffect, useRef, useState } from "react";

import { Button } from "@hsu-react/ui";
import { Popconfirm, Spin, Tooltip } from "antd";
import {
  BranchesOutlined,
  CloseOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  StopOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import Composer from "../Composer";
import TerminalFeed from "../TerminalFeed";
import SessionPanels from "../SessionPanels";
import GitDiffModal from "../GitDiffModal";
import styles from "./index.module.scss";

interface ChatPaneProps {
  task: PortalTaskData;
  /** 是否显示关闭按钮（多格时） */
  closable?: boolean;
}

/** token 数量缩写：1.2k / 3.4M */
const fmtTokens = (n: number) => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
};

const ChatPane: React.FC<ChatPaneProps> = observer((props) => {
  const { task, closable } = props;
  const {
    messagesOf,
    isLoadingMessages,
    control,
    closePane,
    sendInput,
    recallInput,
    recallAllQueued,
    syncMessages,
  } = PortalStore;
  const chatRef = useRef<HTMLDivElement>(null);
  const stickBottomRef = useRef(true);
  const [gitOpen, setGitOpen] = useState(false);

  const id = task.id ?? "";
  const messages = messagesOf(id);
  const loading = isLoadingMessages(id);
  // 清单与后台任务已抽到下方的状态面板；排队中的任务（本地回显 + 终端原生队列）
  // 也不进对话流，改挂在对话框上方（见下方 queuedStrip）。
  const feedMessages = React.useMemo(
    () =>
      messages.filter(
        (m) =>
          m.role !== "todos" &&
          m.role !== "bgtasks" &&
          // 本地乐观回显（排队/待执行）不铺进内容流，改挂底部；被会话接受后由真实
          // 同步消息接管、正常入流
          !(m.local && (m.queued || m.delivered)),
      ),
    [messages],
  );

  // 底部「排队中」挂载项：本地回显（还在 hub 队列、可撤回）+ 终端里 claude 原生
  // 队列（queued_inputs，已被终端接收、撤不回，仅展示）。按内容去重，本地项优先
  // （带 cmdId 可撤回）。
  const queuedItems = React.useMemo(() => {
    const norm = (s: string) => s.replace(/\s+/g, " ").trim();
    const items: {
      text: string;
      cmdId?: string;
      recallable: boolean;
    }[] = [];
    const seen = new Set<string>();
    // 只挂「还在 hub 队列、尚未注入终端」的本地回显（可撤回）；已注入终端的排队状态
    // 一律以 queued_inputs（终端 queue-operation 的真实队列）为准 —— 它在任务被会话
    // 接受时随 remove 出列而清掉，不会像 delivered 回显那样卡着显示「排队中」。
    for (const m of messages) {
      if (m.local && m.queued) {
        const k = norm(m.content);
        if (!seen.has(k)) {
          seen.add(k);
          items.push({ text: m.content, cmdId: m.cmdId, recallable: true });
        }
      }
    }
    for (const t of task.queuedInputs ?? []) {
      const k = norm(t);
      if (!seen.has(k)) {
        seen.add(k);
        items.push({ text: t, recallable: false });
      }
    }
    return items;
  }, [messages, task.queuedInputs]);

  useEffect(() => {
    const el = chatRef.current;
    if (el && stickBottomRef.current) {
      el.scrollTop = el.scrollHeight;
      // Markdown/代码块/字体异步渲染会让高度继续涨，同步滚一次会差一截：
      // 下一帧再钉一次兜住首帧的增量
      requestAnimationFrame(() => {
        const cur = chatRef.current;
        if (cur && stickBottomRef.current) {
          cur.scrollTop = cur.scrollHeight;
        }
      });
    }
  }, [messages]);

  // 内容高度变化（渲染完成、折叠展开等）时，只要用户仍在底部就保持钉底；
  // 用户主动上滚后 stickBottomRef 为 false，不会抢滚动。
  useEffect(() => {
    const el = chatRef.current;
    if (!el) {
      return;
    }
    const ro = new ResizeObserver(() => {
      if (stickBottomRef.current) {
        el.scrollTop = el.scrollHeight;
      }
    });
    ro.observe(el);
    if (el.firstElementChild) {
      ro.observe(el.firstElementChild);
    }
    return () => ro.disconnect();
  }, []);

  const onChatScroll = () => {
    const el = chatRef.current;
    if (el) {
      stickBottomRef.current =
        el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    }
  };

  const paused = task.status === "paused";
  const controllable = !!task.pid;

  return (
    <div className={styles.ChatPane}>
      <header className={styles.paneHeader}>
        <div className={styles.headInfo}>
          {/* 状态放标题前；标题只显示会话标题（设备/IDE/PID 等杂项不再展示） */}
          <div className={styles.headTitle}>
            <span
              className={`${styles.statusChip} ${styles[task.status ?? ""] ?? ""}`}
            >
              {task.statusDsr}
            </span>
            <span className={styles.headTitleText}>
              {task.title || task.prompt || task.projectName || "会话"}
            </span>
          </div>
          <div className={styles.headMeta}>
            <span>{task.projectName}</span>
            {task.usedTokens5h ? (
              <Tooltip title="近 5 小时 token 用量（输入 + 输出 + 缓存创建）">
                <span className={styles.tokenChip}>
                  5h · {fmtTokens(task.usedTokens5h)}
                </span>
              </Tooltip>
            ) : null}
          </div>
        </div>
        <div className={styles.headActions}>
          <Tooltip title="重新同步该终端的对话内容">
            <Button
              size="small"
              type="text"
              icon={<SyncOutlined spin={loading} />}
              onClick={() => syncMessages(id)}
            />
          </Tooltip>
          <Tooltip title="查看代码改动（git diff）">
            <Button
              size="small"
              type="text"
              icon={<BranchesOutlined />}
              onClick={() => setGitOpen(true)}
            />
          </Tooltip>
          <Tooltip title={paused ? "恢复" : "暂停"}>
            <Button
              size="small"
              type="text"
              icon={paused ? <PlayCircleOutlined /> : <PauseCircleOutlined />}
              disabled={!controllable}
              onClick={() => control(id, paused ? "resume" : "pause")}
            />
          </Tooltip>
          <Tooltip title="中断当前任务">
            <Button
              size="small"
              type="text"
              icon={<ThunderboltOutlined />}
              disabled={!controllable}
              onClick={() => control(id, "interrupt")}
            />
          </Tooltip>
          <Popconfirm
            title="确定终止该任务进程？"
            okText="终止"
            cancelText="取消"
            onConfirm={() => control(id, "stop")}
            disabled={!controllable}
          >
            <Tooltip title="终止进程">
              <Button
                size="small"
                type="text"
                danger
                icon={<StopOutlined />}
                disabled={!controllable}
              />
            </Tooltip>
          </Popconfirm>
          {closable ? (
            <Tooltip title="关闭此格">
              <Button
                size="small"
                type="text"
                icon={<CloseOutlined />}
                onClick={() => closePane(id)}
              />
            </Tooltip>
          ) : null}
        </div>
      </header>

      <div className={styles.chat} ref={chatRef} onScroll={onChatScroll}>
        <Spin spinning={loading}>
          {!loading && feedMessages.length === 0 ? (
            <div className={styles.chatEmpty}>
              <div className={styles.big}>💬</div>
              <div>该会话暂无可展示的对话内容</div>
            </div>
          ) : (
            <div className={styles.chatColumn}>
              <TerminalFeed
                messages={feedMessages}
                running={task.status === "running"}
                providerDsr={task.providerDsr}
                onRecall={(cmdId) => recallInput(id, cmdId)}
                onAnswer={(text) => sendInput(id, text)}
              />
            </div>
          )}
        </Spin>
      </div>

      {/* 清单与后台任务是「当前状态」而非时序事件：悬浮在本格右侧、可收起 */}
      <SessionPanels messages={messages} running={task.status === "running"} />

      {queuedItems.length > 0 && (
        <div className={styles.queuedStrip}>
          <div className={styles.chatColumn}>
            <div className={styles.queuedHead}>
              <span className={styles.queuedTitle}>
                <span className={styles.queuedDot} />
                终端排队中 · {queuedItems.length}
              </span>
              {queuedItems.some((q) => q.recallable) ? (
                <span
                  className={styles.recallAll}
                  role="button"
                  tabIndex={0}
                  onClick={() =>
                    recallAllQueued(
                      id,
                      queuedItems
                        .filter((q) => q.recallable && q.cmdId)
                        .map((q) => q.cmdId as string),
                      queuedItems
                        .filter((q) => q.recallable)
                        .map((q) => q.text)
                        .join("\n"),
                    )
                  }
                >
                  全部撤回
                </span>
              ) : null}
            </div>
            <div className={styles.queuedList}>
              {queuedItems.map((q, i) => (
                <div key={i} className={styles.queuedItem}>
                  <span className={styles.queuedItemText}>{q.text}</span>
                  {!q.recallable && (
                    <span className={styles.queuedTag}>已入终端队列</span>
                  )}
                </div>
              ))}
            </div>
            {queuedItems.some((q) => !q.recallable) ? (
              <div className={styles.queuedHint}>
                已进终端原生队列的任务，在终端里按 ↑ 键可撤回（撤回后此处自动同步移除）
              </div>
            ) : null}
          </div>
        </div>
      )}

      <div className={styles.composerWrap}>
        <div className={styles.chatColumn}>
          <Composer
            taskId={id}
            disabled={!controllable}
            machineId={task.machineId}
            cwd={task.process?.cwd}
            onSend={(text) => {
              sendInput(id, text);
              // 发送后强制滚到底部：即使之前上滚看历史，发出内容也应带着滚回底部
              stickBottomRef.current = true;
              requestAnimationFrame(() => {
                const el = chatRef.current;
                if (el) el.scrollTop = el.scrollHeight;
              });
            }}
          />
        </div>
      </div>

      <GitDiffModal
        open={gitOpen}
        taskId={id}
        title={task.projectName}
        onClose={() => setGitOpen(false)}
      />
    </div>
  );
});

export default ChatPane;
