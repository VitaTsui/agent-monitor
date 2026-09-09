import React from "react";

import ConfigSyncPanel from "../../../ConfigSyncPanel";
import st from "../../settings.module.scss";

/** 配置同步分栏。 */
const ConfigsPane: React.FC = () => (
  <>
    <div className={st.paneTitle}>配置同步</div>
    <div className={st.hint}>
      让多台电脑共用同一套 Claude Code / Codex 配置：选一台设备作为
      <strong>配置源</strong>，其余设备自动向它看齐。改动几十秒内送达，
      设备离线时等它上线继续。
    </div>
    <ConfigSyncPanel />
  </>
);

export default ConfigsPane;
