import React, { useCallback, useEffect, useState } from "react";

import { message } from "antd";
import { Button, Input, Panel, Switch } from "@hsu-react/ui";

import {
  getDingtalkAppAdmin,
  getDingtalkRobotAdmin,
  getWecomAppAdmin,
  setDingtalkAppAdmin,
  setDingtalkRobotAdmin,
  setWecomAppAdmin,
  testDingtalkRobotAdmin,
} from "@/services/apis/sysmgmt/dingtalk";
import styles from "./index.module.scss";

/**
 * 机器人接入（后管）：全局配置钉钉企业应用 / 钉钉群机器人 / 企业微信，全体用户共用。
 * 普通用户不再各自配置——给企业应用机器人发消息，按回复的登录链接绑定自己的钉钉 id 即可。
 */
const Dingtalk: React.FC = () => {
  // 钉钉企业应用（双向 Stream 机器人）
  const [appKey, setAppKey] = useState("");
  const [appSecret, setAppSecret] = useState("");
  const [appHasSecret, setAppHasSecret] = useState(false);
  const [appCallback, setAppCallback] = useState<string | null>(null);
  const [appSaving, setAppSaving] = useState(false);

  // 钉钉群机器人（webhook 推送）
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
  const [wecomCallback, setWecomCallback] = useState<string | null>(null);
  const [wecomSaving, setWecomSaving] = useState(false);

  const load = useCallback(() => {
    getDingtalkAppAdmin()
      .then((res) => {
        if (res.code === 0 && res.data) {
          setAppKey(res.data.appKey ?? "");
          setAppHasSecret(res.data.hasSecret);
          setAppCallback(res.data.callbackUrl);
        }
      })
      .catch(() => void 0);
    getDingtalkRobotAdmin()
      .then((res) => {
        if (res.code === 0 && res.data) {
          setRobot({ ...res.data, secret: "", hasSecret: res.data.hasSecret });
        }
      })
      .catch(() => void 0);
    getWecomAppAdmin()
      .then((res) => {
        if (res.code === 0 && res.data) {
          setWecom({
            corpId: res.data.corpId,
            token: res.data.token,
            aesKey: "",
            hasAesKey: res.data.hasAesKey,
          });
          setWecomCallback(res.data.callbackUrl);
        }
      })
      .catch(() => void 0);
  }, []);

  useEffect(() => load(), [load]);

  const saveApp = () => {
    if (!appSecret.trim() && !appHasSecret) {
      message.warning("请填写钉钉应用的 AppSecret");
      return;
    }
    setAppSaving(true);
    setDingtalkAppAdmin({ appSecret: appSecret.trim() || undefined, appKey: appKey.trim() })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success(
          appKey.trim()
            ? "已保存并启用 Stream 长连接，无需公网回调地址"
            : "已保存，把下方回调地址填进钉钉应用的「消息接收(HTTP)」"
        );
        setAppSecret("");
        setAppHasSecret(true);
        setAppCallback(res.data?.callbackUrl ?? null);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setAppSaving(false));
  };

  const saveRobot = (thenTest?: boolean) => {
    if (!robot.webhook.trim()) {
      message.warning("请填写钉钉群机器人 Webhook 地址");
      return;
    }
    setRobotSaving(true);
    setDingtalkRobotAdmin({
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
          testDingtalkRobotAdmin().then((r) =>
            r.code === 0
              ? message.success("已保存并发送测试推送，去钉钉群看看")
              : message.error(r.msg ?? "测试推送失败")
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
    setWecomAppAdmin({
      corpId: wecom.corpId.trim(),
      token: wecom.token.trim(),
      aesKey: wecom.aesKey.trim() || undefined,
    })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success("已保存，把下方回调地址填进企业微信自建应用「接收消息」");
        setWecom((w) => ({ ...w, aesKey: "", hasAesKey: true }));
        setWecomCallback(res.data?.callbackUrl ?? null);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setWecomSaving(false));
  };

  const copy = (url: string | null) => {
    if (!url) return;
    navigator.clipboard?.writeText(url).then(() => message.success("已复制"));
  };

  return (
    <Panel.Default className={styles.Dingtalk}>
      {/* 钉钉企业应用 */}
      <div className={styles.section}>
        <div className={styles.sectionTitle}>钉钉企业应用（双向 · 全局机器人）</div>
        <div className={styles.hint}>
          全体用户共用这一个机器人：用户给它发消息 → 机器人回登录链接 → 登录后绑定自己的钉钉
          id，之后任务通知私聊推送、也能发指令遥控会话。<b>推荐 Stream 模式</b>（填 AppKey，
          无需公网回调）。
        </div>
        <div className={styles.field}>
          <label className={styles.label}>AppKey / ClientID（Stream 模式）</label>
          <Input
            placeholder="填了走 Stream 长连接；留空则用下方 HTTP 回调地址"
            value={appKey}
            onChange={(v) => setAppKey(v)}
          />
        </div>
        <div className={styles.field}>
          <label className={styles.label}>AppSecret</label>
          <Input
            placeholder={appHasSecret ? "已设置，留空不改" : "钉钉企业内部应用的 AppSecret"}
            value={appSecret}
            onChange={(v) => setAppSecret(v)}
          />
        </div>
        {!appKey.trim() && appCallback ? (
          <div className={styles.urlRow}>
            <span className={styles.urlLabel}>回调地址</span>
            <span className={styles.urlValue}>{appCallback}</span>
            <Button size="small" onClick={() => copy(appCallback)}>
              复制
            </Button>
          </div>
        ) : null}
        <div className={styles.actions}>
          <Button type="primary" loading={appSaving} onClick={saveApp}>
            {appKey.trim() ? "保存并启用 Stream" : "保存并生成回调地址"}
          </Button>
        </div>
      </div>

      {/* 钉钉群机器人 */}
      <div className={styles.section}>
        <div className={styles.sectionTitle}>钉钉群机器人（主动推送）</div>
        <div className={styles.hint}>
          群「智能群助手 → 添加机器人 → 自定义」，安全设置选「加签」。会话状态变化时按下方开关
          推到这个群。
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
            placeholder={robot.hasSecret ? "已设置，留空不改" : "SEC…（推荐开启加签）"}
            value={robot.secret}
            onChange={(v) => setRobot((r) => ({ ...r, secret: v }))}
          />
        </div>
        <div className={styles.events}>
          {(
            [
              ["waiting", "等待输入"],
              ["finished", "会话结束"],
              ["newSession", "新会话"],
              ["device", "设备上线/离线"],
            ] as const
          ).map(([k, label]) => (
            <label key={k} className={styles.event}>
              <Switch checked={robot[k]} onChange={(on) => setRobot((r) => ({ ...r, [k]: on }))} />
              <span>{label}</span>
            </label>
          ))}
        </div>
        <div className={styles.actions}>
          <Button type="primary" loading={robotSaving} onClick={() => saveRobot(false)}>
            保存
          </Button>
          <Button onClick={() => saveRobot(true)}>保存并测试</Button>
        </div>
      </div>

      {/* 企业微信 */}
      <div className={styles.section}>
        <div className={styles.sectionTitle}>企业微信自建应用（双向）</div>
        <div className={styles.hint}>
          企业微信后台建自建应用，把下方回调地址填进「接收消息」，即可发指令遥控会话。
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
        {wecomCallback ? (
          <div className={styles.urlRow}>
            <span className={styles.urlLabel}>回调地址</span>
            <span className={styles.urlValue}>{wecomCallback}</span>
            <Button size="small" onClick={() => copy(wecomCallback)}>
              复制
            </Button>
          </div>
        ) : null}
        <div className={styles.actions}>
          <Button type="primary" loading={wecomSaving} onClick={saveWecom}>
            保存并生成回调地址
          </Button>
        </div>
      </div>
    </Panel.Default>
  );
};

export default Dingtalk;
