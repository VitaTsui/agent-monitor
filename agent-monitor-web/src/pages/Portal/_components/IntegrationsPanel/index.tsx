import React, { useCallback, useEffect, useState } from "react";

import { Tooltip, message } from "antd";
import {
  CheckOutlined,
  CopyOutlined,
  DingtalkOutlined,
  FolderOutlined,
  RightOutlined,
  WechatOutlined,
} from "@ant-design/icons";

import { Button, Input, Modal, Switch } from "@hsu-react/ui";

import {
  IntegrationsInfo,
  getIntegrations,
  getTaskDirs,
  setDingtalkApp,
  setDingtalkRecvDir,
  setDingtalkRobot,
  setWecomApp,
  testDingtalkRobot,
} from "@/services/apis/portal";
import styles from "./index.module.scss";

const CopyBtn: React.FC<{ text: string }> = ({ text }) => {
  const [done, setDone] = useState(false);
  return (
    <Tooltip title="复制">
      <span
        className={styles.copyBtn}
        role="button"
        onClick={() =>
          navigator.clipboard?.writeText(text).then(() => {
            setDone(true);
            message.success("已复制");
            window.setTimeout(() => setDone(false), 1500);
          })
        }
      >
        {done ? <CheckOutlined /> : <CopyOutlined />}
      </span>
    </Tooltip>
  );
};

type Editing = "robot" | "wecom" | "ding" | null;

/**
 * 机器人接入：外层三个精简卡片，点击卡片打开对应弹窗配置。
 * - 钉钉群机器人：主动推送
 * - 企业微信自建应用 / 钉钉企业应用：双向遥控
 */
const IntegrationsPanel: React.FC = () => {
  const [editing, setEditing] = useState<Editing>(null);
  // 钉钉群机器人（推送）
  const [robot, setRobot] = useState({
    webhook: "",
    secret: "",
    hasSecret: false,
    waiting: true,
    finished: true,
    newSession: true,
    device: false,
  });
  const [robotSaving, setRobotSaving] = useState(false);
  // 企业微信自建应用
  const [wecom, setWecom] = useState({ corpId: "", token: "", aesKey: "", hasAesKey: false });
  const [wecomUrl, setWecomUrl] = useState("");
  const [wecomSaving, setWecomSaving] = useState(false);
  // 钉钉企业应用
  const [ding, setDing] = useState({
    appSecret: "",
    hasSecret: false,
    appKey: "",
    stream: false,
  });
  const [dingUrl, setDingUrl] = useState("");
  const [dingSaving, setDingSaving] = useState(false);

  // 机器人文件接收目录（通用）：按设备分组的项目
  type RecvProj = { cwd: string; name: string; dir: string; taskId?: string | null };
  const [recvDevices, setRecvDevices] = useState<
    { machineId: string; hostname: string; projects: RecvProj[] }[]
  >([]);
  const [recvModalOpen, setRecvModalOpen] = useState(false);
  // 每个项目目录输入框的当前值（cwd → 目录）；可手输或由「浏览」回填
  const [recvEdits, setRecvEdits] = useState<Record<string, string>>({});
  const recvConfiguredCount = recvDevices.reduce(
    (n, d) => n + d.projects.filter((p) => p.dir).length,
    0
  );
  // 目录选择器：为哪个项目开着 + 浏览态
  const [picker, setPicker] = useState<{
    cwd: string;
    name: string;
    taskId: string;
  } | null>(null);
  const [pickRel, setPickRel] = useState("");
  const [pickDirs, setPickDirs] = useState<string[]>([]);
  const [pickLoading, setPickLoading] = useState(false);
  const pickSeq = React.useRef(0);

  const loadPickDirs = useCallback(
    (taskId: string, rel: string, attempt = 0) => {
      const seq = ++pickSeq.current;
      setPickLoading(true);
      getTaskDirs(taskId, rel)
        .then((res) => {
          if (seq !== pickSeq.current) return;
          if (res.code !== 0) {
            setPickLoading(false);
            message.error(res.msg ?? "读取目录失败");
            return;
          }
          if (res.data?.pending && attempt < 8) {
            window.setTimeout(() => loadPickDirs(taskId, rel, attempt + 1), 1200);
            return;
          }
          setPickDirs(res.data?.dirs ?? []);
          setPickLoading(false);
        })
        .catch(() => seq === pickSeq.current && setPickLoading(false));
    },
    []
  );

  const openPicker = (p: RecvProj) => {
    if (!p.taskId) {
      message.info("该项目当前无活跃会话，无法浏览目录；可等它有会话后再选");
      return;
    }
    // 打开时定位到「当前选中目录的父级」，方便看到它和同级目录。
    // 绝对路径 / 无法在项目树里相对浏览 → 从项目根开始。
    const cur = (recvEdits[p.cwd] ?? p.dir).trim().replace(/^\.\//, "");
    const isAbs = cur.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(cur);
    const startRel =
      cur && !isAbs ? cur.split("/").filter(Boolean).slice(0, -1).join("/") : "";
    setPicker({ cwd: p.cwd, name: p.name, taskId: p.taskId });
    setPickRel(startRel);
    setPickDirs([]);
    loadPickDirs(p.taskId, startRel);
  };

  const commitRecvDir = (cwd: string, dir: string) => {
    setDingtalkRecvDir(cwd, dir)
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success(dir ? "已保存" : "已清除，回落默认 tmp");
        setRecvDevices((devs) =>
          devs.map((d) => ({
            ...d,
            projects: d.projects.map((x) => (x.cwd === cwd ? { ...x, dir } : x)),
          }))
        );
        setPicker(null);
      })
      .catch(() => message.error("保存失败，请检查网络"));
  };

  const load = useCallback(() => {
    getIntegrations()
      .then((res) => {
        if (res.code !== 0 || !res.data) return;
        const d: IntegrationsInfo = res.data;
        if (d.dingtalkRobot) setRobot({ ...d.dingtalkRobot, secret: "" });
        if (d.wecomApp) {
          setWecom({
            corpId: d.wecomApp.corpId,
            token: d.wecomApp.token,
            aesKey: "",
            hasAesKey: d.wecomApp.hasAesKey,
          });
          setWecomUrl(d.wecomApp.callbackUrl);
        }
        if (d.dingtalkApp) {
          setDing({
            appSecret: "",
            hasSecret: d.dingtalkApp.hasSecret,
            appKey: d.dingtalkApp.appKey ?? "",
            stream: !!d.dingtalkApp.stream,
          });
          setDingUrl(d.dingtalkApp.callbackUrl);
        }
        const devs = (d.recvDirDevices ?? []).map((dev) => ({
          machineId: dev.machineId,
          hostname: dev.hostname,
          projects: dev.projects.map((p) => ({
            cwd: p.cwd,
            name: p.name,
            dir: p.dir ?? "",
            taskId: p.taskId,
          })),
        }));
        setRecvDevices(devs);
        // 输入框初值 = 各项目已配目录
        const edits: Record<string, string> = {};
        devs.forEach((dev) =>
          dev.projects.forEach((p) => (edits[p.cwd] = p.dir))
        );
        setRecvEdits(edits);
      })
      .catch(() => void 0);
  }, []);
  useEffect(() => load(), [load]);

  const saveRobot = (thenTest?: boolean) => {
    if (!robot.webhook.trim()) {
      message.warning("请填写钉钉机器人 Webhook 地址");
      return;
    }
    setRobotSaving(true);
    setDingtalkRobot({
      webhook: robot.webhook.trim(),
      secret: robot.secret || undefined,
      waiting: robot.waiting,
      finished: robot.finished,
      newSession: robot.newSession,
      device: robot.device,
    })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        setRobot((r) => ({ ...r, secret: "", hasSecret: r.hasSecret || !!r.secret }));
        if (thenTest) {
          testDingtalkRobot().then((r) =>
            r.code === 0
              ? message.success("已保存并发送测试推送，去钉钉群看看")
              : message.error(r.msg ?? "测试推送失败"),
          );
        } else {
          message.success("已保存");
          setEditing(null);
        }
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setRobotSaving(false));
  };

  const saveWecom = () => {
    if (!wecom.corpId.trim() || !wecom.token.trim() || (!wecom.aesKey.trim() && !wecom.hasAesKey)) {
      message.warning("请填写 CorpID、Token 和 EncodingAESKey");
      return;
    }
    setWecomSaving(true);
    setWecomApp({
      corpId: wecom.corpId.trim(),
      token: wecom.token.trim(),
      aesKey: wecom.aesKey.trim() || undefined,
    })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success("已保存，把下方回调地址填进企业微信自建应用");
        setWecom((w) => ({ ...w, aesKey: "", hasAesKey: true }));
        if (res.data?.callbackUrl) setWecomUrl(res.data.callbackUrl);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setWecomSaving(false));
  };

  const saveDing = () => {
    if (!ding.appSecret.trim() && !ding.hasSecret) {
      message.warning("请填写钉钉应用的 AppSecret");
      return;
    }
    const useStream = !!ding.appKey.trim();
    setDingSaving(true);
    setDingtalkApp({
      appSecret: ding.appSecret.trim() || undefined,
      // AppKey 是可见字段，始终以当前输入为准（清空即切回 HTTP 回调模式）
      appKey: ding.appKey.trim(),
    })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success(
          useStream
            ? "已保存并启用 Stream 长连接，无需公网回调地址，去钉钉里 @机器人 试试"
            : "已保存，把下方回调地址填进钉钉应用的消息接收",
        );
        setDing((d) => ({ ...d, appSecret: "", hasSecret: true, stream: useStream }));
        if (res.data?.callbackUrl) setDingUrl(res.data.callbackUrl);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setDingSaving(false));
  };

  const cards: {
    key: Exclude<Editing, null>;
    icon: React.ReactNode;
    iconCls: string;
    title: string;
    type: string;
    typeCls: string;
    sub: string;
    on: boolean;
  }[] = [
    {
      key: "robot",
      icon: <DingtalkOutlined />,
      iconCls: styles.ding,
      title: "钉钉群机器人",
      type: "主动推送",
      typeCls: styles.push,
      sub: "会话状态变化时主动推到你的钉钉群",
      on: !!robot.webhook,
    },
    {
      key: "ding",
      icon: <DingtalkOutlined />,
      iconCls: styles.ding,
      title: "钉钉企业应用",
      type: "双向遥控",
      typeCls: styles.two,
      sub: "在钉钉里 @机器人 发指令遥控会话",
      on: !!dingUrl,
    },
    {
      key: "wecom",
      icon: <WechatOutlined />,
      iconCls: styles.wecom,
      title: "企业微信自建应用",
      type: "双向遥控",
      typeCls: styles.two,
      sub: "在企业微信里发指令遥控会话",
      on: !!wecomUrl,
    },
  ];

  const renderCard = (c: (typeof cards)[number]) => (
    <div
      key={c.key}
      className={styles.card}
      role="button"
      tabIndex={0}
      onClick={() => setEditing(c.key)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          setEditing(c.key);
        }
      }}
    >
      <span className={`${styles.icon} ${c.iconCls}`}>{c.icon}</span>
      <div className={styles.headText}>
        <div className={styles.headTitle}>
          {c.title}
          <span className={`${styles.typeTag} ${c.typeCls}`}>{c.type}</span>
        </div>
        <div className={styles.headSub}>{c.sub}</div>
      </div>
      <span className={`${styles.status} ${c.on ? styles.on : ""}`}>
        {c.on ? "已启用" : "未配置"}
      </span>
      <RightOutlined className={styles.arrow} />
    </div>
  );

  return (
    <div className={styles.IntegrationsPanel}>
      {/* 钉钉 */}
      <div className={styles.groupTitle}>钉钉</div>
      {cards.filter((c) => c.key === "robot" || c.key === "ding").map(renderCard)}

      {/* 企业微信 */}
      <div className={styles.groupTitle}>企业微信</div>
      {cards.filter((c) => c.key === "wecom").map(renderCard)}

      {/* 文件接收目录（所有渠道通用，弹窗按 设备→项目 配置） */}
      <div className={styles.groupTitle}>文件接收目录</div>
      <div
        className={styles.card}
        role="button"
        tabIndex={0}
        onClick={() => setRecvModalOpen(true)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setRecvModalOpen(true);
          }
        }}
      >
        <span className={`${styles.icon} ${styles.folder}`}>
          <FolderOutlined />
        </span>
        <div className={styles.headText}>
          <div className={styles.headTitle}>机器人文件接收目录</div>
          <div className={styles.headSub}>
            发给机器人的文件落到哪 · 按设备/项目配置 · 默认 tmp
          </div>
        </div>
        <span className={styles.status}>
          {recvConfiguredCount ? `已配 ${recvConfiguredCount}` : "默认"}
        </span>
        <RightOutlined className={styles.arrow} />
      </div>

      {/* 接收目录配置弹窗：设备 → 项目 层级列全 */}
      <Modal
        title="机器人文件接收目录"
        open={recvModalOpen}
        onCancel={() => setRecvModalOpen(false)}
        footer={null}
        width={560}
        centered
      >
        <div className={styles.recvModalHint}>
          发给机器人（钉钉/企业微信）的文件，随下一条任务落到对应会话项目的这个目录。
          留空 = 默认 <code>项目/tmp</code>。点「浏览」在项目目录树里选。
        </div>
        {recvDevices.length === 0 ? (
          <div className={styles.recvEmpty}>暂无项目（有活跃会话后自动出现）</div>
        ) : (
          recvDevices.map((dev) => (
            <div key={dev.machineId || dev.hostname} className={styles.recvDev}>
              <div className={styles.recvDevName}>💻 {dev.hostname}</div>
              {dev.projects.map((p) => (
                <div key={p.cwd} className={styles.recvProjRow}>
                  <div className={styles.recvProjName} title={p.cwd}>
                    <div className={styles.recvProjTitle}>{p.name}</div>
                    <div className={styles.recvProjPath}>{p.cwd}</div>
                  </div>
                  <div className={styles.recvProjEdit}>
                    <Input
                      className={styles.recvProjInput}
                      placeholder="tmp（默认）· 可直接输入或点浏览"
                      value={recvEdits[p.cwd] ?? p.dir}
                      onChange={(v) =>
                        setRecvEdits((m) => ({ ...m, [p.cwd]: v }))
                      }
                    />
                    <Button size="small" onClick={() => openPicker(p)}>
                      浏览
                    </Button>
                    <Button
                      size="small"
                      type="primary"
                      onClick={() =>
                        commitRecvDir(p.cwd, (recvEdits[p.cwd] ?? "").trim())
                      }
                    >
                      保存
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          ))
        )}
      </Modal>

      {/* 目录浏览选择弹窗（复用会话目录树浏览） */}
      <Modal
        title={picker ? `选择接收目录 · ${picker.name}` : "选择接收目录"}
        open={!!picker}
        onCancel={() => setPicker(null)}
        onOk={() => {
          // 回填到该项目输入框（不立即保存，可再改，再点保存）
          if (picker) setRecvEdits((m) => ({ ...m, [picker.cwd]: pickRel }));
          setPicker(null);
        }}
        okText={`用此目录（${pickRel || "项目根"}）`}
        cancelText="取消"
        width={480}
        centered
      >
        <div className={styles.pickPath}>
          <span
            className={styles.pickCrumb}
            role="button"
            tabIndex={0}
            onClick={() => {
              setPickRel("");
              picker && loadPickDirs(picker.taskId, "");
            }}
          >
            项目根
          </span>
          {pickRel
            ? pickRel.split("/").map((seg, i, arr) => {
                const rel = arr.slice(0, i + 1).join("/");
                return (
                  <span key={rel}>
                    <span className={styles.pickSep}>/</span>
                    <span
                      className={styles.pickCrumb}
                      role="button"
                      tabIndex={0}
                      onClick={() => {
                        setPickRel(rel);
                        picker && loadPickDirs(picker.taskId, rel);
                      }}
                    >
                      {seg}
                    </span>
                  </span>
                );
              })
            : null}
        </div>
        <div className={styles.pickList}>
          {pickLoading ? (
            <div className={styles.pickLoading}>读取中…</div>
          ) : pickDirs.length === 0 ? (
            <div className={styles.pickEmpty}>此目录下没有子目录</div>
          ) : (
            pickDirs.map((name) => (
              <div
                key={name}
                className={styles.pickItem}
                role="button"
                tabIndex={0}
                onClick={() => {
                  const next = pickRel ? `${pickRel}/${name}` : name;
                  setPickRel(next);
                  picker && loadPickDirs(picker.taskId, next);
                }}
              >
                <FolderOutlined className={styles.pickIcon} />
                <span className={styles.pickItemName}>{name}</span>
                <RightOutlined className={styles.pickArrow} />
              </div>
            ))
          )}
        </div>
      </Modal>


      {/* 钉钉群机器人 */}
      <Modal
        title="钉钉群机器人 · 主动推送"
        open={editing === "robot"}
        onCancel={() => setEditing(null)}
        footer={null}
        width={520}
        centered
      >
        <div className={styles.form}>
          <div className={styles.desc}>
            个人钉钉建群即可用：群「智能群助手 → 添加机器人 → 自定义」，安全设置选「加签」。
          </div>
          <div className={styles.field}>
            <label className={styles.label}>Webhook 地址</label>
            <Input
              placeholder="粘贴钉钉机器人 Webhook"
              value={robot.webhook}
              onChange={(v) => setRobot((r) => ({ ...r, webhook: v }))}
            />
          </div>
          <div className={styles.field}>
            <label className={styles.label}>加签密钥</label>
            <Input
              placeholder={robot.hasSecret ? "已设置，留空不改" : "SEC... （推荐开启加签）"}
              value={robot.secret}
              onChange={(v) => setRobot((r) => ({ ...r, secret: v }))}
            />
          </div>
          <div className={styles.field}>
            <label className={styles.label}>推送事件</label>
            <div className={styles.events}>
            {([
              ["waiting", "等待输入"],
              ["finished", "会话结束"],
              ["newSession", "新会话"],
              ["device", "设备上线/离线"],
            ] as const).map(([k, label]) => (
              <label key={k} className={styles.event}>
                <Switch checked={robot[k]} onChange={(on) => setRobot((r) => ({ ...r, [k]: on }))} />
                <span>{label}</span>
              </label>
            ))}
            </div>
          </div>
          <div className={styles.actions}>
            <Button
              type="primary"
              className={styles.actBtn}
              loading={robotSaving}
              onClick={() => saveRobot(false)}
            >
              保存
            </Button>
            <Button className={styles.actBtn} onClick={() => saveRobot(true)}>
              保存并测试
            </Button>
          </div>
        </div>
      </Modal>

      {/* 企业微信自建应用 */}
      <Modal
        title="企业微信自建应用 · 双向遥控"
        open={editing === "wecom"}
        onCancel={() => setEditing(null)}
        footer={null}
        width={540}
        centered
      >
        <div className={styles.form}>
          <div className={styles.desc}>
            企业微信后台建自建应用，把下方回调地址填进「接收消息」，即可发指令
            （会话 / 暂停 N / 发 N 内容 …）遥控会话。
          </div>
          <div className={styles.field}>
            <label className={styles.label}>企业 CorpID</label>
            <Input
              placeholder="企业微信「我的企业」里的企业 ID"
              value={wecom.corpId}
              onChange={(v) => setWecom((w) => ({ ...w, corpId: v }))}
            />
          </div>
          <div className={styles.field}>
            <label className={styles.label}>接收消息 Token</label>
            <Input
              placeholder="自建应用「接收消息」的 Token"
              value={wecom.token}
              onChange={(v) => setWecom((w) => ({ ...w, token: v }))}
            />
          </div>
          <div className={styles.field}>
            <label className={styles.label}>EncodingAESKey</label>
            <Input
              placeholder={wecom.hasAesKey ? "已设置，留空不改" : "43 位"}
              value={wecom.aesKey}
              onChange={(v) => setWecom((w) => ({ ...w, aesKey: v }))}
            />
          </div>
          {wecomUrl ? (
            <div className={styles.urlRow}>
              <span className={styles.urlLabel}>回调地址</span>
              <span className={styles.urlValue}>{wecomUrl}</span>
              <CopyBtn text={wecomUrl} />
            </div>
          ) : null}
          <div className={styles.actions}>
            <Button type="primary" className={styles.actBtn} loading={wecomSaving} onClick={saveWecom}>
              保存并生成回调地址
            </Button>
          </div>
        </div>
      </Modal>

      {/* 钉钉企业应用 */}
      <Modal
        title="钉钉企业应用 · 双向遥控"
        open={editing === "ding"}
        onCancel={() => setEditing(null)}
        footer={null}
        width={540}
        centered
      >
        <div className={styles.form}>
          <div className={styles.desc}>
            钉钉开放平台建企业内部应用机器人。<b>推荐 Stream 模式</b>：填 AppKey + AppSecret，
            服务端主动连钉钉收消息，<b>无需公网回调地址</b>（海外服务器也能用，绕开「消息接收
            地址校验失败」）。若留空 AppKey，则回退 HTTP 回调模式，需把下方回调地址填进「消息
            接收模式 · HTTP」。
          </div>
          <div className={styles.field}>
            <label className={styles.label}>AppKey / ClientID（Stream 模式）</label>
            <Input
              placeholder="填了走 Stream 长连接；留空则用下方 HTTP 回调地址"
              value={ding.appKey}
              onChange={(v) => setDing((d) => ({ ...d, appKey: v }))}
            />
          </div>
          <div className={styles.field}>
            <label className={styles.label}>AppSecret</label>
            <Input
              placeholder={ding.hasSecret ? "已设置，留空不改" : "钉钉企业内部应用的 AppSecret"}
              value={ding.appSecret}
              onChange={(v) => setDing((d) => ({ ...d, appSecret: v }))}
            />
          </div>
          {ding.appKey.trim() ? (
            <div className={styles.desc}>
              ✅ Stream 模式：保存后服务端会自动建立长连接，钉钉里 @机器人 发指令即可，
              回调地址可忽略。
            </div>
          ) : dingUrl ? (
            <div className={styles.urlRow}>
              <span className={styles.urlLabel}>回调地址</span>
              <span className={styles.urlValue}>{dingUrl}</span>
              <CopyBtn text={dingUrl} />
            </div>
          ) : null}
          <div className={styles.actions}>
            <Button type="primary" className={styles.actBtn} loading={dingSaving} onClick={saveDing}>
              {ding.appKey.trim() ? "保存并启用 Stream" : "保存并生成回调地址"}
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
};

export default IntegrationsPanel;
