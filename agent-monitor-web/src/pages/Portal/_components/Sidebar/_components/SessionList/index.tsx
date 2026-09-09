import React from "react";

import { Tooltip } from "antd";
import { SplitCellsOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../../../PortalStore";
import { sessionTitle } from "../../../../_utils/sessionNote";
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
  const { selectedGroups, openIds, splitOpen, deviceList, selectedMachineId } =
    PortalStore;

  if (selectedGroups.length === 0) {
    // 空列表有三种成因，各自该做的事完全不同，混成一句就在骗人：
    // 浏览器进来不自动选设备（见 PortalStore.selectedMachineId），此时会话全在、
    // 设备也全在线，却被告知「确认客户端在运行」—— 用户会去排查一台根本没坏的机器。
    const hint = !deviceList.length
      ? "还没有设备上报会话。请确认 agent-task-monitor 正在运行，且已在设备管理中信任。"
      : !selectedMachineId
        ? "在上方「设备」里点一台，这里就会列出它的会话。"
        : "这台设备当前没有活跃会话。在它的终端里开一个，就会出现在这里。";
    return <div className={styles.emptyList}>{hint}</div>;
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
                    plain={sessionTitle(t, "新会话")}
                    text={sessionTitle(t, "新会话")}
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
