import React from "react";

import { Segmented } from "antd";
import { observer } from "mobx-react-lite";
// 外观三态（浅色 / 深色 / 跟随系统）的真源。走深路径而不是 `@hsu-react/ui/es/layout`：
// 那是个 barrel，会把只有后管才用的 Header / Menu / NavTabBar 一起拉进前台的包里。
// 写 `html[data-theme]` 的是根上的 Layout.Theme（src/index.tsx），两边同一个单例。
import ThemeStore from "@hsu-react/ui/es/layout/Theme/ThemeStore";
import type { Appearance } from "@hsu-react/ui/es/layout/Theme/ThemeStore";

import views from "../../views.module.scss";
import st from "../settings.module.scss";

/** 外观（`/portal/settings/appearance`）。 */
const AppearanceView: React.FC = observer(() => (
  <div className={views.pageFixed}>
    <div>
      <div className={`${views.fixedHead} ${st.paneHead}`}>
        <div className={views.headRow}>
          <span className={views.headTitle}>外观</span>
        </div>
      </div>
      <div className={views.fixedBody}>
        <div className={st.device}>
          <div className={st.devInfo}>
            <div className={st.devName}>主题</div>
            <div className={st.devMeta}>
              「跟随系统」会随操作系统的浅色 / 深色设置实时切换
            </div>
          </div>
          <Segmented<Appearance>
            value={ThemeStore.appearance}
            onChange={(v) => ThemeStore.setAppearance(v)}
            options={[
              { value: "light", label: "浅色" },
              { value: "dark", label: "深色" },
              { value: "system", label: "跟随系统" },
            ]}
          />
        </div>
      </div>
    </div>
  </div>
));

export default AppearanceView;
