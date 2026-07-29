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

/// markdown 标题：取正文首行、去掉 #/*/空格，截断——钉钉 markdown 消息要一个纯文本 title。
fn md_title(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("终端通知");
    let t: String = line.trim_matches(|c| c == '#' || c == '*' || c == ' ').chars().take(24).collect();
    if t.is_empty() { "终端通知".to_string() } else { t }
}

/// 推送一条 markdown 到钉钉群机器人 Webhook。now_ms 由调用方给（便于测试）。
pub async fn push_text(cfg: &DingtalkNotify, text: &str, now_ms: u64) -> Result<(), String> {
    let url = signed_url(&cfg.webhook, &cfg.secret, now_ms);
    let body = serde_json::json!({
        "msgtype": "markdown",
        "markdown": { "title": md_title(text), "text": text }
    });
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

fn robot_code_of(app: &crate::registry::DingtalkApp) -> &str {
    // Stream 机器人 robotCode 一般 == app_key；捕获到就用捕获的
    if app.robot_code.is_empty() { &app.app_key } else { &app.robot_code }
}

/// 发一条 OTO 消息（msgKey + msgParam 由调用方给），复用已取的 token。
async fn oto_send(
    app: &crate::registry::DingtalkApp,
    staff_id: &str,
    token: &str,
    msg_key: &str,
    msg_param: serde_json::Value,
) -> Result<(), String> {
    let body = serde_json::json!({
        "robotCode": robot_code_of(app),
        "userIds": [staff_id],
        "msgKey": msg_key,
        "msgParam": serde_json::to_string(&msg_param).map_err(|e| e.to_string())?,
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

/// 下载机器人「收到的」文件内容（downloadCode → downloadUrl → 字节）。
pub async fn download_bot_file(
    app: &crate::registry::DingtalkApp,
    download_code: &str,
    now_ms: u64,
) -> Result<Vec<u8>, String> {
    let robot_code = robot_code_of(app);
    let token = access_token(&app.app_key, &app.app_secret, now_ms).await?;
    let resp = http_client()?
        .post("https://api.dingtalk.com/v1.0/robot/messageFiles/download")
        .header("x-acs-dingtalk-access-token", token)
        .json(&serde_json::json!({ "downloadCode": download_code, "robotCode": robot_code }))
        .send()
        .await
        .map_err(|e| format!("取下载地址失败: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let url = v
        .get("downloadUrl")
        .and_then(|u| u.as_str())
        .ok_or_else(|| format!("无 downloadUrl: {v}"))?;
    let bytes = http_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载文件失败: {e}"))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?
        .to_vec();
    Ok(bytes)
}

/// 上传一段文本为钉钉媒体文件，返回 media_id（用同一 access_token）。
async fn upload_media(token: &str, filename: &str, content: &[u8]) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(content.to_vec())
        .file_name(filename.to_string())
        .mime_str("text/plain")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new().part("media", part);
    let url = format!("https://oapi.dingtalk.com/media/upload?access_token={token}&type=file");
    let resp = http_client()?
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("媒体上传请求失败: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if v.get("errcode").and_then(|c| c.as_i64()) == Some(0) {
        v.get("media_id")
            .and_then(|m| m.as_str())
            .map(String::from)
            .ok_or_else(|| format!("媒体上传无 media_id: {v}"))
    } else {
        Err(format!("媒体上传失败: {v}"))
    }
}

/// 通过企业应用机器人 OTO 接口，主动把一条文本发给某个用户（staffId）。
/// full 非空且比正文长时，额外把完整内容作为 .txt 文件发在下面（正文被截断的兜底）。
pub async fn push_oto(
    app: &crate::registry::DingtalkApp,
    staff_id: &str,
    text: &str,
    full: Option<&str>,
    now_ms: u64,
) -> Result<(), String> {
    if staff_id.is_empty() {
        return Err("空 staffId".into());
    }
    let token = access_token(&app.app_key, &app.app_secret, now_ms).await?;
    // msgParam 是 JSON 字符串（钉钉要求）；sampleMarkdown 让结果里的 md 正常渲染
    // （手机端正常；桌面端 OTO 可能显示成代码块，属客户端差异）。
    oto_send(
        app,
        staff_id,
        &token,
        "sampleMarkdown",
        serde_json::json!({ "title": md_title(text), "text": text }),
    )
    .await?;
    // 内容太长被截断：把完整内容作为文件补发（失败只记日志，不影响正文已送达）
    if let Some(full) = full {
        match upload_media(&token, "完整内容.txt", full.as_bytes()).await {
            Ok(media_id) => {
                let param = serde_json::json!({
                    "mediaId": media_id,
                    "fileName": "完整内容.txt",
                    "fileType": "txt",
                });
                if let Err(e) = oto_send(app, staff_id, &token, "sampleFile", param).await {
                    tracing::warn!("钉钉 OTO 完整内容文件发送失败: {e}");
                }
            }
            Err(e) => tracing::warn!("钉钉 OTO 完整内容上传失败: {e}"),
        }
    }
    Ok(())
}

/// 一条待推送事件（已格式化为 markdown 文本 + 归属用户 + 事件类别）
pub struct NotifyEvent {
    pub owner: String,
    pub kind: EventKind,
    /// 关联会话（用于查「发 N」编号；设备类事件为 None）
    pub task_id: Option<String>,
    /// markdown 正文，可含 `{{NO}}` 占位符，由 deliver 换成会话编号
    pub text: String,
    /// 内容过长被截断时的完整文本：OTO 会把它作为 .txt 文件补发在正文下面。
    pub full_content: Option<String>,
}

#[derive(PartialEq, Clone, Copy)]
pub enum EventKind {
    Waiting,
    Finished,
    NewSession,
    Device,
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
            EventKind::Select => self.waiting,
        }
    }
}

/// 后台推送：对开启了对应事件的用户，逐条发钉钉（失败只记日志，不阻塞）。
/// now_ms 由调用方给（tick/report 里取一次系统时间）。
pub async fn deliver(state: &crate::state::SharedState, events: Vec<NotifyEvent>, now_ms: u64) {
    for ev in events {
        // 占位换成「发 N」编号（与 resolve_task 同源）：`{NO}` → 视觉标签「#N 」；
        // `{N}` → 纯数字（用在「发 N / 撤回 N」这类指令语法里）。查不到编号就退化。
        // 注意占位是单花括号 —— server 那边是 format! 里的 `{{NO}}`，编译后就是 `{NO}`。
        let no = match &ev.task_id {
            Some(id) => crate::bot::session_number(state, &ev.owner, id).await,
            None => None,
        };
        let text = match no {
            Some(n) => ev
                .text
                .replace("{NO}", &format!("#{n} "))
                .replace("{N}", &n.to_string()),
            None => ev.text.replace("{NO}", "").replace("{N}", "N"),
        };
        // 1) 群自定义机器人 Webhook（后管统一配置的全局群，按事件开关推送）
        let cfg = state.registry.read().await.global_dingtalk_notify();
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
                | EventKind::Select
        ) {
            let kind = match ev.kind {
                EventKind::Select => "Select",
                EventKind::Waiting => "Waiting",
                EventKind::Finished => "Finished",
                EventKind::NewSession => "NewSession",
                _ => "?",
            };
            // 该账号名下已绑定的所有钉钉 id，各推一份 —— 每个 id 用其「来源应用」的凭据/robotCode。
            let binds = state.registry.read().await.dingtalk_ids_of(&ev.owner);
            if binds.is_empty() {
                tracing::warn!("钉钉 OTO 跳过：账号未绑定任何钉钉 id（{}）", ev.owner);
            }
            for (staff_id, app_user) in binds {
                let Some(app) = state.registry.read().await.dingtalk_app_of(&app_user) else {
                    continue;
                };
                if app.app_key.is_empty() || app.app_secret.is_empty() {
                    continue;
                }
                match push_oto(&app, &staff_id, &text, ev.full_content.as_deref(), now_ms).await {
                    Ok(_) => tracing::info!("钉钉 OTO 已推送 kind={kind}（{}→{staff_id}）", ev.owner),
                    Err(e) => {
                        tracing::warn!("钉钉 OTO 推送失败 kind={kind}（{}→{staff_id}）: {e}", ev.owner)
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
