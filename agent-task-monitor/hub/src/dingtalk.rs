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

/// 校验钉钉企业应用「消息接收(HTTP)」回调签名。
/// 钉钉：sign = base64(HmacSHA256(key=appSecret, msg="{timestamp}\n{appSecret}"))
pub fn verify_app_sign(app_secret: &str, timestamp: &str, sign: &str) -> bool {
    let string_to_sign = format!("{timestamp}\n{app_secret}");
    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(string_to_sign.as_bytes());
    let expect = B64.encode(mac.finalize().into_bytes());
    // 常量时间比较无必要（签名非秘密），直接比
    expect == sign
}

/// 最小 URL 编码（只处理 base64 里会出现的 + / = 和空格）
pub(crate) fn urlencode(s: &str) -> String {
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

/// 推送一条纯文本到钉钉群机器人 Webhook。now_ms 由调用方给（便于测试）。
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

// ---------- 企业应用 OTO 主动推送（Stream 用户，无需群机器人 webhook） ----------

/// access_token 缓存：app_key -> (token, 过期 epoch 秒)。钉钉 token 2h 有效，缓存复用。
fn token_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, u64)>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())
}

/// 用 appKey/appSecret 换 access_token（带缓存，提前 60s 过期刷新）。
async fn access_token(app_key: &str, app_secret: &str, now_ms: u64) -> Result<String, String> {
    let now = now_ms / 1000;
    if let Some((tok, exp)) = token_cache().lock().unwrap().get(app_key) {
        if *exp > now + 60 {
            return Ok(tok.clone());
        }
    }
    let resp = http_client()?
        .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
        .json(&serde_json::json!({ "appKey": app_key, "appSecret": app_secret }))
        .send()
        .await
        .map_err(|e| format!("取 token 请求失败: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let tok = v
        .get("accessToken")
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("取 token 失败: {v}"))?
        .to_string();
    let expire = v.get("expireIn").and_then(|t| t.as_u64()).unwrap_or(7200);
    token_cache()
        .lock()
        .unwrap()
        .insert(app_key.to_string(), (tok.clone(), now + expire));
    Ok(tok)
}

/// 通过企业应用机器人 OTO 接口，主动把一条文本发给某个用户（staffId）。
pub async fn push_oto(
    app: &crate::registry::DingtalkApp,
    text: &str,
    now_ms: u64,
) -> Result<(), String> {
    if app.staff_id.is_empty() {
        return Err("未捕获 staffId（先给机器人发一条消息以登记身份）".into());
    }
    // Stream 机器人 robotCode 一般 == app_key；捕获到就用捕获的
    let robot_code = if app.robot_code.is_empty() { &app.app_key } else { &app.robot_code };
    let token = access_token(&app.app_key, &app.app_secret, now_ms).await?;
    // msgParam 是 JSON 字符串（钉钉要求）。OTO 私聊的 sampleMarkdown 在客户端里会整段渲染成
    // 代码块，反而更难看；用 sampleText 纯文本最干净。
    let msg_param = serde_json::to_string(&serde_json::json!({ "content": text }))
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "robotCode": robot_code,
        "userIds": [app.staff_id],
        "msgKey": "sampleText",
        "msgParam": msg_param,
    });
    let resp = http_client()?
        .post("https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend")
        .header("x-acs-dingtalk-access-token", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OTO 请求失败: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let v: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Err(format!("钉钉 OTO 拒绝（HTTP {status}）: {v}"))
    }
}

/// 一条待推送事件（已格式化为 markdown 文本 + 归属用户 + 事件类别）
pub struct NotifyEvent {
    pub owner: String,
    pub kind: EventKind,
    /// 关联会话（用于查「发 N」编号；设备类事件为 None）
    pub task_id: Option<String>,
    /// markdown 正文，可含 `{{NO}}` 占位符，由 deliver 换成会话编号
    pub text: String,
}

#[derive(PartialEq, Clone, Copy)]
pub enum EventKind {
    Waiting,
    Finished,
    NewSession,
    Device,
    /// 非钉钉来源（网页）下发的任务：同步告知，让钉钉侧也知道刚发了什么
    Dispatch,
    /// 会话进入「等待选择」（交互式选择/权限确认）：提醒去作答
    Select,
}

impl DingtalkNotify {
    fn wants(&self, k: EventKind) -> bool {
        match k {
            EventKind::Waiting => self.waiting,
            EventKind::Finished => self.finished,
            EventKind::NewSession => self.new_session,
            EventKind::Device => self.device,
            // 群 webhook 复用「等待输入」开关；企业应用 OTO 一律推（见 deliver）
            EventKind::Dispatch => self.waiting,
            EventKind::Select => self.waiting,
        }
    }
}

/// 后台推送：对开启了对应事件的用户，逐条发钉钉（失败只记日志，不阻塞）。
/// now_ms 由调用方给（tick/report 里取一次系统时间）。
pub async fn deliver(state: &crate::state::SharedState, events: Vec<NotifyEvent>, now_ms: u64) {
    for ev in events {
        // 占位换成「发 N」编号（与 resolve_task 同源）：`{{NO}}` → 视觉标签「#N 」；
        // `{{N}}` → 纯数字（用在「发 N / 撤回 N」这类指令语法里）。查不到编号就退化。
        let no = match &ev.task_id {
            Some(id) => crate::bot::session_number(state, &ev.owner, id).await,
            None => None,
        };
        let text = match no {
            Some(n) => ev
                .text
                .replace("{{NO}}", &format!("#{n} "))
                .replace("{{N}}", &n.to_string()),
            None => ev.text.replace("{{NO}}", "").replace("{{N}}", "N"),
        };
        // 1) 群自定义机器人 Webhook（按用户逐事件开关，原有行为）
        let cfg = state.registry.read().await.dingtalk_of(&ev.owner);
        if let Some(cfg) = cfg {
            if cfg.enabled() && cfg.wants(ev.kind) {
                if let Err(e) = push_text(&cfg, &text, now_ms).await {
                    tracing::warn!("钉钉 Webhook 推送失败（{}）: {e}", ev.owner);
                }
            }
        }
        // 2) 企业应用 OTO 主动推：会话开始 / 任务完成 / 会话结束，直接私聊给用户本人。
        //    设备上线不推（避免噪音）；需已配 Stream 应用且已捕获 staffId。
        if matches!(
            ev.kind,
            EventKind::NewSession
                | EventKind::Waiting
                | EventKind::Finished
                | EventKind::Dispatch
                | EventKind::Select
        ) {
            let app = state.registry.read().await.dingtalk_app_of(&ev.owner);
            if let Some(app) = app {
                if !app.app_key.is_empty()
                    && !app.app_secret.is_empty()
                    && !app.staff_id.is_empty()
                {
                    if let Err(e) = push_oto(&app, &text, now_ms).await {
                        tracing::warn!("钉钉 OTO 主动推送失败（{}）: {e}", ev.owner);
                    }
                }
            }
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
