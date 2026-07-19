import React, { useCallback, useEffect, useState } from "react";

import { Switch, Tooltip, message } from "antd";
import {
  CheckOutlined,
  CopyOutlined,
  DingtalkOutlined,
  WechatOutlined,
} from "@ant-design/icons";

import { Button, Input } from "@hsu-react/ui";

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

/**
 * 机器人接入（每个用户自助配置自己的渠道）：
 * - 钉钉群机器人：会话状态变化主动推送
 * - 企业微信自建应用 / 钉钉企业应用：双向遥控（专属回调地址填进各自后台）
 */
const IntegrationsPanel: React.FC = () => {
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

  return (
    <div className={styles.IntegrationsPanel}>
      {/* 钉钉群机器人 · 主动推送 */}
      <div className={styles.card}>
        <div className={styles.cardHead}>
          <span className={`${styles.icon} ${styles.ding}`}>
            <DingtalkOutlined />
          </span>
          <div className={styles.headText}>
            <div className={styles.headTitle}>
              钉钉群机器人
              <span className={`${styles.typeTag} ${styles.push}`}>主动推送</span>
            </div>
            <div className={styles.headSub}>会话状态变化时主动推到你的钉钉群</div>
          </div>
          <span className={`${styles.status} ${robot.webhook ? styles.on : ""}`}>
            {robot.webhook ? "已启用" : "未配置"}
          </span>
        </div>
        <div className={styles.desc}>
          个人钉钉建群即可用：群「智能群助手 → 添加机器人 → 自定义」，安全设置选「加签」。
        </div>
        <Input
          placeholder="钉钉机器人 Webhook 地址"
          value={robot.webhook}
          onChange={(v) => setRobot((r) => ({ ...r, webhook: v }))}
          style={{ marginBottom: 8 }}
        />
        <Input
          placeholder={robot.hasSecret ? "加签密钥（已设置，留空不改）" : "加签密钥 SEC...（推荐）"}
          value={robot.secret}
          onChange={(v) => setRobot((r) => ({ ...r, secret: v }))}
          style={{ marginBottom: 10 }}
        />
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
        <div className={styles.actions}>
          <Button type="primary" size="small" loading={robotSaving} onClick={() => saveRobot(false)}>
            保存
          </Button>
          <Button size="small" onClick={() => saveRobot(true)}>
            保存并测试
          </Button>
        </div>
      </div>

      {/* 企业微信自建应用 · 双向 */}
      <div className={styles.card}>
        <div className={styles.cardHead}>
          <span className={`${styles.icon} ${styles.wecom}`}>
            <WechatOutlined />
          </span>
          <div className={styles.headText}>
            <div className={styles.headTitle}>
              企业微信自建应用
              <span className={`${styles.typeTag} ${styles.two}`}>双向遥控</span>
            </div>
            <div className={styles.headSub}>在企业微信里发指令遥控会话</div>
          </div>
          <span className={`${styles.status} ${wecomUrl ? styles.on : ""}`}>
            {wecomUrl ? "已启用" : "未配置"}
          </span>
        </div>
        <div className={styles.desc}>
          企业微信后台建自建应用，把下方回调地址填进「接收消息」，即可发指令
          （会话 / 暂停 N / 发 N 内容 …）遥控会话。
        </div>
        <Input
          placeholder="企业 CorpID"
          value={wecom.corpId}
          onChange={(v) => setWecom((w) => ({ ...w, corpId: v }))}
          style={{ marginBottom: 8 }}
        />
        <Input
          placeholder="接收消息 Token"
          value={wecom.token}
          onChange={(v) => setWecom((w) => ({ ...w, token: v }))}
          style={{ marginBottom: 8 }}
        />
        <Input
          placeholder={wecom.hasAesKey ? "EncodingAESKey（已设置，留空不改）" : "EncodingAESKey（43 位）"}
          value={wecom.aesKey}
          onChange={(v) => setWecom((w) => ({ ...w, aesKey: v }))}
          style={{ marginBottom: 10 }}
        />
        {wecomUrl ? (
          <div className={styles.urlRow}>
            <span className={styles.urlLabel}>回调地址</span>
            <span className={styles.urlValue}>{wecomUrl}</span>
            <CopyBtn text={wecomUrl} />
          </div>
        ) : null}
        <div className={styles.actions}>
          <Button type="primary" size="small" loading={wecomSaving} onClick={saveWecom}>
            保存并生成回调地址
          </Button>
        </div>
      </div>

      {/* 钉钉企业应用 · 双向 */}
      <div className={styles.card}>
        <div className={styles.cardHead}>
          <span className={`${styles.icon} ${styles.ding}`}>
            <DingtalkOutlined />
          </span>
          <div className={styles.headText}>
            <div className={styles.headTitle}>
              钉钉企业应用
              <span className={`${styles.typeTag} ${styles.two}`}>双向遥控</span>
            </div>
            <div className={styles.headSub}>在钉钉里 @机器人 发指令遥控会话</div>
          </div>
          <span className={`${styles.status} ${dingUrl ? styles.on : ""}`}>
            {dingUrl ? "已启用" : "未配置"}
          </span>
        </div>
        <div className={styles.desc}>
          钉钉开放平台建企业内部应用机器人，「消息接收模式」选 HTTP 填下方回调地址，
          AppSecret 用于验签。
        </div>
        <Input
          placeholder={ding.hasSecret ? "AppSecret（已设置，留空不改）" : "钉钉应用 AppSecret"}
          value={ding.appSecret}
          onChange={(v) => setDing((d) => ({ ...d, appSecret: v }))}
          style={{ marginBottom: 10 }}
        />
        {dingUrl ? (
          <div className={styles.urlRow}>
            <span className={styles.urlLabel}>回调地址</span>
            <span className={styles.urlValue}>{dingUrl}</span>
            <CopyBtn text={dingUrl} />
          </div>
        ) : null}
        <div className={styles.actions}>
          <Button type="primary" size="small" loading={dingSaving} onClick={saveDing}>
            保存并生成回调地址
          </Button>
        </div>
      </div>
    </div>
  );
};

export default IntegrationsPanel;
