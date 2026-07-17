import React, { useEffect, useRef } from "react";

import { Button } from "@hsu-react/ui";
import { Popconfirm, Spin, Tooltip } from "antd";
import {
  CloseOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  StopOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import Composer from "../Composer";
import TerminalFeed from "../TerminalFeed";
import styles from "./index.module.scss";

interface ChatPaneProps {
  task: PortalTaskData;
  /** 是否显示关闭按钮（多格时） */
  closable?: boolean;
}

const PLATFORM_ICON: Record<string, string> = {
  macos: "",
  windows: "🪟",
  linux: "🐧",
};

/** token 数量缩写：1.2k / 3.4M */
const fmtTokens = (n: number) => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
};

const ChatPane: React.FC<ChatPaneProps> = observer((props) => {
  const { task, closable } = props;
  const { messagesOf, isLoadingMessages, control, closePane, sendInput } =
    PortalStore;
  const chatRef = useRef<HTMLDivElement>(null);
  const stickBottomRef = useRef(true);

  const id = task.id ?? "";
  const messages = messagesOf(id);
  const loading = isLoadingMessages(id);

  useEffect(() => {
    const el = chatRef.current;
    if (el && stickBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages]);

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
          <div className={styles.headTitle}>
            {task.title || task.prompt || task.projectName || "会话"}
          </div>
          <div className={styles.headMeta}>
            <span>{task.projectName}</span>
            <span>
              {PLATFORM_ICON[task.platform ?? ""] ?? ""} {task.hostname}
            </span>
            <span>{task.ideDsr}</span>
            {task.pid ? <span>PID {task.pid}</span> : null}
            {task.usedTokens5h ? (
              <Tooltip title="近 5 小时 token 用量（输入 + 输出 + 缓存创建）">
                <span className={styles.tokenChip}>
                  5h · {fmtTokens(task.usedTokens5h)}
                  {task.tokenLimit ? ` / ${fmtTokens(task.tokenLimit)}` : ""}
                </span>
              </Tooltip>
            ) : null}
            {task.autoPaused ? (
              <span className={`${styles.statusChip} ${styles.paused}`}>
                额度暂停
              </span>
            ) : null}
            <span
              className={`${styles.statusChip} ${styles[task.status ?? ""] ?? ""}`}
            >
              {task.statusDsr}
            </span>
          </div>
        </div>
        <div className={styles.headActions}>
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
          {!loading && messages.length === 0 ? (
            <div className={styles.chatEmpty}>
              <div className={styles.big}>💬</div>
              <div>该会话暂无可展示的对话内容</div>
            </div>
          ) : (
            <div className={styles.chatColumn}>
              <TerminalFeed
                messages={messages}
                running={task.status === "running"}
              />
            </div>
          )}
        </Spin>
      </div>

      <div className={styles.composerWrap}>
        <div className={styles.chatColumn}>
          <Composer
            taskId={id}
            disabled={!controllable}
            onSend={(text) => sendInput(id, text)}
          />
        </div>
      </div>
    </div>
  );
});

export default ChatPane;
