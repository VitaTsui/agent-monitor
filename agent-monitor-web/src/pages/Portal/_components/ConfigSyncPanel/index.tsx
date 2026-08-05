import React, { useCallback, useEffect, useState } from "react";

// message / Empty / Tag / Badge / Popconfirm 属 hsu-ui 未覆盖的能力，按约定用 antd 兜底
import { Badge, Empty, Popconfirm, Spin, Tag, message } from "antd";
import { CloudSyncOutlined } from "@ant-design/icons";

import { Button } from "@hsu-react/ui";

import {
  ConfigSyncDevice,
  ConfigSyncInfo,
  getConfigSync,
  setConfigSource,
} from "@/services/apis/portal";
import { localMachineId } from "@/utils/clientAuth";
import styles from "./index.module.scss";

/**
 * 配置同步（Claude Code / Codex）。
 *
 * 模型是**单向镜像**：选一台设备当「配置源」，其余设备向它看齐。之所以不做双向合并——
 * 两台机器同时改同一个 CLAUDE.md 时，任何自动合并都会在用户毫不知情的情况下丢掉一边的内容。
 *
 * 同步的只有 md 类配置（CLAUDE.md、agents/、commands/、skills/、Codex 的 AGENTS.md 与
 * prompts/）。凭据与 settings.json 一律不同步，原因见下面的说明文案。
 */
const ConfigSyncPanel: React.FC = () => {
  const [info, setInfo] = useState<ConfigSyncInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState("");
  // 客户端窗口内标出「本机」（浏览器里取不到，为 null）
  const [localId, setLocalId] = useState<string | null>(null);
  useEffect(() => {
    localMachineId().then(setLocalId);
  }, []);

  const load = useCallback(async (silent?: boolean) => {
    if (!silent) {
      setLoading(true);
    }
    const res = await getConfigSync();
    if (res.code === 0 && res.data) {
      setInfo(res.data);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    load();
    // 同步是后台推进的（客户端每轮心跳推一点），停留在本页时定期刷新进度，
    // 否则用户会看着一个不动的「还差 N 份」以为卡住了
    const timer = setInterval(() => load(true), 4000);
    return () => clearInterval(timer);
  }, [load]);

  const choose = async (machineId: string) => {
    setSaving(machineId || "off");
    const res = await setConfigSource(machineId);
    setSaving("");
    if (res.code === 0) {
      message.success(res.data?.result ?? "已保存");
      load(true);
    }
  };

  const renderDevice = (d: ConfigSyncDevice) => {
    const isLocal = d.machineId === localId;
    return (
      <div key={d.machineId} className={styles.device}>
        <div className={styles.devInfo}>
          <div className={styles.devName}>
            <Badge status={d.online ? "success" : "default"} />
            <span>{d.hostname || d.machineId}</span>
            <Tag>{d.platform}</Tag>
            {isLocal ? <Tag color="purple">本机</Tag> : null}
            {d.isSource ? <Tag color="green">配置源</Tag> : null}
          </div>
          <div className={styles.devMeta}>{describe(d, info?.enabled)}</div>
        </div>
        <div className={styles.devActions}>
          {d.isSource ? (
            <Popconfirm
              title="关闭配置同步？"
              description="其余设备将停止接收更新，已同步的文件保留。"
              okText="关闭"
              cancelText="取消"
              onConfirm={() => choose("")}
            >
              <Button size="small" className={styles.actBtn} loading={saving === "off"}>
                关闭同步
              </Button>
            </Popconfirm>
          ) : (
            <Popconfirm
              title="设为配置源？"
              description="该设备的配置会成为基准，其余设备将被覆盖为与它一致。"
              okText="设为配置源"
              cancelText="取消"
              onConfirm={() => choose(d.machineId)}
            >
              <Button
                size="small"
                type="primary"
                className={styles.sourceBtn}
                disabled={!d.trusted}
                loading={saving === d.machineId}
              >
                设为配置源
              </Button>
            </Popconfirm>
          )}
        </div>
      </div>
    );
  };

  if (loading) {
    return (
      <div className={styles.ConfigSyncPanel}>
        <div className={styles.loading}>
          <Spin />
        </div>
      </div>
    );
  }

  const devices = info?.devices ?? [];

  return (
    <div className={styles.ConfigSyncPanel}>
      <div className={styles.summary}>
        <div className={styles.sumIcon}>
          <CloudSyncOutlined />
        </div>
        <div className={styles.sumText}>
          <div className={styles.sumTitle}>
            {info?.enabled ? "配置同步已开启" : "配置同步未开启"}
          </div>
          <div className={styles.sumMeta}>
            {info?.enabled
              ? `基准配置 ${info.baselineCount} 份，其余设备自动向配置源看齐`
              : "选一台设备作为「配置源」，其余设备会自动与它保持一致"}
          </div>
        </div>
      </div>

      {devices.length === 0 ? (
        <Empty description="还没有设备" image={Empty.PRESENTED_IMAGE_SIMPLE} />
      ) : (
        devices.map(renderDevice)
      )}

      <div className={styles.note}>
        <div className={styles.noteTitle}>同步范围</div>
        <div className={styles.noteBody}>
          <strong>整份同步</strong>：<code>CLAUDE.md</code>、<code>agents/</code>、
          <code>commands/</code>、<code>skills/</code>，以及 Codex 的 <code>AGENTS.md</code> 与
          <code>prompts/</code>。
          <br />
          <strong>按字段同步</strong>：<code>settings.json</code> 只同步 <code>model</code>，
          合并进本机文件 —— 你自己写的其它字段一律原样保留，<code>hooks</code>、
          <code>apiKeyHelper</code>、<code>statusLine</code>、<code>permissions</code>
          等含本机路径的字段<strong>永不同步</strong>（覆盖过去会让另一台机器的会话配对失效）。
          即使是可同步字段，值里含绝对路径时也会自动跳过。
          <br />
          <strong>登录凭据不会同步</strong>，它们不会离开你本机。
          <br />
          被改动的文件会在原地留一份 <code>.am-bak</code> 备份。源机删除的文件<strong>不会</strong>
          在其它设备上被删除。
        </div>
      </div>
    </div>
  );
};

/** 单台设备的状态描述。源机与镜像机的「差多少」含义相反，措辞要分开。 */
function describe(d: ConfigSyncDevice, enabled?: boolean): string {
  if (!d.trusted) {
    return "已断开同步（需先在设备管理里信任）";
  }
  if (!d.supported) {
    return d.online ? "客户端版本较旧或正在首次扫描，暂不支持配置同步" : "离线";
  }
  if (!enabled) {
    return `本机 ${d.fileCount} 份配置`;
  }
  if (d.isSource) {
    return d.behind > 0
      ? `本机 ${d.fileCount} 份配置 · 正在上传 ${d.behind} 份`
      : `本机 ${d.fileCount} 份配置 · 已全部上传`;
  }
  if (d.behind > 0) {
    return d.online
      ? `本机 ${d.fileCount} 份配置 · 还差 ${d.behind} 份，同步中`
      : `本机 ${d.fileCount} 份配置 · 还差 ${d.behind} 份，等设备上线后继续`;
  }
  return `本机 ${d.fileCount} 份配置 · 已与配置源一致`;
}

export default ConfigSyncPanel;
