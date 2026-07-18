import React, { useEffect, useState } from "react";

import { Button, Input, Modal, Switch } from "@hsu-react/ui";
// Progress：hsu-ui 无对应组件，按规范用 antd 兜底
import { Badge, Empty, Popconfirm, Progress, Tag, message } from "antd";
import {
  CloseOutlined,
  CodeOutlined,
  DashboardOutlined,
  InfoCircleOutlined,
  LaptopOutlined,
  LogoutOutlined,
  SafetyOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import {
  PortalDevice,
  getPortalQuota,
  setPortalQuota,
} from "@/services/apis/portal";
import { getUserInfo, removeToken } from "@/utils/auth";
import { localMachineId } from "@/utils/clientAuth";
import PortalStore from "../../PortalStore";
import {
  BUILTIN_DANGER_PATTERNS,
  loadGuardConfig,
  saveGuardConfig,
} from "../../_utils/dangerCheck";
import styles from "./index.module.scss";

export type SettingsTab = "account" | "devices" | "quota" | "security" | "about";

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
  const [quotaLimit, setQuotaLimit] = useState<number>(0);
  const [quotaUsed, setQuotaUsed] = useState<number>(0);
  const [savingQuota, setSavingQuota] = useState(false);
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
      getPortalQuota()
        .then((res) => {
          if (res.code === 0) {
            setQuotaLimit(res.data?.limit ?? 0);
            setQuotaUsed(res.data?.used ?? 0);
          }
        })
        .catch(() => void 0);
    }
  }, [open, loadDevices]);

  const saveQuota = () => {
    setSavingQuota(true);
    setPortalQuota(quotaLimit)
      .then((res) => {
        if (res.code === 0) message.success("已保存额度上限");
        else message.error(res.msg ?? "保存失败");
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setSavingQuota(false));
  };

  const user = (getUserInfo() as {
    nickname?: string;
    username?: string;
    isSuper?: boolean;
  }) ?? {};

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  const navItems: { key: Tab; label: string; icon: React.ReactNode; badge?: number }[] = [
    { key: "account", label: "账户", icon: <UserOutlined /> },
    { key: "devices", label: "设备管理", icon: <LaptopOutlined />, badge: pendingCount },
    { key: "quota", label: "额度限制", icon: <DashboardOutlined /> },
    { key: "security", label: "安全防护", icon: <SafetyOutlined /> },
    { key: "about", label: "关于", icon: <InfoCircleOutlined /> },
  ];

  const renderDevice = (d: PortalDevice) => (
    <div key={d.id} className={styles.device}>
      <div className={styles.devInfo}>
        <div className={styles.devName}>
          <Badge status={d.online ? "success" : "default"} />
          <span>{d.hostname}</span>
          <Tag color={PLATFORM_COLOR[d.platform ?? ""] || "default"}>{d.platformDsr}</Tag>
          {d.id === localId ? <Tag color="purple">本机</Tag> : null}
          {d.trusted ? <Tag color="green">已信任</Tag> : <Tag color="warning">已断开</Tag>}
        </div>
        <div className={styles.devMeta}>
          {d.online ? "在线" : "离线"} · {d.sessionCount} 个会话 · v{d.version}
        </div>
      </div>
      <div className={styles.devActions}>
        {d.trusted
          ? !d.isHub && (
              <Button
                size="small"
                className={styles.untrustBtn}
                onClick={() => untrustDevice(d.id)}
              >
                撤销信任
              </Button>
            )
          : (
              <Button size="small" type="primary" onClick={() => trustDevice(d.id)}>
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
            <Button size="small" danger type="text">
              删除
            </Button>
          </Popconfirm>
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
    >
      <div className={styles.layout}>
        <span
          className={styles.closeBtn}
          role="button"
          tabIndex={0}
          aria-label="关闭设置"
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
          <div className={styles.navList} role="tablist" aria-label="设置分类">
            {navItems.map((n) => (
              <div
                key={n.key}
                className={`${styles.navItem} ${tab === n.key ? styles.active : ""}`}
                role="tab"
                tabIndex={0}
                aria-selected={tab === n.key}
                onClick={() => setTab(n.key)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setTab(n.key);
                  }
                }}
              >
                {n.icon}
                <span>{n.label}</span>
                {n.badge ? <Badge count={n.badge} size="small" /> : null}
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
              <Button icon={<LogoutOutlined />} danger onClick={onLogout}>
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
            </div>
          )}

          {tab === "quota" && (
            <div className={styles.pane}>
              <div className={styles.paneTitle}>额度限制</div>
              <div className={styles.hint}>
                这是<strong>你自己的账号额度</strong>，只作用于你名下的设备。统计各会话近{" "}
                <strong>5 小时</strong>滚动窗口的 token 用量（input + output +
                缓存创建）。设置上限后，某会话用量达到上限会<strong>自动暂停</strong>
                该终端任务；随着旧用量移出 5 小时窗口、用量回落到上限以下，会
                <strong>自动恢复</strong>。设为 0 表示不限制。
              </div>
              <div className={styles.quotaRow}>
                <span className={styles.quotaLabel}>5 小时 token 上限</span>
                <Input.Number
                  min={0}
                  step={100000}
                  value={String(quotaLimit)}
                  onChange={(v) => setQuotaLimit(Number(v || 0))}
                  wrapperClassName={styles.quotaInput}
                />
                <Button type="primary" loading={savingQuota} onClick={saveQuota}>
                  保存
                </Button>
              </div>
              {quotaLimit > 0 ? (
                <>
                  <Progress
                    className={styles.quotaProgress}
                    percent={Math.min(
                      100,
                      Math.round((quotaUsed / quotaLimit) * 100)
                    )}
                    // 80% 起警示、满额红色，与「达到上限自动暂停」的语义对齐
                    strokeColor={
                      quotaUsed >= quotaLimit
                        ? "#f56c6c"
                        : quotaUsed / quotaLimit >= 0.8
                          ? "#f2b234"
                          : "#0f9bad"
                    }
                  />
                  <div className={styles.quotaUsed}>
                    已用 {Math.min(100, Math.round((quotaUsed / quotaLimit) * 100))}%
                    （最高会话 {quotaUsed.toLocaleString()} / 上限{" "}
                    {quotaLimit.toLocaleString()} tokens）
                  </div>
                </>
              ) : (
                <div className={styles.quotaUsed}>
                  未设上限 · 当前用量最高的会话 {quotaUsed.toLocaleString()} tokens
                </div>
              )}
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

    </Modal>
  );
});

export default SettingsModal;
