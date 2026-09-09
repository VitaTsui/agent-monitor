import React from "react";

import IntegrationsPanel from "../../../_components/IntegrationsPanel";
import views from "../../views.module.scss";
import st from "../settings.module.scss";

/** 机器人管理（`/portal/settings/bots`）。 */
const BotsView: React.FC = () => (
  <div className={views.pageFixed}>
    <div>
      <div className={`${views.fixedHead} ${st.paneHead}`}>
        <div className={views.headRow}>
          <span className={views.headTitle}>机器人管理</span>
        </div>
      </div>
      <div className={views.fixedBody}>
        <div className={st.hint}>
          配置你自己的钉钉机器人：一个账号一个，它收到的消息就归你、推送也只发给你。
          配好后在钉钉里发指令就能遥控会话，任务完成/需要你决定时也会私聊提醒。
        </div>
        <IntegrationsPanel />
      </div>
    </div>
  </div>
);

export default BotsView;
