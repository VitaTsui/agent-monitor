import React from "react";

import { Tooltip } from "antd";
import { SplitCellsOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../../../PortalStore";
import ScrollText from "../../../ScrollText";
import styles from "./index.module.scss";

const STATUS_LABEL: Record<string, string> = {
  running: "执行中",
  idle: "等待输入",
  paused: "已暂停",
  finished: "已结束",
};

interface SessionListProps {
  isMobile: boolean;
  onSelect: (id: string) => void;
}

/** 侧栏的会话列表（按终端类型分组）。 */
const SessionList: React.FC<SessionListProps> = observer((props) => {
  const { isMobile, onSelect } = props;
  const { selectedGroups, openIds, splitOpen } = PortalStore;

  if (selectedGroups.length === 0) {
    return (
      <div className={styles.emptyList}>
        该设备暂无活跃会话。请确认 agent-task-monitor
        正在该设备上运行，且已在设备管理中信任。
      </div>
    );
  }

  return (
    <div className={styles.sessionList}>
      {selectedGroups.map((g) => (
        <div key={g.key} className={styles.termGroup}>
          {/* 分组标签：会话直接铺开，不做展开收起 */}
          <div className={styles.termTitle}>
            <span>{g.title}</span>
            <span className={styles.termCount}>{g.tasks.length}</span>
          </div>
          {g.tasks.map((t) => (
            <div
              key={t.id}
              className={`${styles.session} ${
                openIds.includes(t.id ?? "") ? styles.active : ""
              }`}
              role="button"
              tabIndex={0}
              aria-current={openIds.includes(t.id ?? "")}
              onClick={() => onSelect(t.id ?? "")}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelect(t.id ?? "");
                }
              }}
            >
              <span className={`${styles.dot} ${styles[t.status ?? ""] ?? ""}`} />
              {/* 号位：与钉钉「#N」同一个编号，在手机上照着这个号下发 */}
              {t.slot != null && (
                <Tooltip title={`钉钉里发「#${t.slot} 内容」即下发到这个终端`}>
                  <span className={styles.sessSlot}>{t.slot}</span>
                </Tooltip>
              )}
              <div className={styles.sessBody}>
                {/* 标题 + 右侧状态徽标同一行；来源等杂项不再展示 */}
                <div className={styles.sessRow}>
                  <ScrollText
                    className={styles.sessName}
                    active={openIds.includes(t.id ?? "")}
                    plain={t.title || t.prompt || t.projectName || "新会话"}
                    text={t.title || t.prompt || t.projectName || "新会话"}
                  />
                  <span
                    className={`${styles.sessStatus} ${styles[t.status ?? ""] ?? ""}`}
                  >
                    {STATUS_LABEL[t.status ?? ""] ?? t.statusDsr}
                  </span>
                </div>
              </div>
              {/* 移动端窄屏不支持拆分并排，去掉拆分按钮，只单会话查看 */}
              {!isMobile && (
                <Tooltip title="拆分显示">
                  <SplitCellsOutlined
                    className={styles.splitBtn}
                    onClick={(e) => {
                      e.stopPropagation();
                      splitOpen(t.id ?? "");
                    }}
                  />
                </Tooltip>
              )}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
});

export default SessionList;
