import React from "react";

import { Button } from "@hsu-react/ui";
import { Tooltip } from "antd";
import { CloseOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import SessionPanels from "../SessionPanels";
import styles from "./index.module.scss";

interface SessionStatePaneProps {
  /** 这一栏说的是哪一条会话。栏归本格所有，内容也只能是本格的 */
  task: PortalTaskData;
}

/**
 * 一格右栏的内容：**这一条会话此刻在办什么**。
 *
 * 对应 VitaAgent 右栏的 `ActivityPanel`（任务进度那一栏），排版照抄
 * （`web/src/pages/chat/_components/ActivityPanel/index.module.scss:23-47`）：
 * 头一行 12 内边距、标题 14/400，下面一块块 12 内边距、块间一条发丝线。
 *
 * **卡的那圈壳没有了**：栏本身已经是一整条白底（`RightPane` 的 `--card`），
 * 白底上再压一张白卡等于多画一圈看不见的框。圆角、描边环、投影一并撤掉，
 * 分区仍靠发丝线区隔。
 *
 * 从前这里遍历 `openTasks` 把所有打开的会话都列一遍 —— 因为那时整页只有一条右栏，
 * 它没有主语，只能把几格的状态堆在一起。现在栏长在每一格里，主语就是本格：
 * 一栏一会话，不再有「这张卡是哪一格的」这种要靠小标题才能分清的事。
 *
 * **这一栏里没有子代理**：它是执行链上的一步，就地画在正文里（见 `AgentCard`）。
 * 留在这儿的两样都有同一个理由 —— 它们在链上无处可挂：任务清单是每轮重算的状态，
 * 后台命令压根不产生链节点。
 *
 * **没有空态分支。** 这条会话没东西可说的时候，整条栏根本不渲染、开关也是灰的
 * （判据在 ChatPane 的 `hasSessionState`，只有一处）—— 所以走到这里就一定有内容。
 * 上一版在这里画一句「暂无进行中的任务清单或后台命令」，那是「栏一定在」年代的
 * 做法；现在留着它就是第二套「空了怎么办」的判断。
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

  return (
    <div className={styles.SessionStatePane}>
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

      {/* `flat`：栏内分区交给这一层的发丝线，`SessionPanels` 不再自带描边卡。
        同一份内容在窄屏是**对话流末尾的一组独立卡片**（那儿没有白栏可依），
        在这儿是**一条白栏里的几块** —— 边框与圆角属于所处的位置，不属于内容。 */}
      <SessionPanels
        flat
        messages={messages}
        subTasks={subTasks}
        running={task.status === "running"}
      />
    </div>
  );
});

export default SessionStatePane;
