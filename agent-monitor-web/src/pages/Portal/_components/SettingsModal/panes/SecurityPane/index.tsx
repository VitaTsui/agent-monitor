import React, { useEffect, useState } from "react";

import { Button, Input, Switch, message } from "@hsu-react/ui";
import { Tag } from "antd";

import {
  BUILTIN_DANGER_PATTERNS,
  loadGuardConfig,
  saveGuardConfig,
} from "../../../../_utils/dangerCheck";
import st from "../../settings.module.scss";
import styles from "./index.module.scss";

/** 安全防护分栏：危险输入的多重确认配置。 */
const SecurityPane: React.FC = () => {
  const [guardEnabled, setGuardEnabled] = useState(true);
  const [guardPatterns, setGuardPatterns] = useState("");

  useEffect(() => {
    const cfg = loadGuardConfig();
    setGuardEnabled(cfg.enabled);
    setGuardPatterns(cfg.customPatterns.join("\n"));
  }, []);

  const saveGuard = (enabled: boolean, patterns: string) => {
    const saved = saveGuardConfig({
      enabled,
      customPatterns: patterns
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean),
    });
    if (!saved) {
      message.error("防护配置保存失败（浏览器存储不可用）");
    }
  };

  return (
    <>
      <div className={st.paneTitle}>安全防护</div>
      <div className={st.hint}>
        发布到终端会话的内容命中危险模式（如 Claude Code 的
        <code>bypass permissions</code>、<code>rm -rf</code> 等）时， 需要经过
        <strong>两步确认</strong>（风险告知 + 输入确认词）才会真正发布。
      </div>
      <div className={st.row}>
        <div className={st.rowInfo}>
          <div className={st.rowTitle}>启用危险输入防护</div>
        </div>
        <Switch
          checked={guardEnabled}
          onChange={(checked) => {
            setGuardEnabled(!!checked);
            // 开关只保存启用位，textarea 未保存的草稿不随开关落盘
            saveGuard(!!checked, loadGuardConfig().customPatterns.join("\n"));
          }}
        />
      </div>
      <div className={st.section}>
        <div className={st.sectionTitle}>内置危险模式</div>
        <div className={styles.builtinPatterns}>
          {BUILTIN_DANGER_PATTERNS.map((p) => (
            <Tag key={p.pattern} className={styles.patternTag}>
              {p.pattern}
            </Tag>
          ))}
        </div>
      </div>
      <div className={`${st.section} ${styles.patternEditor}`}>
        <div className={st.sectionTitle}>
          自定义危险模式（一行一个，子串匹配）
        </div>
        <Input.TextArea
          value={guardPatterns}
          onChange={(value) => setGuardPatterns(value)}
          autoSize={{ minRows: 3, maxRows: 6 }}
          placeholder={"例如：\ndrop table\nkubectl delete"}
        />
        <Button
          className={styles.guardSave}
          type="primary"
          onClick={() => {
            saveGuard(guardEnabled, guardPatterns);
            message.success("已保存安全防护配置");
          }}
        >
          保存
        </Button>
      </div>
    </>
  );
};

export default SecurityPane;
