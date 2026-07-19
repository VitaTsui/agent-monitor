import React, { useCallback, useEffect, useState } from "react";

import { Switch, Tooltip, message } from "antd";
import {
  CheckOutlined,
  CopyOutlined,
  DingtalkOutlined,
  RightOutlined,
  WechatOutlined,
} from "@ant-design/icons";

import { Button, Input, Modal } from "@hsu-react/ui";

import {
  IntegrationsInfo,
  getIntegrations,
  setDingtalkApp,
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
  const [ding, setDing] = useState({ appSecret: "", hasSecret: false });
  const [dingUrl, setDingUrl] = useState("");
  const [dingSaving, setDingSaving] = useState(false);

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
          setDing({ appSecret: "", hasSecret: d.dingtalkApp.hasSecret });
          setDingUrl(d.dingtalkApp.callbackUrl);
        }
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
    setDingSaving(true);
    setDingtalkApp({ appSecret: ding.appSecret.trim() || undefined })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success("已保存，把下方回调地址填进钉钉应用的消息接收");
        setDing((d) => ({ ...d, appSecret: "", hasSecret: true }));
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
      key: "wecom",
      icon: <WechatOutlined />,
      iconCls: styles.wecom,
      title: "企业微信自建应用",
      type: "双向遥控",
      typeCls: styles.two,
      sub: "在企业微信里发指令遥控会话",
      on: !!wecomUrl,
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
  ];

  return (
    <div className={styles.IntegrationsPanel}>
      {cards.map((c) => (
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
      ))}

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
            钉钉开放平台建企业内部应用机器人，「消息接收模式」选 HTTP 填下方回调地址，
            AppSecret 用于验签。
          </div>
          <div className={styles.field}>
            <label className={styles.label}>AppSecret</label>
            <Input
              placeholder={ding.hasSecret ? "已设置，留空不改" : "钉钉企业内部应用的 AppSecret"}
              value={ding.appSecret}
              onChange={(v) => setDing((d) => ({ ...d, appSecret: v }))}
            />
          </div>
          {dingUrl ? (
            <div className={styles.urlRow}>
              <span className={styles.urlLabel}>回调地址</span>
              <span className={styles.urlValue}>{dingUrl}</span>
              <CopyBtn text={dingUrl} />
            </div>
          ) : null}
          <div className={styles.actions}>
            <Button type="primary" className={styles.actBtn} loading={dingSaving} onClick={saveDing}>
              保存并生成回调地址
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
};

export default IntegrationsPanel;
