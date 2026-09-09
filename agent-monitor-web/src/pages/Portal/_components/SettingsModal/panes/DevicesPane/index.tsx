import React, { useEffect, useState } from "react";

import { Button, Input, Modal, Switch, message } from "@hsu-react/ui";
import {
  Badge,
  Empty,
  Modal as AntModal,
  Popconfirm,
  Progress,
  Tag,
} from "antd";
import { LinkOutlined, RightOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import {
  PortalDevice,
  connectShare,
  disconnectShare,
} from "@/services/apis/portal";
import { localMachineId } from "@/utils/clientAuth";
import PortalStore from "../../../../PortalStore";
import ShareModal from "../../../ShareModal";
import st from "../../settings.module.scss";
import styles from "./index.module.scss";

const PLATFORM_COLOR: Record<string, string> = {
  macos: "geekblue",
  windows: "cyan",
  linux: "orange",
};

interface UpdateProgress {
  phase: "downloading" | "installing" | "restarting";
  received: number;
  total: number;
}

/**
 * 设备管理分栏。
 *
 * 三块内容：本机客户端（开机自启 / 客户端与插件版本 / 监控范围，仅客户端窗口内）、
 * 设备列表（信任 / 撤销 / 删除 / 协助共享）、接入他人电脑。
 */
const DevicesPane: React.FC = observer(() => {
  const { devices, loadDevices, trustDevice, untrustDevice, deleteDevice } =
    PortalStore;

  // 桌面客户端 IPC 桥：仅在客户端窗口内存在（浏览器里为 undefined，相关 UI 不渲染）
  const tauriInvoke = (
    window as unknown as {
      __TAURI__?: {
        core?: {
          invoke?: (
            cmd: string,
            args?: Record<string, unknown>,
          ) => Promise<unknown>;
        };
      };
    }
  ).__TAURI__?.core?.invoke;

  // null = 尚未取到（或不在客户端内）
  const [autostart, setAutostart] = useState<boolean | null>(null);
  // 本机监控范围（终端列表 + 排除态；仅客户端窗口内）
  const [terminals, setTerminals] = useState<
    { key: string; name: string; excluded: boolean }[]
  >([]);
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
  // 协助共享：主人管理弹窗的目标设备
  const [shareDevice, setShareDevice] = useState<PortalDevice | null>(null);
  // 接入他人设备弹窗
  const [connectOpen, setConnectOpen] = useState(false);
  const [connCode, setConnCode] = useState("");
  const [connPwd, setConnPwd] = useState("");
  const [connecting, setConnecting] = useState(false);
  // 客户端窗口内标出「本机」
  const [localId, setLocalId] = useState<string | null>(null);

  const loadTerminals = () => {
    tauriInvoke?.("terminals_get")
      .then((v) => setTerminals((v as typeof terminals) ?? []))
      .catch(() => setTerminals([]));
  };

  const loadClientVersion = () => {
    tauriInvoke?.("update_status")
      .then((v) =>
        setClientVer(
          v as {
            current: string;
            latest: string | null;
            progress?: UpdateProgress | null;
          },
        ),
      )
      .catch(() => setClientVer(null));
  };

  const loadPluginVersion = () => {
    tauriInvoke?.("plugin_status")
      .then((v) =>
        setPluginVer(v as { installed: string | null; latest: string }),
      )
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
          message.success(
            `已安装桥接插件 v${r.version ?? ""} 到 ${r.installed} 个编辑器，重载编辑器窗口即生效`,
          );
        } else {
          message.warning(
            "未检测到 Cursor/VSCode 命令行（在编辑器里执行「Shell Command: Install 'code'/'cursor' command in PATH」后重试）",
          );
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
              tauriInvoke?.("update_start").catch(() =>
                message.error("启动更新失败"),
              );
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

  useEffect(() => {
    localMachineId().then(setLocalId);
  }, []);

  useEffect(() => {
    loadDevices();
  }, [loadDevices]);

  useEffect(() => {
    if (!tauriInvoke) {
      return;
    }
    tauriInvoke("autostart_get")
      .then((v) => setAutostart(Boolean(v)))
      .catch(() => setAutostart(null));
    loadTerminals();
    loadClientVersion();
    loadPluginVersion();
    // tauriInvoke 是宿主环境常量，不会在会话中途变化
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 停留在本分栏时轮询版本/进度（更新中每 1.5s 刷新进度条）。
  // 挂在「本分栏是否挂载」上而不是弹窗的 open 上：切走分栏即卸载，
  // 定时器自然停掉，不会在别的分栏里空转。
  useEffect(() => {
    if (!tauriInvoke) {
      return;
    }
    const timer = window.setInterval(loadClientVersion, 1500);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const renderDevice = (d: PortalDevice) => (
    <div key={d.id} className={st.row}>
      <div className={st.rowInfo}>
        <div className={st.rowTitle}>
          <Badge status={d.online ? "success" : "default"} />
          <span>{d.hostname}</span>
          <Tag color={PLATFORM_COLOR[d.platform ?? ""] || "default"}>
            {d.platformDsr}
          </Tag>
          {d.id === localId ? <Tag color="purple">本机</Tag> : null}
          {d.shared ? (
            <Tag color="cyan">协助接入{d.owner ? ` · ${d.owner}` : ""}</Tag>
          ) : d.trusted ? (
            <Tag color="green">已信任</Tag>
          ) : (
            <Tag color="warning">已断开</Tag>
          )}
        </div>
        <div className={st.rowDesc}>
          {d.online ? "在线" : "离线"} · {d.sessionCount} 个会话 · v{d.version}
        </div>
      </div>
      <div className={st.rowActions}>
        {d.shared ? (
          <Button
            size="small"
            className={st.actBtn}
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
                  className={st.shareBtn}
                  onClick={() => setShareDevice(d)}
                >
                  协助共享
                </Button>
                {!d.isHub && (
                  <Button
                    size="small"
                    className={st.actBtn}
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
                className={st.trustBtn}
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
                <Button size="small" className={st.delBtn} danger type="text">
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
    <>
      <div className={st.paneTitle}>设备管理</div>
      <div className={st.hint}>
        新设备接入<strong>默认信任</strong>
        ：在其它电脑安装客户端并登录你的账号，
        它会自动出现在这里并开始同步。信任开关就是同步链接的开关——
        撤销信任即断开该设备的同步（不会被自动恢复），随时可手动重新信任。
        需要协助别人时，在自己设备上点「协助共享」生成连接码给对方。
      </div>

      {autostart !== null && (
        <div className={st.section}>
          <div className={st.sectionTitle}>本机客户端</div>
          <div className={st.row}>
            <div className={st.rowInfo}>
              <div className={st.rowTitle}>开机自启</div>
              <div className={st.rowDesc}>
                随系统启动，在后台持续同步本机终端会话
              </div>
            </div>
            <Switch checked={autostart} onChange={toggleAutostart} />
          </div>
          <div className={st.row}>
            <div className={st.rowInfo}>
              <div className={st.rowTitle}>
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
                            (clientVer.progress.received /
                              clientVer.progress.total) *
                              100,
                          ),
                        )}
                        size="small"
                        strokeColor="var(--primary)"
                      />
                      <span className={styles.updateProgressText}>
                        {(clientVer.progress.received / 1024 / 1024).toFixed(1)}{" "}
                        / {(clientVer.progress.total / 1024 / 1024).toFixed(1)}{" "}
                        MB
                      </span>
                    </>
                  ) : (
                    <span className={styles.updateProgressText}>
                      完成后客户端将自动重启，请稍候
                    </span>
                  )}
                </div>
              ) : (
                <div className={st.rowDesc}>更新会自动下载安装并重启客户端</div>
              )}
            </div>
            {clientVer?.latest && !updating ? (
              // 有新版本且尚未在更新：给「更新到 vX」按钮。更新中则落到下面显示
              // 「更新中」+进度（updating 已排除了「已是最新却有遗留进度」的误判）。
              <Button
                size="small"
                className={styles.updateNowBtn}
                onClick={() =>
                  tauriInvoke?.("update_start").catch(() =>
                    message.error("启动更新失败"),
                  )
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
            <div className={st.row}>
              <div className={st.rowInfo}>
                <div className={st.rowTitle}>
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
                <div className={st.rowDesc}>
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
                  {pluginVer.installed
                    ? `更新到 v${pluginVer.latest}`
                    : "安装插件"}
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
            <div className={st.sectionTitle}>监控范围</div>
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
        <div className={st.section}>
          <div className={st.sectionTitle}>
            未信任 · 已断开（{pending.length}）
          </div>
          {pending.map(renderDevice)}
        </div>
      )}
      <div className={st.section}>
        <div className={st.sectionTitle}>已信任（{trusted.length}）</div>
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
        <div className={styles.connectTip}>
          输入对方在「协助共享」里生成的连接码和密码，接入后即可查看、控制对方设备的终端会话。
        </div>
        <Input
          placeholder="连接码"
          value={connCode}
          onChange={(v) => setConnCode(v)}
          className={styles.connectInput}
        />
        <Input
          placeholder="密码"
          value={connPwd}
          onChange={(v) => setConnPwd(v)}
        />
      </Modal>
    </>
  );
});

export default DevicesPane;
