import React, { useCallback, useEffect, useState } from "react";

import { message } from "antd";
import { Button, Input, Panel } from "@hsu-react/ui";

import {
  getDingtalkAppAdmin,
  setDingtalkAppAdmin,
} from "@/services/apis/sysmgmt/dingtalk";
import styles from "./index.module.scss";

/**
 * 机器人接入（后管）：全局配置一个钉钉企业应用，全体用户共用这一个机器人。
 * 普通用户不再各自配置应用——他们给机器人发消息，机器人回登录链接，登录后把自己的
 * 钉钉 id 绑到账号即可（见前台设置「机器人接入 → 已绑定的钉钉」）。
 */
const Dingtalk: React.FC = () => {
  const [appKey, setAppKey] = useState("");
  const [appSecret, setAppSecret] = useState("");
  const [hasSecret, setHasSecret] = useState(false);
  const [stream, setStream] = useState(false);
  const [callbackUrl, setCallbackUrl] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    getDingtalkAppAdmin()
      .then((res) => {
        if (res.code === 0 && res.data) {
          setAppKey(res.data.appKey ?? "");
          setHasSecret(res.data.hasSecret);
          setStream(res.data.stream);
          setCallbackUrl(res.data.callbackUrl);
        } else {
          message.error(res.msg ?? "加载失败");
        }
      })
      .catch(() => message.error("加载失败，请检查网络"));
  }, []);

  useEffect(() => load(), [load]);

  const save = () => {
    if (!appSecret.trim() && !hasSecret) {
      message.warning("请填写钉钉应用的 AppSecret");
      return;
    }
    setSaving(true);
    setDingtalkAppAdmin({
      appSecret: appSecret.trim() || undefined,
      appKey: appKey.trim(),
    })
      .then((res) => {
        if (res.code !== 0) {
          return message.error(res.msg ?? "保存失败");
        }
        const useStream = !!appKey.trim();
        message.success(
          useStream
            ? "已保存并启用 Stream 长连接，无需公网回调地址"
            : "已保存，把下方回调地址填进钉钉应用的「消息接收(HTTP)」"
        );
        setAppSecret("");
        setHasSecret(true);
        setStream(useStream);
        setCallbackUrl(res.data?.callbackUrl ?? null);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setSaving(false));
  };

  const copyCallback = () => {
    if (!callbackUrl) return;
    navigator.clipboard?.writeText(callbackUrl).then(() => {
      setCopied(true);
      message.success("已复制");
      window.setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <Panel.Default className={styles.Dingtalk}>
      <div className={styles.section}>
        <div className={styles.sectionTitle}>钉钉企业应用（全局机器人）</div>
        <div className={styles.hint}>
          在钉钉开放平台建企业内部应用机器人，把 AppKey + AppSecret 填这里。
          <b>推荐 Stream 模式</b>：填了 AppKey 服务端主动连钉钉收消息，
          <b>无需公网回调地址</b>（海外服务器也能用）。留空 AppKey 则回退 HTTP
          回调模式，需把下方回调地址填进「消息接收模式 · HTTP」。
          <br />
          全体用户共用这一个机器人：他们给机器人发消息 → 机器人回登录链接 →
          登录后绑定自己的钉钉 id，之后任务通知私聊推送、也能发指令遥控会话。
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
            placeholder={hasSecret ? "已设置，留空不改" : "钉钉企业内部应用的 AppSecret"}
            value={appSecret}
            onChange={(v) => setAppSecret(v)}
          />
        </div>

        {appKey.trim() ? (
          <div className={styles.streamNote}>
            ✅ Stream 模式：保存后服务端会自动建立长连接，钉钉里 @机器人 发指令即可，
            回调地址可忽略。
          </div>
        ) : callbackUrl ? (
          <div className={styles.urlRow}>
            <span className={styles.urlLabel}>回调地址</span>
            <span className={styles.urlValue}>{callbackUrl}</span>
            <Button size="small" onClick={copyCallback}>
              {copied ? "已复制" : "复制"}
            </Button>
          </div>
        ) : null}

        <div className={styles.actions}>
          <Button type="primary" loading={saving} onClick={save}>
            {appKey.trim() ? "保存并启用 Stream" : "保存并生成回调地址"}
          </Button>
          <span className={styles.status}>
            {stream ? "当前：Stream 长连接" : hasSecret ? "当前：HTTP 回调" : "未配置"}
          </span>
        </div>
      </div>
    </Panel.Default>
  );
};

export default Dingtalk;
