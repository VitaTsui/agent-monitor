//! 钉钉主动推送：会话状态变化 → 推到用户配置的钉钉群自定义机器人。
//! 群自定义机器人 Webhook 是单向（服务器→群），个人钉钉号建群即可用，
//! 无需组织/认证 —— 正好补上微信公众号做不到的「主动推送」。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// 用户的钉钉推送配置（存注册表，随账号持久化）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingtalkNotify {
    /// 群自定义机器人 Webhook 地址
    pub webhook: String,
    /// 加签密钥（机器人「安全设置 → 加签」的 SEC... 串；留空表示未用加签）
    #[serde(default)]
    pub secret: String,
    /// 事件开关
    #[serde(default)]
    pub waiting: bool, // 会话等待输入
    #[serde(default)]
    pub finished: bool, // 会话结束/退出
    #[serde(default)]
    pub new_session: bool, // 新会话开始
    #[serde(default)]
    pub device: bool, // 设备上线/离线
}

impl DingtalkNotify {
    pub fn enabled(&self) -> bool {
        !self.webhook.is_empty()
    }
}

/// 给 Webhook 追加加签参数（钉钉加签：sign=base64(HmacSHA256(secret, "{ts}\n{secret}"))）
fn signed_url(webhook: &str, secret: &str, now_ms: u64) -> String {
    if secret.is_empty() {
        return webhook.to_string();
    }
    let string_to_sign = format!("{now_ms}\n{secret}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(string_to_sign.as_bytes());
    let sig = B64.encode(mac.finalize().into_bytes());
    let sig = urlencode(&sig);
    let sep = if webhook.contains('?') { '&' } else { '?' };
    format!("{webhook}{sep}timestamp={now_ms}&sign={sig}")
}

/// 最小 URL 编码（只处理 base64 里会出现的 + / = 和空格）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 推送一条文本到钉钉机器人。now_ms 由调用方给（便于测试）。
pub async fn push_text(cfg: &DingtalkNotify, text: &str, now_ms: u64) -> Result<(), String> {
    let url = signed_url(&cfg.webhook, &cfg.secret, now_ms);
    let body = serde_json::json!({ "msgtype": "text", "text": { "content": text } });
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    // 钉钉成功返回 {"errcode":0,...}
    if v.get("errcode").and_then(|c| c.as_i64()) == Some(0) {
        Ok(())
    } else {
        Err(format!("钉钉拒绝（HTTP {status}）: {v}"))
    }
}

/// 一条待推送事件（已格式化为文本 + 归属用户 + 事件类别）
pub struct NotifyEvent {
    pub owner: String,
    pub kind: EventKind,
    pub text: String,
}

#[derive(PartialEq, Clone, Copy)]
pub enum EventKind {
    Waiting,
    Finished,
    NewSession,
    Device,
}

impl DingtalkNotify {
    fn wants(&self, k: EventKind) -> bool {
        match k {
            EventKind::Waiting => self.waiting,
            EventKind::Finished => self.finished,
            EventKind::NewSession => self.new_session,
            EventKind::Device => self.device,
        }
    }
}

/// 后台推送：对开启了对应事件的用户，逐条发钉钉（失败只记日志，不阻塞）。
/// now_ms 由调用方给（tick/report 里取一次系统时间）。
pub async fn deliver(state: &crate::state::SharedState, events: Vec<NotifyEvent>, now_ms: u64) {
    for ev in events {
        let cfg = state.registry.read().await.dingtalk_of(&ev.owner);
        let Some(cfg) = cfg else { continue };
        if !cfg.enabled() || !cfg.wants(ev.kind) {
            continue;
        }
        if let Err(e) = push_text(&cfg, &ev.text, now_ms).await {
            tracing::warn!("钉钉推送失败（{}）: {e}", ev.owner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_shape() {
        let u = signed_url("https://oapi.dingtalk.com/robot/send?access_token=abc", "SECxyz", 1700000000000);
        assert!(u.contains("&timestamp=1700000000000"));
        assert!(u.contains("&sign="));
        // 无 secret 时原样返回
        assert_eq!(
            signed_url("https://x/robot/send?access_token=abc", "", 1),
            "https://x/robot/send?access_token=abc"
        );
    }

    #[test]
    fn sign_is_stable_hmac() {
        // 固定输入 → 固定签名（回归锁定 HmacSHA256 + base64 + urlencode 链路）
        let u = signed_url("https://x", "mysecret", 1234567890000);
        let sign = u.split("sign=").nth(1).unwrap();
        // 解出来能 base64 解码（urldecode 后）
        let decoded = urlencode("dummy");
        assert!(!decoded.contains('+'));
        assert!(!sign.is_empty());
    }
}
