import React, { useEffect, useState } from "react";

import { Input } from "@hsu-react/ui";
import { Badge, Popover, Tooltip } from "antd";
import {
  CheckOutlined,
  CodeOutlined,
  ControlOutlined,
  DownOutlined,
  LaptopOutlined,
  LogoutOutlined,
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  RightOutlined,
  SafetyOutlined,
  SearchOutlined,
  SettingOutlined,
  SplitCellsOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { getAccessToken, getUserInfo, removeToken } from "@/utils/auth";
import PortalStore from "./PortalStore";
import ChatPane from "./_components/ChatPane";
import SettingsModal from "./_components/SettingsModal";
import type { SettingsTab } from "./_components/SettingsModal";
import styles from "./index.module.scss";

const PLATFORM_ICON: Record<string, string> = {
  macos: "",
  windows: "🪟",
  linux: "🐧",
};

const STATUS_LABEL: Record<string, string> = {
  running: "执行中",
  idle: "等待输入",
  paused: "已暂停",
  finished: "已结束",
};

const Portal: React.FC = observer(() => {
  const {
    deviceList,
    selectedMachineId,
    selectMachine,
    selectedGroups,
    pendingCount,
    openIds,
    openTasks,
    keyword,
    setKeyword,
    isCollapsed,
    toggleCollapse,
    init,
    stopPolling,
    select,
    splitOpen,
  } = PortalStore;
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("account");
  const [siderFolded, setSiderFolded] = useState(false);
  const [userMenuOpen, setUserMenuOpen] = useState(false);

  // 前台需登录：无 token 跳登录并带回跳地址
  useEffect(() => {
    if (!getAccessToken()) {
      window.location.href = "/login?redirect=%2Fportal";
      return;
    }
    init();

    return () => {
      stopPolling();
    };
  }, [init, stopPolling]);

  if (!getAccessToken()) {
    return null;
  }

  const paneCount = openTasks.length;
  const user = (getUserInfo() as {
    nickname?: string;
    username?: string;
    isSuper?: boolean;
  }) ?? {};
  const nickname = user.nickname ?? user.username ?? "";

  const openSettings = (tab: SettingsTab) => {
    setSettingsTab(tab);
    setSettingsOpen(true);
    setUserMenuOpen(false);
  };

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  const userMenu = (
    <div className={styles.userMenu}>
      <div className={styles.userMenuEmail}>{user.username}</div>
      <div className={styles.userMenuAccount}>
        <span className={styles.userMenuAvatar}>{nickname.slice(0, 1) || "U"}</span>
        <span className={styles.userMenuAccText}>
          <span className={styles.userMenuName}>{nickname}</span>
          <span className={styles.userMenuSub}>
            {user.isSuper ? "超级管理员" : "普通用户"}
          </span>
        </span>
        <CheckOutlined className={styles.userMenuCheck} />
      </div>
      <div className={styles.userMenuDivider} />
      <div className={styles.userMenuItem} onClick={() => openSettings("account")}>
        <SettingOutlined />
        <span>设置</span>
      </div>
      <div className={styles.userMenuItem} onClick={() => openSettings("devices")}>
        <LaptopOutlined />
        <span>设备管理</span>
        {pendingCount ? (
          <Badge count={pendingCount} size="small" className={styles.userMenuBadge} />
        ) : null}
      </div>
      <div className={styles.userMenuItem} onClick={() => openSettings("security")}>
        <SafetyOutlined />
        <span>安全防护</span>
      </div>
      {user.isSuper ? (
        <div
          className={styles.userMenuItem}
          onClick={() => {
            window.open("/admin", "_blank");
            setUserMenuOpen(false);
          }}
        >
          <ControlOutlined />
          <span>后台管理</span>
        </div>
      ) : null}
      <div className={styles.userMenuDivider} />
      <div className={styles.userMenuItem} onClick={onLogout}>
        <LogoutOutlined />
        <span>退出登录</span>
      </div>
    </div>
  );

  return (
    <div className={styles.Portal}>
      <aside className={`${styles.sider} ${siderFolded ? styles.folded : ""}`}>
        <div className={styles.siderHeader}>
          <div className={styles.brand}>
            <span className={styles.logo}>
              <CodeOutlined />
            </span>
            {!siderFolded && <span className={styles.brandName}>终端任务监控</span>}
          </div>
          <Tooltip title={siderFolded ? "展开侧栏" : "收起侧栏"} placement="right">
            <span
              className={styles.foldBtn}
              onClick={() => setSiderFolded(!siderFolded)}
            >
              {siderFolded ? <MenuUnfoldOutlined /> : <MenuFoldOutlined />}
            </span>
          </Tooltip>
        </div>

        {!siderFolded && (
          <>
            <div className={styles.siderSearch}>
              <Input
                className={styles.search}
                placeholder="搜索会话 / 项目"
                prefix={<SearchOutlined className={styles.searchIcon} />}
                allowClear
                value={keyword}
                onChange={(value) => setKeyword(value)}
              />
            </div>

            <div className={styles.siderScroll}>
              {/* 设备 */}
              <div className={styles.sectionLabel}>设备</div>
              <div className={styles.deviceBar}>
                {deviceList.length === 0 ? (
                  <div className={styles.noDevice}>暂无设备</div>
                ) : (
                  deviceList.map((d) => (
                    <div
                      key={d.machineId}
                      className={`${styles.deviceTab} ${
                        d.machineId === selectedMachineId ? styles.active : ""
                      }`}
                      onClick={() => selectMachine(d.machineId)}
                      title={`${d.hostname} · ${d.platformDsr}`}
                    >
                      <LaptopOutlined />
                      <span className={styles.deviceTabName}>
                        {PLATFORM_ICON[d.platform] ?? ""} {d.hostname}
                      </span>
                      <span className={styles.deviceTabStat}>
                        {d.count} 会话
                        {d.running > 0 ? ` · ${d.running} 执行中` : ""}
                      </span>
                      {d.running > 0 ? (
                        <span className={styles.deviceTabDot} />
                      ) : null}
                    </div>
                  ))
                )}
              </div>

              {/* 会话 */}
              <div className={styles.sectionLabel}>会话</div>
              <div className={styles.sessionList}>
                {selectedGroups.length === 0 ? (
                  <div className={styles.emptyList}>
                    该设备暂无活跃会话。请确认 agent-task-monitor
                    正在该设备上运行，且已在设备管理中信任。
                  </div>
                ) : (
                  selectedGroups.map((g) => {
                    const gKey = `${selectedMachineId}-${g.key}`;
                    const gCollapsed = isCollapsed(gKey);
                    return (
                      <div key={g.key} className={styles.termGroup}>
                        <div
                          className={styles.termTitle}
                          onClick={() => toggleCollapse(gKey)}
                        >
                          {gCollapsed ? <RightOutlined /> : <DownOutlined />}
                          <span>{g.title}</span>
                          <span className={styles.termCount}>{g.tasks.length}</span>
                        </div>
                        {!gCollapsed &&
                          g.tasks.map((t) => (
                            <div
                              key={t.id}
                              className={`${styles.session} ${
                                openIds.includes(t.id ?? "") ? styles.active : ""
                              }`}
                              onClick={() => select(t.id ?? "")}
                            >
                              <span
                                className={`${styles.dot} ${
                                  styles[t.status ?? ""] ?? ""
                                }`}
                              />
                              <div className={styles.sessBody}>
                                <div className={styles.sessName}>
                                  {t.title || t.prompt || t.projectName || "新会话"}
                                </div>
                                <div className={styles.sessSub}>
                                  {t.projectName} ·{" "}
                                  {STATUS_LABEL[t.status ?? ""] ?? t.statusDsr}
                                </div>
                              </div>
                              <Tooltip title="拆分显示">
                                <SplitCellsOutlined
                                  className={styles.splitBtn}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    splitOpen(t.id ?? "");
                                  }}
                                />
                              </Tooltip>
                            </div>
                          ))}
                      </div>
                    );
                  })
                )}
              </div>
            </div>
          </>
        )}
        {siderFolded && <div className={styles.siderScroll} />}

        {/* 底部用户区（Claude 式） */}
        <Popover
          open={userMenuOpen}
          onOpenChange={setUserMenuOpen}
          content={userMenu}
          trigger="click"
          placement="topLeft"
          arrow={false}
          overlayClassName={styles.userMenuOverlay}
        >
          <div className={styles.userRow} title="账户与设置">
            <span className={styles.userAvatar}>{nickname.slice(0, 1) || "U"}</span>
            {!siderFolded && (
              <>
                <div className={styles.userText}>
                  <div className={styles.userName}>{nickname}</div>
                  <div className={styles.userPlan}>
                    {user.isSuper ? "超级管理员" : "普通用户"}
                  </div>
                </div>
                <Badge count={pendingCount} size="small">
                  <DownOutlined className={styles.userChevron} />
                </Badge>
              </>
            )}
          </div>
        </Popover>
      </aside>

      <main className={styles.main}>
        {paneCount === 0 ? (
          <div className={styles.mainEmpty}>
            <div className={styles.greeting}>
              <span className={styles.greetLogo}>
                <CodeOutlined />
              </span>
              你好，{nickname}
            </div>
            <div className={styles.greetSub}>从左侧选择一个终端会话查看执行内容</div>
            <div className={styles.hint}>
              点击会话右侧的 <SplitCellsOutlined /> 可并排显示多个任务
            </div>
          </div>
        ) : (
          <div className={styles.paneGrid} data-count={Math.min(paneCount, 4)}>
            {openTasks.map((t) => (
              <ChatPane key={t.id} task={t} closable={paneCount > 1} />
            ))}
          </div>
        )}
      </main>

      <SettingsModal
        open={settingsOpen}
        initialTab={settingsTab}
        onClose={() => setSettingsOpen(false)}
      />
    </div>
  );
});

export default Portal;
