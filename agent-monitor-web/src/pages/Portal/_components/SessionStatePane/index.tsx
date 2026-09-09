import React from "react";

import { Button } from "@hsu-react/ui";
import { CloseOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../PortalStore";
import SessionPanels from "../SessionPanels";
import { sessionTitle } from "../../_utils/sessionNote";
import { isEmptySessionState, sessionStateOf } from "../../_utils/sessionState";
import styles from "./index.module.scss";

/**
 * 右栏的内容：**当前打开的每一条会话此刻在办什么**。
 *
 * 对应 VitaAgent 右栏的 `ActivityPanel`（任务进度那一栏）。差别在于那边一次
 * 只有一条会话，这里主区可以并排开到 4 格 —— 所以右栏按会话分节，一次把几格的
 * 状态都列出来，而不是只认其中一格。多格时才写会话名：单格时那行标题是废话。
 *
 * 会话状态本来排在每一格对话流的末尾。搬到这里之后它不再跟着对话滚走 ——
 * 「这个会话正在办什么」是**此刻的状态**，不是时序事件，往上翻历史时它不该消失。
 * 窄屏没有第三栏可摆，仍旧留在对话流末尾（见 ChatPane）。
 */
const SessionStatePane: React.FC = observer(() => {
  const { openTasks, toggleRightPane } = PortalStore;

  const withState = openTasks.filter(
    (t) => !isEmptySessionState(sessionStateOf(PortalStore.messagesOf(t.id ?? ""))),
  );
  const multi = openTasks.length > 1;

  return (
    <div className={styles.SessionStatePane}>
      <div className={styles.head}>
        <span className={styles.title}>会话状态</span>
        <Button
          size="small"
          type="text"
          icon={<CloseOutlined />}
          title="收起右栏"
          onClick={toggleRightPane}
        />
      </div>

      <div className={styles.body}>
        {withState.length === 0 ? (
          /* 空态照实说，不藏起整栏：栏一会儿在一会儿不在，正文宽度就会自己跳。
             也顺带告诉用户这块会长出什么来 —— 藏起来的话没人知道有这回事。 */
          <div className={styles.empty}>
            暂无进行中的任务清单、后台命令或子代理
          </div>
        ) : (
          withState.map((t) => (
            <section key={t.id} className={styles.group}>
              {multi && (
                <div className={styles.groupTitle}>{sessionTitle(t, "会话")}</div>
              )}
              <SessionPanels
                className={styles.panels}
                messages={PortalStore.messagesOf(t.id ?? "")}
                running={t.status === "running"}
              />
            </section>
          ))
        )}
      </div>
    </div>
  );
});

export default SessionStatePane;
