import React from "react";

import IntegrationsPanel from "../../../IntegrationsPanel";
import st from "../../settings.module.scss";

/** 机器人管理分栏。 */
const BotsPane: React.FC = () => (
  <>
    <div className={st.paneTitle}>机器人管理</div>
    <div className={st.hint}>
      配置你自己的钉钉机器人：一个账号一个，它收到的消息就归你、推送也只发给你。
      配好后在钉钉里发指令就能遥控会话，任务完成/需要你决定时也会私聊提醒。
    </div>
    <IntegrationsPanel />
  </>
);

export default BotsPane;
