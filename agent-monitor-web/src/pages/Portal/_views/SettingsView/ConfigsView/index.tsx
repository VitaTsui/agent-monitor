import React from "react";

import ConfigSyncPanel from "../../../_components/ConfigSyncPanel";
import views from "../../views.module.scss";
import st from "../settings.module.scss";

/** 配置同步（`/portal/settings/configs`）。 */
const ConfigsView: React.FC = () => (
  <div className={views.pageFixed}>
    <div>
      <div className={`${views.fixedHead} ${st.paneHead}`}>
        <div className={views.headRow}>
          <span className={views.headTitle}>配置同步</span>
        </div>
      </div>
      <div className={views.fixedBody}>
        <div className={st.hint}>
          让多台电脑共用同一套 Claude Code / Codex 配置：选一台设备作为
          <strong>配置源</strong>，其余设备自动向它看齐。改动几十秒内送达，
          设备离线时等它上线继续。
        </div>
        <ConfigSyncPanel />
      </div>
    </div>
  </div>
);

export default ConfigsView;
