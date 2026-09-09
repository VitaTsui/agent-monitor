import React from "react";

import { CodeOutlined } from "@ant-design/icons";

import views from "../../views.module.scss";
import st from "../settings.module.scss";
import styles from "./index.module.scss";

/** 关于（`/portal/settings/about`）。 */
const AboutView: React.FC = () => (
  <div className={views.pageFixed}>
    <div>
      <div className={`${views.fixedHead} ${st.paneHead}`}>
        <div className={views.headRow}>
          <span className={views.headTitle}>关于</span>
        </div>
      </div>
      <div className={views.fixedBody}>
        <div className={styles.about}>
          <div className={styles.aboutLogo}>
            <CodeOutlined />
          </div>
          <div className={styles.aboutName}>终端任务监控</div>
          <div className={styles.aboutDesc}>
            监控多台电脑终端里 AI 编码代理（Claude Code、Codex 等）正在执行的任务，
            支持实时查看、控制与发布任务。会话内容纯实时读取、不落存储。
          </div>
        </div>
      </div>
    </div>
  </div>
);

export default AboutView;
