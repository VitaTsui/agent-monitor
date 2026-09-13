import React from "react";

import { Button } from "@hsu-react/ui";
import { Tooltip } from "antd";
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
 * 对应 VitaAgent 右栏的 `ActivityPanel`（任务进度那一栏），形制照抄
 * （`web/src/pages/chat/_components/ActivityPanel/index.module.scss:1-70`）：
 * **一张浮着的卡**（圆角 8 ＋ `0 0 0 1px` 描边环 ＋ 一层柔和投影、**高度随内容**），
 * 卡里头一行 12 内边距、标题 14/400，下面一块块 12 内边距、块间一条发丝线。
 *
 * 那张卡就画在这一层。外面的 `RightPane` 只负责「露底 ＋ 滚动 ＋ 拖宽」，
 * 一点底色和边都不画 —— **两层都画卡就是两圈框套着**，而参照的观感恰恰来自
 * 「露底让内卡浮起来」，不来自外面那圈框。
 *
 * 从前这里遍历 `openTasks` 把所有打开的会话都列一遍 —— 因为那时整页只有一条右栏，
 * 它没有主语，只能把几格的状态堆在一起。现在栏长在每一格里，主语就是本格：
 * 一栏一会话，不再有「这张卡是哪一格的」这种要靠小标题才能分清的事。
 *
 * **这一栏里没有子代理**：它是执行链上的一步，就地画在正文里（见 `AgentCard`）。
 * 留在这儿的两样都有同一个理由 —— 它们在链上无处可挂：任务清单是每轮重算的状态，
 * 后台命令压根不产生链节点。
 *
 * 会话状态本来排在每一格对话流的末尾。搬到这里之后它不再跟着对话滚走 ——
 * 「这个会话正在办什么」是**此刻的状态**，不是时序事件，往上翻历史时它不该消失。
 * 窄屏没有第三栏可摆，仍旧留在对话流末尾（见 ChatPane）。
 */
const SessionStatePane: React.FC<SessionStatePaneProps> = observer((props) => {
  const { task } = props;
  const id = task.id ?? "";
  const messages = PortalStore.messagesOf(id);
  /* 全量那份（按需拉回来的），不是上报捎带的 `task.subTasks` —— 后者只有活跃会话
     才有、且只覆盖近 24 小时 / 50 条。ChatPane 已经负责把它拉上来了 */
  const subTasks = PortalStore.subTasksOf(id);
  const empty = isEmptySessionState(sessionStateOf(messages, subTasks));

  return (
    <div className={styles.SessionStatePane}>
      <div className={styles.card}>
        <div className={styles.head}>
          <span className={styles.title}>会话状态</span>
          {/* 收起：只收本格这一栏，不动别的格。
            **说明文字只能走 Tooltip / aria-label，不能走 `title`**：
            hsu-ui 的 `Button` 把 `title` 当**按钮文案**用（`children ?? title`，
            见 `@hsu-react/ui/es/components/Button/index.js:91`），不是原生的悬停
            提示 —— 于是「收起这一格的会话状态栏」被原样印在叉号右边，
            24px 宽的按钮里塞一整句话，溢出到栏外把标题挤没了。
            这颗按钮只留图标，语义由 `aria-label` 承担、提示由 Tooltip 给。 */}
          <Tooltip title="收起这一格的会话状态栏">
            <Button
              size="small"
              type="text"
              className={styles.close}
              icon={<CloseOutlined />}
              aria-label="收起这一格的会话状态栏"
              onClick={() => PortalStore.toggleRightPane(id)}
            />
          </Tooltip>
        </div>

        {empty ? (
          /* 空态照实说，不藏起整栏：栏一会儿在一会儿不在，正文宽度就会自己跳。
           也顺带告诉用户这块会长出什么来 —— 藏起来的话没人知道有这回事。 */
          <div className={styles.empty}>暂无进行中的任务清单或后台命令</div>
        ) : (
          /* `flat`：卡内分区交给这一层的发丝线，`SessionPanels` 不再自带描边卡。
           同一份内容在窄屏是**对话流末尾的一组独立卡片**（那儿没有外卡可依），
           在这儿是**一张卡里的几块** —— 边框与圆角属于所处的位置，不属于内容。 */
          <SessionPanels
            flat
            messages={messages}
            subTasks={subTasks}
            running={task.status === "running"}
          />
        )}
      </div>
    </div>
  );
});

export default SessionStatePane;
