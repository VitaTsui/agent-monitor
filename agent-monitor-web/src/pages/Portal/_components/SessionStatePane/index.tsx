import React from "react";

import { Button } from "@hsu-react/ui";
import { CloseOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import SessionPanels from "../SessionPanels";
import { isEmptySessionState, sessionStateOf } from "../../_utils/sessionState";
import styles from "./index.module.scss";

interface SessionStatePaneProps {
  /** 这一栏说的是哪一条会话。栏归本格所有，内容也只能是本格的 */
  task: PortalTaskData;
}

/**
 * 一格右栏的内容：**这一条会话此刻在办什么**。
 *
 * 对应 VitaAgent 右栏的 `ActivityPanel`（任务进度那一栏）：头一行 12 内边距、
 * 标题 14/400，下面一块块用发丝线分开。
 *
 * 从前这里遍历 `openTasks` 把所有打开的会话都列一遍 —— 因为那时整页只有一条右栏，
 * 它没有主语，只能把几格的状态堆在一起。现在栏长在每一格里，主语就是本格：
 * 一栏一会话，不再有「这张卡是哪一格的」这种要靠小标题才能分清的事。
 *
 * 会话状态本来排在每一格对话流的末尾。搬到这里之后它不再跟着对话滚走 ——
 * 「这个会话正在办什么」是**此刻的状态**，不是时序事件，往上翻历史时它不该消失。
 * 窄屏没有第三栏可摆，仍旧留在对话流末尾（见 ChatPane）。
 */
const SessionStatePane: React.FC<SessionStatePaneProps> = observer((props) => {
  const { task } = props;
  const id = task.id ?? "";
  const messages = PortalStore.messagesOf(id);
  const empty = isEmptySessionState(sessionStateOf(messages, task.subTasks));

  return (
    <div className={styles.SessionStatePane}>
      <div className={styles.head}>
        <span className={styles.title}>会话状态</span>
        {/* 收起：只收本格这一栏，不动别的格 */}
        <Button
          size="small"
          type="text"
          className={styles.close}
          icon={<CloseOutlined />}
          title="收起这一格的会话状态栏"
          onClick={() => PortalStore.toggleRightPane(id)}
        />
      </div>

      <div className={styles.body}>
        {empty ? (
          /* 空态照实说，不藏起整栏：栏一会儿在一会儿不在，正文宽度就会自己跳。
             也顺带告诉用户这块会长出什么来 —— 藏起来的话没人知道有这回事。 */
          <div className={styles.empty}>
            暂无进行中的任务清单、后台命令或子代理
          </div>
        ) : (
          <SessionPanels
            className={styles.panels}
            messages={messages}
            subTasks={task.subTasks}
            running={task.status === "running"}
          />
        )}
      </div>
    </div>
  );
});

export default SessionStatePane;
