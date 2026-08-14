//! 钉钉主动推送：会话状态变化 → 私聊推给账号本人。
//! 走企业应用 OTO（Stream 长连接），机器人由用户在前台自助配置：
//! 谁配的机器人就服务谁，收到的消息也归他。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

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

/// markdown 标题：钉钉 markdown 消息必须带 title（会话列表/通知里显示的就是它）。
fn md_title(text: &str) -> String {
    crate::mdfmt::derive_title(text, "终端通知", 24)
}

// ---------- 企业应用 OTO 主动推送 ----------

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
async fn upload_media(
    token: &str,
    filename: &str,
    content: &[u8],
    kind: &str,
    mime: &str,
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(content.to_vec())
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new().part("media", part);
    let url = format!("https://oapi.dingtalk.com/media/upload?access_token={token}&type={kind}");
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

// ---------- 扫码绑定：钉钉扫码授权 → 认出扫码的是谁 ----------

/// 扫码授权页地址。二维码里放的就是它 —— 用钉钉扫一下，授权后回调到 hub。
///
/// 走钉钉标准 OAuth：扫码人在钉钉里确认授权，我们才拿得到「他是谁」。
/// 这是扫码绑定与「扫出一段文字再手动发」的根本差别。
pub fn qr_auth_url(app_key: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "https://login.dingtalk.com/oauth2/auth?redirect_uri={}&response_type=code\
         &client_id={}&scope=openid&state={}&prompt=consent",
        urlencode(redirect_uri),
        urlencode(app_key),
        urlencode(state),
    )
}

/// 用授权码换出「扫码人是谁」——返回其企业内 userId（即消息里的 senderStaffId）与昵称。
///
/// 分三步，缺一不可：
/// 1. code → 用户级 access_token
/// 2. 该 token → 用户的 unionId（此接口只给 unionId/openId，拿不到企业 userId）
/// 3. unionId + **企业级** token → userId
///
/// 第 3 步常被忽略：机器人消息里的 senderStaffId 是企业 userId，与 unionId 不是一回事，
/// 不换这一步就会绑上一个永远匹配不到来信的 id。
pub async fn resolve_scan_user(
    app_key: &str,
    app_secret: &str,
    code: &str,
    now_ms: u64,
) -> Result<(String, String), String> {
    let cli = http_client()?;
    // 1) 授权码 → 用户 token
    let v: serde_json::Value = cli
        .post("https://api.dingtalk.com/v1.0/oauth2/userAccessToken")
        .json(&serde_json::json!({
            "clientId": app_key,
            "clientSecret": app_secret,
            "code": code,
            "grantType": "authorization_code",
        }))
        .send()
        .await
        .map_err(|e| format!("换取用户令牌失败: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let user_token = v
        .get("accessToken")
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("换取用户令牌失败: {v}"))?;

    // 2) 用户 token → unionId / 昵称
    let me: serde_json::Value = cli
        .get("https://api.dingtalk.com/v1.0/contact/users/me")
        .header("x-acs-dingtalk-access-token", user_token)
        .send()
        .await
        .map_err(|e| format!("获取用户信息失败: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let union_id = me
        .get("unionId")
        .and_then(|u| u.as_str())
        .ok_or_else(|| format!("获取用户信息失败: {me}"))?;
    let nick = me.get("nick").and_then(|n| n.as_str()).unwrap_or("").to_string();

    // 3) unionId → 企业 userId（= senderStaffId）
    let corp_token = access_token(app_key, app_secret, now_ms).await?;
    let uv: serde_json::Value = cli
        .post(format!(
            "https://oapi.dingtalk.com/topapi/user/getbyunionid?access_token={corp_token}"
        ))
        .json(&serde_json::json!({ "unionid": union_id }))
        .send()
        .await
        .map_err(|e| format!("换取企业 userId 失败: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let user_id = uv
        .pointer("/result/userid")
        .and_then(|u| u.as_str())
        .ok_or_else(|| format!("换取企业 userId 失败（扫码人可能不在该企业内）: {uv}"))?;
    Ok((user_id.to_string(), nick))
}

/// 通过企业应用机器人 OTO 接口，主动把一条文本发给某个用户（staffId）。
/// full 非空且比正文长时，额外把完整内容作为 .txt 文件发在下面（正文被截断的兜底）。
pub async fn push_oto(
    app: &crate::registry::DingtalkApp,
    staff_id: &str,
    text: &str,
    full: Option<&str>,
    now_ms: u64,
    images: &[String],
) -> Result<(), String> {
    if staff_id.is_empty() {
        return Err("空 staffId".into());
    }
    let token = access_token(&app.app_key, &app.app_secret, now_ms).await?;
    // 私聊是「远程继续会话」的主通道，同样要先降级 + 分片：agent 的结果里表格和代码围栏
    // 最多，手机端渲染不了。完整原文仍由下面的 .txt 附件兜底，降级只影响正文可读性。
    let md = crate::mdfmt::downgrade_for_dingtalk(text);
    let chunks = crate::mdfmt::chunk_text(&md, crate::mdfmt::DINGTALK_MAX_LEN);
    // msgParam 是 JSON 字符串（钉钉要求）；sampleMarkdown 让结果里的 md 正常渲染
    // （手机端正常；桌面端 OTO 可能显示成代码块，属客户端差异）。
    for chunk in &chunks {
        oto_send(
            app,
            staff_id,
            &token,
            "sampleMarkdown",
            serde_json::json!({ "title": md_title(chunk), "text": chunk }),
        )
        .await?;
    }
    // 正文里引用的本地截图：正文已把标记换成 `[图: alt]`，图在这里作为真正的图片消息补上。
    //
    // **必须走 `photoURL` + 公网 URL**：实测 `mediaId` 会发出去但显示破损（钉钉接受了、
    // 渲染不了），只有公网 URL 能内联显示。所以图片经 hub 的一次性外链给出去，
    // 由钉钉服务器来拉一次（见 server::stash_pub_image）。
    for url in images {
        if let Err(e) = oto_send(app, staff_id, &token, "sampleImageMsg",
                                 serde_json::json!({ "photoURL": url })).await {
            tracing::warn!("钉钉 OTO 图片发送失败: {e}");
        }
    }
    // 内容太长被截断：把完整内容作为文件补发（失败只记日志，不影响正文已送达）
    if let Some(full) = full {
        match upload_media(&token, "完整内容.txt", full.as_bytes(), "file", "text/plain").await {
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
#[derive(Clone)]
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

/// 后台推送：把会话事件私聊推给账号本人（失败只记日志，不阻塞）。
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
        // 企业应用 OTO 主动推：会话开始 / 任务完成 / 会话结束，直接私聊给用户本人。
        // 设备上线不推（避免噪音）；需该账号已配好自己的应用、且捕获过对面的 staffId
        //（用户给机器人发过一句话就有了）。
        //
        // 只认「本人的应用 → 本人的钉钉号」这一条通路：机器人是谁配的就服务谁，
        // 不再有全局群 webhook，也不再按 staffId 去查它归属哪个账号。
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
            // 用哪个机器人、发给谁：自己的优先，没配才回退到管理员的全局机器人
            //（后者需要该账号绑过钉钉号，否则认不出该发给谁）
            let target = state.registry.read().await.dingtalk_push_target(&ev.owner);
            let Some((app, staff_id)) = target else {
                tracing::warn!(
                    "钉钉推送跳过：账号既没配自己的机器人，也没绑钉钉号（{}）",
                    ev.owner
                );
                continue;
            };
            if app.app_secret.is_empty() {
                continue;
            }
            // 正文里引用的本地截图：向会话所在机器现取，再挂成一次性外链交给钉钉去拉。
            // 取不到就算了 —— 正文里已经有 `[图: xxx]` 占位，不该为一张图卡住整条推送。
            let (text, refs) = crate::mdfmt::take_local_images(&text);
            let mut images: Vec<String> = Vec::new();
            if let Some(task_id) = ev.task_id.as_deref() {
                for (_, rel) in &refs {
                    if let Some((mime, bytes)) =
                        crate::server::fetch_session_file(state, &ev.owner, task_id, rel).await
                    {
                        if mime.starts_with("image/") {
                            images.push(crate::server::stash_pub_image(state, bytes, &mime).await);
                        }
                    }
                }
            }
            match push_oto(&app, &staff_id, &text, ev.full_content.as_deref(), now_ms, &images).await {
                Ok(_) => tracing::info!("钉钉已推送 kind={kind}（{}）", ev.owner),
                Err(e) => tracing::warn!("钉钉推送失败 kind={kind}（{}）: {e}", ev.owner),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回调验签是钉钉侧唯一的身份凭据：签错就该拒，不能放行。
    #[test]
    fn app_sign_verifies_and_rejects() {
        // 用同一套算法算出期望签名，验证 verify_app_sign 认它
        let secret = "mysecret";
        let ts = "1700000000000";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{ts}\n{secret}").as_bytes());
        let good = B64.encode(mac.finalize().into_bytes());
        assert!(verify_app_sign(secret, ts, &good), "正确签名应通过");
        assert!(!verify_app_sign(secret, ts, "bogus"), "错误签名必须拒绝");
        assert!(!verify_app_sign(secret, "1700000000001", &good), "时间戳变了签名就不该过");
    }

    #[test]
    fn urlencode_escapes_base64_chars() {
        // base64 里的 + / = 必须转义，否则拼进 URL 会被解析成别的意思
        assert_eq!(urlencode("a+b/c="), "a%2Bb%2Fc%3D");
    }
}
