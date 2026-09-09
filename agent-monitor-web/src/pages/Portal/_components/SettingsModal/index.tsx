import React, { useEffect, useState } from "react";

import { Button, Input, Modal, Switch } from "@hsu-react/ui";
import { Badge, Empty, Modal as AntModal, Popconfirm, Progress, Tag } from "antd";
import { message } from "@hsu-react/ui";
import {
  CloseOutlined,
  CloudSyncOutlined,
  CodeOutlined,
  InfoCircleOutlined,
  LaptopOutlined,
  LinkOutlined,
  LogoutOutlined,
  LeftOutlined,
  RightOutlined,
  RobotOutlined,
  SafetyOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import {
  PortalDevice,
  connectShare,
  disconnectShare,
} from "@/services/apis/portal";
import { getUserInfo, removeToken } from "@/utils/auth";
import { localMachineId } from "@/utils/clientAuth";
import PortalStore from "../../PortalStore";
import ShareModal from "../ShareModal";
import IntegrationsPanel from "../IntegrationsPanel";
import ConfigSyncPanel from "../ConfigSyncPanel";
import {
  BUILTIN_DANGER_PATTERNS,
  loadGuardConfig,
  saveGuardConfig,
} from "../../_utils/dangerCheck";
import styles from "./index.module.scss";

export type SettingsTab =
  | "account"
  | "devices"
  | "configs"
  | "bots"
  | "security"
  | "about";

interface SettingsModalProps {
  open?: boolean;
  /** 打开时定位到的分栏 */
  initialTab?: SettingsTab;
  onClose?: () => void;
}

type Tab = SettingsTab;

const PLATFORM_COLOR: Record<string, string> = {
  macos: "geekblue",
  windows: "cyan",
  linux: "orange",
};

const SettingsModal: React.FC<SettingsModalProps> = observer((props) => {
  const { open, initialTab, onClose } = props;
  const { devices, pendingCount, loadDevices, trustDevice, untrustDevice, deleteDevice } =
    PortalStore;
  const [tab, setTab] = useState<Tab>("account");

  // 每次打开时定位到调用方指定的分栏
  useEffect(() => {
    if (open && initialTab) {
      setTab(initialTab);
    }
  }, [open, initialTab]);
  // 安全防护（危险输入多重确认）
  const [guardEnabled, setGuardEnabled] = useState(true);
  const [guardPatterns, setGuardPatterns] = useState("");
  // 桌面客户端 IPC 桥：仅在客户端窗口内存在（浏览器里为 undefined，相关 UI 不渲染）
  const tauriInvoke = (
    window as unknown as {
      __TAURI__?: { core?: { invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } };
    }
  ).__TAURI__?.core?.invoke;
  // null = 尚未取到（或不在客户端内）
  const [autostart, setAutostart] = useState<boolean | null>(null);
  // 本机监控范围（终端列表 + 排除态；仅客户端窗口内）
  const [terminals, setTerminals] = useState<
    { key: string; name: string; excluded: boolean }[]
  >([]);

  const loadTerminals = () => {
    tauriInvoke?.("terminals_get")
      .then((v) => setTerminals((v as typeof terminals) ?? []))
      .catch(() => setTerminals([]));
  };

  // 客户端版本与更新（仅客户端窗口内）
  interface UpdateProgress {
    phase: "downloading" | "installing" | "restarting";
    received: number;
    total: number;
  }
  const [clientVer, setClientVer] = useState<{
    current: string;
    latest: string | null;
    progress?: UpdateProgress | null;
  } | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  // 桥接插件（Cursor/VSCode 扩展）版本与更新（仅客户端窗口内；旧客户端无此 IPC → 保持 null 不渲染）
  const [pluginVer, setPluginVer] = useState<{
    installed: string | null;
    latest: string;
  } | null>(null);
  const [checkingPlugin, setCheckingPlugin] = useState(false);

  const loadClientVersion = () => {
    tauriInvoke?.("update_status")
      .then((v) =>
        setClientVer(
          v as { current: string; latest: string | null; progress?: UpdateProgress | null },
        ),
      )
      .catch(() => setClientVer(null));
  };

  const loadPluginVersion = () => {
    tauriInvoke?.("plugin_status")
      .then((v) => setPluginVer(v as { installed: string | null; latest: string }))
      .catch(() => setPluginVer(null));
  };

  // 强制重装插件（从 hub 拉最新 vsix）；用于「安装 / 更新到 vX」
  const updatePlugin = () => {
    setCheckingPlugin(true);
    tauriInvoke?.("plugin_update")
      .then((v) => {
        const r = v as { installed: number; version: string | null };
        setPluginVer((p) => (p ? { ...p, installed: r.version } : p));
        if (r.installed > 0) {
          message.success(`已安装桥接插件 v${r.version ?? ""} 到 ${r.installed} 个编辑器，重载编辑器窗口即生效`);
        } else {
          message.warning("未检测到 Cursor/VSCode 命令行（在编辑器里执行「Shell Command: Install 'code'/'cursor' command in PATH」后重试）");
        }
      })
      .catch(() => message.error("插件安装失败"))
      .finally(() => setCheckingPlugin(false));
  };

  // 检查插件更新：重新读状态，已是最新给提示，有更新则按钮切成「更新到 vX」
  const checkPluginUpdate = () => {
    setCheckingPlugin(true);
    tauriInvoke?.("plugin_status")
      .then((v) => {
        const s = v as { installed: string | null; latest: string };
        setPluginVer(s);
        if (!s.installed) {
          message.info("未检测到已安装的桥接插件，点「安装插件」装上");
        } else if (s.installed === s.latest) {
          message.success(`桥接插件已是最新（v${s.installed}）`);
        }
      })
      .catch(() => message.error("检查失败"))
      .finally(() => setCheckingPlugin(false));
  };
  // 「更新中」必须是「确有新版本 + 有进度」才算——否则辅助下载（如桥接扩展 vsix）
  // 遗留的进度状态会把按钮卡在「更新中」不可点（客户端已是最新却显示更新中）。
  const updating = !!clientVer?.progress && !!clientVer?.latest;

  // 打开设置期间轮询版本/进度（更新中每 1.5s 刷新进度条）
  useEffect(() => {
    if (!open || !tauriInvoke) {
      return;
    }
    const timer = window.setInterval(loadClientVersion, 1500);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const checkUpdate = () => {
    setCheckingUpdate(true);
    tauriInvoke?.("update_status")
      .then((v) => {
        const s = v as { current: string; latest: string | null };
        setClientVer(s);
        if (s.latest) {
          AntModal.confirm({
            centered: true,
            title: `发现新版本 v${s.latest}`,
            content: `当前版本 v${s.current}。更新将自动完成并重启客户端。`,
            okText: "立即更新",
            cancelText: "稍后",
            onOk: () => {
              tauriInvoke?.("update_start").catch(() => message.error("启动更新失败"));
            },
          });
        } else {
          message.success(`已是最新版本（v${s.current}）`);
        }
      })
      .catch(() => message.error("检查更新失败"))
      .finally(() => setCheckingUpdate(false));
  };

  const toggleTerminal = (key: string, excluded: boolean) => {
    tauriInvoke?.("terminal_set_excluded", { key, excluded })
      .then(() => loadTerminals())
      .catch(() => message.error("设置失败"));
  };
  // 协助共享：主人管理弹窗的目标设备
  const [shareDevice, setShareDevice] = useState<PortalDevice | null>(null);
  // 接入他人设备弹窗
  const [connectOpen, setConnectOpen] = useState(false);
  const [connCode, setConnCode] = useState("");
  const [connPwd, setConnPwd] = useState("");
  const [connecting, setConnecting] = useState(false);

  const doConnect = () => {
    if (!connCode.trim() || !connPwd.trim()) {
      message.warning("请输入连接码和密码");
      return;
    }
    setConnecting(true);
    connectShare(connCode.trim(), connPwd.trim())
      .then((res) => {
        if (res.code === 0) {
          message.success("接入成功，已加入设备列表");
          setConnectOpen(false);
          setConnCode("");
          setConnPwd("");
          loadDevices();
        } else {
          message.error(res.msg ?? "接入失败");
        }
      })
      .catch(() => message.error("接入失败，请检查网络"))
      .finally(() => setConnecting(false));
  };

  // 客户端窗口内标出「本机」
  const [localId, setLocalId] = useState<string | null>(null);
  useEffect(() => {
    localMachineId().then(setLocalId);
  }, []);

  useEffect(() => {
    if (open && tauriInvoke) {
      tauriInvoke("autostart_get")
        .then((v) => setAutostart(Boolean(v)))
        .catch(() => setAutostart(null));
      loadTerminals();
      loadClientVersion();
      loadPluginVersion();
    }
    // tauriInvoke 是宿主环境常量，不会在会话中途变化
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const toggleAutostart = (next: boolean) => {
    tauriInvoke?.("autostart_set", { enable: next })
      .then((r) => {
        // 以系统真实状态为准：设置可能因权限等原因未生效
        const actual = Boolean(r);
        setAutostart(actual);
        if (actual !== next) {
          message.warning("设置未生效，请稍后重试或使用托盘菜单");
        }
      })
      .catch(() => message.error("设置失败"));
  };

  useEffect(() => {
    if (open) {
      const cfg = loadGuardConfig();
      setGuardEnabled(cfg.enabled);
      setGuardPatterns(cfg.customPatterns.join("\n"));
    }
  }, [open]);

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


  useEffect(() => {
    if (open) {
      loadDevices();
    }
  }, [open, loadDevices]);


  const user = (getUserInfo() as {
    nickname?: string;
    username?: string;
    isSuper?: boolean;
  }) ?? {};

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  // 移动端两级导航：menu=一级菜单列表，content=二级分区内容（带返回）
  const [mobileView, setMobileView] = useState<"menu" | "content">("menu");
  useEffect(() => {
    if (open) {
      setMobileView("menu");
    }
  }, [open]);

  const navItems: {
    key: Tab;
    label: string;
    icon: React.ReactNode;
    color: string;
    badge?: number;
  }[] = [
    { key: "account", label: "账户", icon: <UserOutlined />, color: "var(--primary)" },
    { key: "devices", label: "设备管理", icon: <LaptopOutlined />, color: "#3a8cff", badge: pendingCount },
    { key: "configs", label: "配置同步", icon: <CloudSyncOutlined />, color: "var(--primary)" },
    { key: "bots", label: "机器人管理", icon: <RobotOutlined />, color: "#21b34a" },
    { key: "security", label: "安全防护", icon: <SafetyOutlined />, color: "#f2933c" },
    { key: "about", label: "关于", icon: <InfoCircleOutlined />, color: "#8a94a6" },
  ];

  const renderDevice = (d: PortalDevice) => (
    <div key={d.id} className={styles.device}>
      <div className={styles.devInfo}>
        <div className={styles.devName}>
          <Badge status={d.online ? "success" : "default"} />
          <span>{d.hostname}</span>
          <Tag color={PLATFORM_COLOR[d.platform ?? ""] || "default"}>{d.platformDsr}</Tag>
          {d.id === localId ? <Tag color="purple">本机</Tag> : null}
          {d.shared ? (
            <Tag color="cyan">协助接入{d.owner ? ` · ${d.owner}` : ""}</Tag>
          ) : d.trusted ? (
            <Tag color="green">已信任</Tag>
          ) : (
            <Tag color="warning">已断开</Tag>
          )}
        </div>
        <div className={styles.devMeta}>
          {d.online ? "在线" : "离线"} · {d.sessionCount} 个会话 · v{d.version}
        </div>
      </div>
      <div className={styles.devActions}>
        {d.shared ? (
          <Button
            size="small"
            className={styles.actBtn}
            onClick={() => {
              disconnectShare(d.id).then((res) => {
                if (res.code === 0) {
                  message.success("已断开");
                  loadDevices();
                }
              });
            }}
          >
            断开接入
          </Button>
        ) : (
          <>
            {d.trusted ? (
              <>
                <Button
                  size="small"
                  className={styles.shareBtn}
                  onClick={() => setShareDevice(d)}
                >
                  协助共享
                </Button>
                {!d.isHub && (
                  <Button
                    size="small"
                    className={styles.actBtn}
                    onClick={() => untrustDevice(d.id)}
                  >
                    撤销信任
                  </Button>
                )}
              </>
            ) : (
              <Button
                size="small"
                type="primary"
                className={styles.trustBtn}
                onClick={() => trustDevice(d.id)}
              >
                信任
              </Button>
            )}
            {!d.isHub && (
              <Popconfirm
                title="删除该设备记录？"
                okText="删除"
                cancelText="取消"
                onConfirm={() => deleteDevice(d.id)}
              >
                <Button size="small" className={styles.delBtn} danger type="text">
                  删除
                </Button>
              </Popconfirm>
            )}
          </>
        )}
      </div>
    </div>
  );

  const pending = devices.filter((d) => !d.trusted);
  const trusted = devices.filter((d) => d.trusted);

  return (
    <Modal
      className={styles.SettingsModal}
      open={open}
      onCancel={onClose}
      footer={null}
      width={920}
      title={null}
      closable={false}
      centered
    >
      <div
        className={`${styles.layout} ${
          mobileView === "content" ? styles.mobileContent : styles.mobileMenu
        }`}
      >
        {/* 移动端二级头部：返回 + 标题（一级/桌面隐藏，见 scss） */}
        <div className={styles.mobileHead}>
          {mobileView === "content" ? (
            <span
              className={styles.mobileBack}
              role="button"
              tabIndex={0}
              onClick={() => setMobileView("menu")}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setMobileView("menu");
                }
              }}
            >
              <LeftOutlined className={styles.mobileBackIcon} />
              设置
            </span>
          ) : (
            <span className={styles.mobileHeadTitle}>设置</span>
          )}
          {mobileView === "content" ? (
            <span className={styles.mobileHeadTitle}>
              {navItems.find((n) => n.key === tab)?.label}
            </span>
          ) : null}
        </div>
        <span
          className={styles.closeBtn}
          role="button"
          tabIndex={0}
          aria-label="关闭设置"
          data-sheet-close
          onClick={onClose}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onClose?.();
            }
          }}
        >
          <CloseOutlined />
        </span>
        <aside className={styles.nav}>
          <div className={styles.navTitle}>设置</div>
          {/* Claude sheet 顶部的身份 pill（仅移动端，见 scss）：点按进账户 */}
          <div
            className={styles.identityPill}
            role="button"
            tabIndex={0}
            onClick={() => {
              setTab("account");
              setMobileView("content");
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setTab("account");
                setMobileView("content");
              }
            }}
          >
            <span className={styles.identityAvatar}>
              {(user.nickname ?? user.username ?? "U").slice(0, 1)}
            </span>
            <span className={styles.identityName}>
              {user.nickname ?? user.username}
            </span>
            <RightOutlined className={styles.identityArrow} />
          </div>
          <div className={styles.navList} role="tablist" aria-label="设置分类">
            {navItems.map((n) => (
              <div
                key={n.key}
                className={`${styles.navItem} ${tab === n.key ? styles.active : ""}`}
                role="tab"
                tabIndex={0}
                aria-selected={tab === n.key}
                onClick={() => {
                  setTab(n.key);
                  setMobileView("content");
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setTab(n.key);
                    setMobileView("content");
                  }
                }}
              >
                <span
                  className={styles.navIcon}
                  style={{ ["--nav-color" as string]: n.color }}
                >
                  {n.icon}
                </span>
                <span className={styles.navLabel}>{n.label}</span>
                {n.badge ? <Badge count={n.badge} size="small" /> : null}
                <RightOutlined className={styles.navChevron} />
              </div>
            ))}
          </div>
        </aside>

        <section className={styles.content}>
          {tab === "account" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>账户</div>
              <div className={styles.account}>
                <div className={styles.avatar}>
                  {(user.nickname ?? user.username ?? "U").slice(0, 1)}
                </div>
                <div>
                  <div className={styles.accName}>{user.nickname ?? user.username}</div>
                  <div className={styles.accMeta}>
                    用户名 {user.username}
                    {user.isSuper ? " · 超级管理员" : ""}
                  </div>
                </div>
              </div>
              <Button icon={<LogoutOutlined />} danger onClick={onLogout} style={{ marginTop: 20 }}>
                退出登录
              </Button>
            </div>
          )}

          {tab === "devices" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>设备管理</div>
              <div className={styles.hint}>
                新设备接入<strong>默认信任</strong>：在其它电脑安装客户端并登录你的账号，
                它会自动出现在这里并开始同步。信任开关就是同步链接的开关——
                撤销信任即断开该设备的同步（不会被自动恢复），随时可手动重新信任。
                需要协助别人时，在自己设备上点「协助共享」生成连接码给对方。
              </div>
              {autostart !== null && (
                <div className={styles.section}>
                  <div className={styles.sectionTitle}>本机客户端</div>
                  <div className={styles.device}>
                    <div className={styles.devInfo}>
                      <div className={styles.devName}>开机自启</div>
                      <div className={styles.devMeta}>
                        随系统启动，在后台持续同步本机终端会话
                      </div>
                    </div>
                    <Switch checked={autostart} onChange={toggleAutostart} />
                  </div>
                  <div className={styles.device}>
                    <div className={styles.devInfo}>
                      <div className={styles.devName}>
                        客户端版本
                        {/* 版本标签固定显示当前版本，更新中也不切成「正在下载/安装」——
                            更新进度由下方的进度条单独呈现，标签只作版本标识 */}
                        {clientVer ? (
                          <Tag color={clientVer.latest ? "warning" : "green"}>
                            v{clientVer.current}
                          </Tag>
                        ) : null}
                      </div>
                      {updating && clientVer?.progress ? (
                        <div className={styles.updateProgress}>
                          {clientVer.progress.phase === "downloading" &&
                          clientVer.progress.total > 0 ? (
                            <>
                              <Progress
                                percent={Math.min(
                                  100,
                                  Math.round(
                                    (clientVer.progress.received / clientVer.progress.total) * 100,
                                  ),
                                )}
                                size="small"
                                strokeColor="var(--primary)"
                              />
                              <span className={styles.updateProgressText}>
                                {(clientVer.progress.received / 1024 / 1024).toFixed(1)} /{" "}
                                {(clientVer.progress.total / 1024 / 1024).toFixed(1)} MB
                              </span>
                            </>
                          ) : (
                            <span className={styles.updateProgressText}>
                              完成后客户端将自动重启，请稍候
                            </span>
                          )}
                        </div>
                      ) : (
                        <div className={styles.devMeta}>
                          更新会自动下载安装并重启客户端
                        </div>
                      )}
                    </div>
                    {clientVer?.latest && !updating ? (
                      // 有新版本且尚未在更新：给「更新到 vX」按钮。更新中则落到下面显示
                      // 「更新中」+进度（updating 已排除了「已是最新却有遗留进度」的误判）。
                      <Button
                        size="small"
                        className={styles.updateNowBtn}
                        onClick={() =>
                          tauriInvoke
                            ?.("update_start")
                            .catch(() => message.error("启动更新失败"))
                        }
                      >
                        更新到 v{clientVer.latest}
                      </Button>
                    ) : (
                      <Button
                        size="small"
                        className={styles.checkUpdateBtn}
                        loading={checkingUpdate || updating}
                        disabled={updating}
                        onClick={checkUpdate}
                      >
                        {updating ? "更新中" : "检查更新"}
                      </Button>
                    )}
                  </div>
                  {/* 插件版本（Cursor/VSCode 桥接扩展）：旧客户端无 plugin_status IPC → pluginVer 为 null，整行不渲染 */}
                  {pluginVer ? (
                    <div className={styles.device}>
                      <div className={styles.devInfo}>
                        <div className={styles.devName}>
                          插件版本
                          <Tag
                            color={
                              pluginVer.installed === pluginVer.latest
                                ? "green"
                                : pluginVer.installed
                                  ? "warning"
                                  : "default"
                            }
                          >
                            {pluginVer.installed ? `v${pluginVer.installed}` : "未安装"}
                          </Tag>
                        </div>
                        <div className={styles.devMeta}>
                          Cursor/VSCode 桥接扩展，内嵌终端下发靠它
                        </div>
                      </div>
                      {pluginVer.installed !== pluginVer.latest ? (
                        <Button
                          size="small"
                          className={styles.updateNowBtn}
                          loading={checkingPlugin}
                          onClick={updatePlugin}
                        >
                          {pluginVer.installed ? `更新到 v${pluginVer.latest}` : "安装插件"}
                        </Button>
                      ) : (
                        <Button
                          size="small"
                          className={styles.checkUpdateBtn}
                          loading={checkingPlugin}
                          onClick={checkPluginUpdate}
                        >
                          检查更新
                        </Button>
                      )}
                    </div>
                  ) : null}
                  <div className={styles.termScope}>
                    <div className={styles.sectionTitle}>监控范围</div>
                    {terminals.length === 0 ? (
                      <div className={styles.termScopeEmpty}>
                        暂未检测到本机终端会话
                      </div>
                    ) : (
                      terminals.map((tm) => (
                        <div key={tm.key} className={styles.termScopeRow}>
                          <span className={styles.termScopeName} title={tm.key}>
                            {tm.name}
                          </span>
                          <Switch
                            checked={!tm.excluded}
                            onChange={(on) => toggleTerminal(tm.key, !on)}
                          />
                        </div>
                      ))
                    )}
                    <div className={styles.termScopeHint}>
                      关闭开关 = 不监控该终端（对应会话不再上报）
                    </div>
                  </div>
                </div>
              )}
              {pending.length > 0 && (
                <div className={styles.section}>
                  <div className={styles.sectionTitle}>未信任 · 已断开（{pending.length}）</div>
                  {pending.map(renderDevice)}
                </div>
              )}
              <div className={styles.section}>
                <div className={styles.sectionTitle}>已信任（{trusted.length}）</div>
                {trusted.length ? (
                  trusted.map(renderDevice)
                ) : (
                  <Empty
                    image={Empty.PRESENTED_IMAGE_SIMPLE}
                    description="暂无已信任设备"
                  />
                )}
              </div>

              {/* 接入他人电脑：独立入口（输入对方协助码），与「管理自己的设备」区分开 */}
              <div
                className={styles.connectEntry}
                role="button"
                tabIndex={0}
                onClick={() => setConnectOpen(true)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setConnectOpen(true);
                  }
                }}
              >
                <span className={styles.connectEntryIcon}>
                  <LinkOutlined />
                </span>
                <div className={styles.connectEntryText}>
                  <div className={styles.connectEntryTitle}>接入他人电脑</div>
                  <div className={styles.connectEntryDesc}>
                    输入对方的协助码，远程查看、控制其终端会话
                  </div>
                </div>
                <RightOutlined className={styles.connectEntryArrow} />
              </div>
            </div>
          )}

          {tab === "configs" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>配置同步</div>
              <div className={styles.hint}>
                让多台电脑共用同一套 Claude Code / Codex 配置：选一台设备作为
                <strong>配置源</strong>，其余设备自动向它看齐。改动几十秒内送达，
                设备离线时等它上线继续。
              </div>
              <ConfigSyncPanel />
            </div>
          )}

          {tab === "bots" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>机器人管理</div>
              <div className={styles.hint}>
                配置你自己的钉钉机器人：一个账号一个，它收到的消息就归你、推送也只发给你。
                配好后在钉钉里发指令就能遥控会话，任务完成/需要你决定时也会私聊提醒。
              </div>
              <IntegrationsPanel />
            </div>
          )}

          {tab === "security" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>安全防护</div>
              <div className={styles.hint}>
                发布到终端会话的内容命中危险模式（如 Claude Code 的
                <code>bypass permissions</code>、<code>rm -rf</code> 等）时，
                需要经过<strong>两步确认</strong>（风险告知 + 输入确认词）才会真正发布。
              </div>
              <div className={styles.guardRow}>
                <span className={styles.guardLabel}>启用危险输入防护</span>
                <Switch
                  checked={guardEnabled}
                  onChange={(checked) => {
                    setGuardEnabled(!!checked);
                    // 开关只保存启用位，textarea 未保存的草稿不随开关落盘
                    saveGuard(
                      !!checked,
                      loadGuardConfig().customPatterns.join("\n")
                    );
                  }}
                />
              </div>
              <div className={styles.section}>
                <div className={styles.sectionTitle}>内置危险模式</div>
                <div className={styles.builtinPatterns}>
                  {BUILTIN_DANGER_PATTERNS.map((p) => (
                    <Tag key={p.pattern} className={styles.patternTag}>
                      {p.pattern}
                    </Tag>
                  ))}
                </div>
              </div>
              <div className={styles.section}>
                <div className={styles.sectionTitle}>自定义危险模式（一行一个，子串匹配）</div>
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
            </div>
          )}

          {tab === "about" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>关于</div>
              <div className={styles.about}>
                <div className={styles.aboutLogo}><CodeOutlined /></div>
                <div className={styles.aboutName}>终端任务监控</div>
                <div className={styles.aboutDesc}>
                  监控多台电脑终端里 AI 编码代理（Claude Code、Codex 等）正在执行的任务，
                  支持实时查看、控制与发布任务。会话内容纯实时读取、不落存储。
                </div>
              </div>
            </div>
          )}
        </section>
      </div>

      <ShareModal device={shareDevice} onClose={() => setShareDevice(null)} />

      <Modal
        title="接入他人设备"
        open={connectOpen}
        onCancel={() => setConnectOpen(false)}
        onOk={doConnect}
        okText="接入"
        cancelText="取消"
        confirmLoading={connecting}
        width={400}
        centered
      >
        <div style={{ fontSize: 12, color: "#7c9096", marginBottom: 12 }}>
          输入对方在「协助共享」里生成的连接码和密码，接入后即可查看、控制对方设备的终端会话。
        </div>
        <Input
          placeholder="连接码"
          value={connCode}
          onChange={(v) => setConnCode(v)}
          style={{ marginBottom: 10 }}
        />
        <Input
          placeholder="密码"
          value={connPwd}
          onChange={(v) => setConnPwd(v)}
        />
      </Modal>
    </Modal>
  );
});

export default SettingsModal;
