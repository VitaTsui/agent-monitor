import React, { useCallback, useEffect, useState } from "react";

// message / Empty / Tag / Badge / Popconfirm 属 hsu-ui 未覆盖的能力，按约定用 antd 兜底
import { Badge, Empty, Popconfirm, Spin, Tag, message } from "antd";
import { CloudSyncOutlined } from "@ant-design/icons";

import { Button, Switch } from "@hsu-react/ui";

import {
  ConfigSyncDevice,
  ConfigSyncInfo,
  getConfigSync,
  setConfigFieldSync,
  setConfigSource,
} from "@/services/apis/portal";
import { localMachineId } from "@/utils/clientAuth";
import DiffModal, { DiffTarget } from "./DiffModal";
import styles from "./index.module.scss";

/**
 * 配置同步（Claude Code / Codex）。
 *
 * 模型是**单向镜像**：选一台设备当「配置源」，其余设备向它看齐。之所以不做双向合并——
 * 两台机器同时改同一个 CLAUDE.md 时，任何自动合并都会在用户毫不知情的情况下丢掉一边的内容。
 *
 * 两条路径：md 类整份同步；settings.json / config.toml 按**字段**合并（另有独立开关）。
 * 凭据永不同步；hooks 只同步「通用」条目，配对 hook 与指向本机路径的 hook 留在原机。
 * 详见下面的说明文案与 core 的 configpath。
 */
const ConfigSyncPanel: React.FC = () => {
  const [info, setInfo] = useState<ConfigSyncInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState("");
  // 点开某一项看详情（hooks 展开到命令级）
  const [diff, setDiff] = useState<DiffTarget | null>(null);
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

  const toggleFieldSync = async (enabled: boolean) => {
    const res = await setConfigFieldSync(enabled);
    if (res.code === 0) {
      message.success(res.data?.result ?? "已保存");
      load(true);
    }
  };

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
          {/* 缺依赖被跳过的：如实说明「这台为什么和配置源不一致」，
              否则用户只会看到两台死活对不齐却不知道差在哪 */}
          {d.skips?.length > 0 && (
            <div className={styles.fieldDiff}>
              {d.skips.map((s) => (
                <div key={`${s.file}.${s.item}`} className={styles.skipRow}>
                  <span className={styles.skipTag}>缺依赖</span>
                  <code>{s.item}</code>
                  <span className={styles.skipWhy}>{s.reason}</span>
                </div>
              ))}
            </div>
          )}
          {/* 字段级差异逐条列出：settings.json 的改动比 md 隐蔽，
              只说「差 N 项」用户仍然不知道会被动什么 */}
          {d.fieldDiff?.length > 0 && (
            <div className={styles.fieldDiff}>
              {d.fieldDiff.map((f) => (
                <div
                  key={`${f.file}.${f.field}`}
                  className={`${styles.diffRow} ${styles.clickable}`}
                  role="button"
                  tabIndex={0}
                  onClick={() =>
                    setDiff({
                      field: f.field,
                      file: f.file,
                      from: f.current,
                      to: f.target,
                      hostname: d.hostname,
                      fromLabel: "本机当前",
                      toLabel: "配置源",
                    })
                  }
                  onKeyDown={(e) => e.key === "Enter" && e.currentTarget.click()}
                >
                  <code>{f.field}</code>
                  <span className={styles.diffFrom}>{fmt(f.current, f.field)}</span>
                  <span className={styles.diffArrow}>→</span>
                  <span className={styles.diffTo}>{fmt(f.target, f.field)}</span>
                  <span className={styles.detail}>详情</span>
                </div>
              ))}
            </div>
          )}
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
  // 「当前同步：settings.json 的 model」——如实说明会动用户什么，而不是只说「已开启」
  const syncedFieldsText = (info?.syncedFields ?? [])
    .filter((f) => f.fields.length > 0)
    .map((f) => `${f.file.split("/").pop()} 的 ${f.fields.join("、")}`)
    .join("；");

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

      {/* 字段级同步是独立开关：改 settings.json 的风险比搬 md 高得多，
          用户完全可能只想共用 CLAUDE.md 而不希望别的机器来动自己的 settings */}
      <div className={styles.device}>
        <div className={styles.devInfo}>
          <div className={styles.devName}>同步 settings.json 里的配置项</div>
          <div className={styles.devMeta}>
            {syncedFieldsText
              ? `当前同步：${syncedFieldsText}`
              : "同步 model 与通用 hooks；配对 hook 和指向本机路径的字段不动"}
          </div>
        </div>
        <div className={styles.devActions}>
          <Switch
            checked={!!info?.fieldSyncEnabled}
            disabled={!info?.enabled}
            onChange={(checked) => toggleFieldSync(!!checked)}
          />
        </div>
      </div>

      {/* 近期改动：字段级同步是静默生效的，用户不会察觉自己的 model 被另一台机器改了。
          开关说明「会动什么」，这里回答「已经动了什么」 */}
      {(info?.recentChanges?.length ?? 0) > 0 && (
        <div className={styles.changes}>
          <div className={styles.changesTitle}>近期改动</div>
          {info!.recentChanges.map((c, i) => (
            <div
              key={`${c.at}-${c.machineId}-${c.field}-${i}`}
              className={`${styles.changeRow} ${styles.clickable}`}
              role="button"
              tabIndex={0}
              onClick={() =>
                setDiff({
                  field: c.field,
                  file: c.file,
                  from: c.from,
                  to: c.to,
                  hostname: c.hostname || c.machineId,
                  fromLabel: "改动前",
                  toLabel: "改动后",
                })
              }
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.click()}
            >
              <span className={styles.changeHost}>{c.hostname || c.machineId}</span>
              <code>{c.field}</code>
              <span className={styles.diffFrom}>{fmt(c.from, c.field)}</span>
              <span className={styles.diffArrow}>→</span>
              <span className={styles.diffTo}>{fmt(c.to, c.field)}</span>
              <span className={styles.changeAt}>{ago(c.at)}</span>
            </div>
          ))}
        </div>
      )}

      <div className={styles.note}>
        <div className={styles.noteTitle}>同步范围</div>
        <div className={styles.noteBody}>
          <strong>整份同步</strong>：<code>CLAUDE.md</code>、<code>agents/</code>、
          <code>commands/</code>、<code>skills/</code>，以及 Codex 的 <code>AGENTS.md</code> 与
          <code>prompts/</code>。
          <br />
          <strong>按字段同步</strong>：<code>settings.json</code> 的 <code>model</code> 与{" "}
          <code>hooks</code>、<code>~/.claude.json</code> 的 <code>mcpServers</code>（MCP 服务器）、
          Codex <code>config.toml</code> 的 <code>model</code>，合并进本机文件 ——
          你自己写的其它字段、表段与<strong>注释</strong>一律原样保留。
          <code>~/.claude.json</code> 里的登录凭据与项目历史一个字节都不动。
          <br />
          <strong>MCP</strong>：命令里的家目录路径会自动换算（
          <code>/Users/你/.local/bin/x</code> 到对方机器上变成他自己的家目录、Windows 上换成
          反斜杠），所以路径不同也能用。但 <code>env</code>（常放 API key）
          <strong>不同步</strong>，需要在各台机器上自行填；本机已填好的不会被覆盖掉。
          <br />
          <strong>依赖会尽量一起带过去</strong>：<code>~/.claude/hooks/</code> 下的脚本、
          以及 MCP / hook 用 <code>--config</code> 引用到的 <code>~/.claude/*.json</code>
          都会同步（脚本自动补执行位；只带**被引用到的**配置文件，不会把目录里其它东西搬走）。
          <br />
          <strong>但二进制不分发</strong> —— 平台相关（mac 编的 Windows 用不了）。
          可执行文件在这台机器上找不到时，该条会被<strong>跳过</strong>而不是照搬，
          并在上面标出「缺依赖」和原因；把它装好，下一轮自己就恢复了。
          <br />
          <strong>hooks 只同步「通用」条目</strong>（<code>npx prettier --write</code>、
          <code>~/.claude/hooks/x</code> 这类）。本客户端自己写入的配对 hook、以及命令是
          <strong>绝对路径</strong>（<code>/Users/…</code>、<code>/Applications/…</code>）的 hook
          <strong>留在原机不动，也不会外传</strong> —— 它们换台机器就不成立，
          覆盖过去会让那台的会话配对静默失效。
          <br />
          <code>apiKeyHelper</code>、<code>statusLine</code>、<code>permissions</code>、
          <code>env</code> 等字段<strong>永不同步</strong>；即使是可同步字段，
          值里含绝对路径时也会自动跳过。
          <br />
          <strong>登录凭据不会同步</strong>，它们不会离开你本机。
          <br />
          被改动的文件会在原地留一份 <code>.am-bak</code> 备份。源机删除的文件<strong>不会</strong>
          在其它设备上被删除。
        </div>
      </div>

      <DiffModal target={diff} onClose={() => setDiff(null)} />
    </div>
  );
};

/** hooks 里的命令条数。顶层只有事件数（改一条命令前后都是「2 项」，等于没说），
 *  真正会变的是内层条目，所以数到命令这一层。 */
function countHooks(v: unknown): number {
  if (!v || typeof v !== "object") return 0;
  return Object.values(v as Record<string, unknown>).reduce<number>((n, entries) => {
    if (!Array.isArray(entries)) return n;
    return (
      n +
      entries.reduce<number>((m, e) => {
        const inner = (e as Record<string, unknown> | null)?.hooks;
        return m + (Array.isArray(inner) ? inner.length : 0);
      }, 0)
    );
  }, 0);
}

/** 配置项值的展示形态。复合值不展开，只给规模 —— 一行里塞不下，但「{…}」等于没说。 */
function fmt(v: unknown, field?: string): string {
  if (v === undefined || v === null) return "（无）";
  if (typeof v === "string") return v;
  if (field === "hooks" && typeof v === "object") {
    return `${Object.keys(v as object).length} 个事件 · ${countHooks(v)} 条`;
  }
  if (Array.isArray(v)) return `${v.length} 项`;
  if (typeof v === "object") return `${Object.keys(v as object).length} 项`;
  return String(v);
}

/** 相对时间。改动是几十秒级别发生的，所以要精确到分钟以内。 */
function ago(at: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - at);
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return `${Math.floor(s / 86400)} 天前`;
}

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
  // 文件都到齐了，但配置项可能还没跟上 —— 此时不能说「已一致」，
  // 否则会和下面逐条列出的字段差异自相矛盾
  const fields = d.fieldDiff?.length ?? 0;
  if (fields > 0) {
    return d.online
      ? `本机 ${d.fileCount} 份配置 · 文件已一致，${fields} 个配置项同步中`
      : `本机 ${d.fileCount} 份配置 · 文件已一致，${fields} 个配置项等上线后同步`;
  }
  return `本机 ${d.fileCount} 份配置 · 已与配置源一致`;
}

export default ConfigSyncPanel;
