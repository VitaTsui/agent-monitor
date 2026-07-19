import React, { useEffect, useState } from "react";

import { Input } from "@hsu-react/ui";
import { Badge, ConfigProvider, Popover, Tooltip } from "antd";
import { platformIcon } from "./_utils/platform";
import { useNativeBack } from "./_hooks/useNativeBack";
import { useApkUpdateCheck } from "./_hooks/useApkUpdateCheck";
import { useClientUpdateToast } from "./_hooks/useClientUpdateToast";
import { claimPairDevice } from "@/services/apis/portal";
import { message as antdMessage } from "antd";
import {
  CodeOutlined,
  ControlOutlined,
  DownOutlined,
  LaptopOutlined,
  LogoutOutlined,
  MenuFoldOutlined,
  MenuUnfoldOutlined,
  SafetyOutlined,
  SearchOutlined,
  SettingOutlined,
  SplitCellsOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { getAccessToken, getUserInfo, removeToken } from "@/utils/auth";
import { clientSilentLogin, inDesktopClient, localMachineId } from "@/utils/clientAuth";
import PortalStore from "./PortalStore";
import ChatPane from "./_components/ChatPane";
import ScrollText from "./_components/ScrollText";
import SettingsModal from "./_components/SettingsModal";
import type { SettingsTab } from "./_components/SettingsModal";
import styles from "./index.module.scss";

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
    init,
    refresh,
    loadDevices,
    stopPolling,
    select,
    splitOpen,
  } = PortalStore;
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("account");
  // 客户端窗口内标出「本机」（浏览器里为 null，不标）
  const [localId, setLocalId] = useState<string | null>(null);
  useEffect(() => {
    localMachineId().then(setLocalId);
  }, []);
  const [siderFolded, setSiderFolded] = useState(false);
  const [userMenuOpen, setUserMenuOpen] = useState(false);
  // 移动端：侧栏抽屉开合
  const [mobileNav, setMobileNav] = useState(false);

  // 移动端强制展开侧栏内容：桌面折叠态下缩窄窗口时，
  // CSS 会把抽屉撑到 84vw，但折叠态 JSX 不渲染内容 → 空白抽屉，这里在 JS 层纠正
  useEffect(() => {
    const mq = window.matchMedia("(max-width: 760px)");
    const sync = () => {
      if (mq.matches) {
        setSiderFolded(false);
      }
    };
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  // 移动端选中会话后自动收起抽屉
  const selectSession = (id: string) => {
    select(id);
    setMobileNav(false);
  };

  // 抽屉打开时：锁背景滚动 + Esc 关闭
  useEffect(() => {
    if (!mobileNav) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileNav(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prev;
      window.removeEventListener("keydown", onKey);
    };
  }, [mobileNav]);

  // 前台需登录：无 token 时，客户端窗口先用设备令牌静默续登（客户端登录
  // 态永不过期），浏览器（或续登失败）才跳登录页并带回跳地址
  useEffect(() => {
    if (!getAccessToken()) {
      if (inDesktopClient()) {
        clientSilentLogin().then((ok) => {
          if (ok) {
            window.location.reload();
          } else {
            window.location.href = "/login?redirect=%2Fportal";
          }
        });
      } else {
        window.location.href = "/login?redirect=%2Fportal";
      }
      return;
    }
    init();

    return () => {
      stopPolling();
    };
  }, [init, stopPolling]);

  // 移动端更新推送：原生壳内检测 APK 新版本（浏览器里空转）
  useApkUpdateCheck();
  // 客户端窗口内右下角的新版本提醒（浏览器里空转）
  useClientUpdateToast();
  // 客户端窗口内：Cmd/Ctrl+R 刷新页面（webview 默认不绑，站点发新版可手动拉最新）
  useEffect(() => {
    if (!inDesktopClient()) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "r" || e.key === "R")) {
        e.preventDefault();
        window.location.reload();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 设备配对认领：客户端窗口带 ?pair=码 打开本页，登录后自动把那台电脑
  // 绑定到当前账号（绑定即信任），页面随即出现该设备 —— 用户零手工配置。
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const code = params.get("pair");
    if (!code) return;
    // 无论成败都摘掉参数，避免刷新重复认领
    const clean = () => {
      params.delete("pair");
      const q = params.toString();
      window.history.replaceState(null, "", window.location.pathname + (q ? `?${q}` : ""));
    };
    claimPairDevice(code)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(
            `已绑定本机「${res.data?.hostname ?? ""}」到你的账号，终端会话马上出现`
          );
          refresh();
          loadDevices();
        } else {
          antdMessage.warning(res.msg || "配对码无效或已过期，请重启客户端重试");
        }
      })
      .catch(() => antdMessage.error("绑定失败，请检查网络后重启客户端重试"))
      .finally(clean);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 原生壳（Android）的返回键：优先关掉当前浮层，都没有才交还系统语义。
  // 不接管的话按返回会直接退出整个 App。浏览器里此钩子空转。
  useNativeBack(() => {
    if (mobileNav) {
      setMobileNav(false);
      return true;
    }
    if (settingsOpen) {
      setSettingsOpen(false);
      return true;
    }
    if (userMenuOpen) {
      setUserMenuOpen(false);
      return true;
    }
    return false;
  });

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
        <span className={styles.userMenuName}>{nickname}</span>
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
    // Portal 路由挂在全局 Theme 之外，自带品牌主色；
    // ConfigProvider 走 React context，portal 出去的弹窗一样生效。
    <ConfigProvider theme={{ token: { colorPrimary: "#0F9BAD" } }}>
    <div className={styles.Portal}>
      {/* 移动端顶部栏（仅窄屏显示） */}
      <div className={styles.mobileBar}>
        <span
          className={styles.mobileMenuBtn}
          role="button"
          tabIndex={0}
          aria-label="打开会话列表"
          onClick={() => setMobileNav(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              setMobileNav(true);
            }
          }}
        >
          <MenuUnfoldOutlined />
        </span>
        <span className={styles.mobileTitle}>
          {openTasks[0]?.title ||
            openTasks[0]?.prompt ||
            openTasks[0]?.projectName ||
            "终端任务监控"}
        </span>
      </div>

      {/* 移动端抽屉遮罩 */}
      {mobileNav && (
        <div
          className={styles.mobileBackdrop}
          onClick={() => setMobileNav(false)}
        />
      )}

      <aside
        className={`${styles.sider} ${siderFolded ? styles.folded : ""} ${
          mobileNav ? styles.mobileOpen : ""
        }`}
      >
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
              role="button"
              tabIndex={0}
              aria-label={siderFolded ? "展开侧栏" : "收起侧栏"}
              aria-expanded={!siderFolded}
              onClick={() => setSiderFolded(!siderFolded)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setSiderFolded(!siderFolded);
                }
              }}
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
                      role="button"
                      tabIndex={0}
                      aria-pressed={d.machineId === selectedMachineId}
                      onClick={() => selectMachine(d.machineId)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          selectMachine(d.machineId);
                        }
                      }}
                      title={`${d.hostname} · ${d.platformDsr}`}
                    >
                      <LaptopOutlined />
                      <ScrollText
                        className={styles.deviceTabName}
                        active={d.machineId === selectedMachineId}
                        plain={d.hostname}
                        text={
                          <>
                            {platformIcon(d.platform)} {d.hostname}
                            {d.machineId === localId ? (
                              <span className={styles.localTag}>本机</span>
                            ) : null}
                          </>
                        }
                      />
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
                    return (
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
                              onClick={() => selectSession(t.id ?? "")}
                              onKeyDown={(e) => {
                                if (e.key === "Enter" || e.key === " ") {
                                  e.preventDefault();
                                  selectSession(t.id ?? "");
                                }
                              }}
                            >
                              <span
                                className={`${styles.dot} ${
                                  styles[t.status ?? ""] ?? ""
                                }`}
                              />
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
                                    className={`${styles.sessStatus} ${
                                      styles[t.status ?? ""] ?? ""
                                    }`}
                                  >
                                    {STATUS_LABEL[t.status ?? ""] ?? t.statusDsr}
                                  </span>
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
          {/* Popover 的 click 触发挂在本元素注入的 onClick 上，键盘路径合成一次
              click 即可复用；不加 role/tabIndex 的话，设置/设备管理/安全防护/
              后台管理/退出登录全部无法用键盘抵达（它们没有别的入口）。 */}
          <div
            className={styles.userRow}
            title="账户与设置"
            role="button"
            tabIndex={0}
            aria-label="账户与设置"
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                e.currentTarget.click();
              }
            }}
          >
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
    </ConfigProvider>
  );
});

export default Portal;
