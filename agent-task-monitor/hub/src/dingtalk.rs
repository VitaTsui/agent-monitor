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
    static C: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>,
    > = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 控制面用：换 token、发 OTO 消息这类小 JSON 往返。
///
/// 原来是「整体 8s」一刀切。hub 在境外(Vultr)、钉钉在境内，这条链路会**间歇性卡住**，
/// 8s 的整体预算太紧。改成三档：连不上 8s 就认（对端不可达要快速失败），连上之后
/// 每次读最多等 15s，整体封顶 30s —— 卡死的连接照样识别得出来，只是不再把「慢一下」
/// 当成失败。
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .read_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

/// 取文件字节专用。
///
/// **绝不能与 `http_client` 共用同一个预算**：那是给几百字节的 JSON 定的，而这里要把一个
/// 任意大小的文件从钉钉的国内 CDN 拉到境外的 hub 上。线上抓到过（2026-09-09 hub 日志）：
/// 一份几 KB 的 .md 连着两次卡在 8.0s 整上超时（02:46:37→02:46:49、02:47:08→02:47:19，
/// 都是 3s 攒批窗口 + 8s 超时），第三次 0.5s 就下完了 —— 失败与文件大小无关，纯粹是这条
/// 跨境链路会卡。而超时的后果是 `bot::send_input` 提前 return：文件没下发，**任务正文也
/// 一起没下发**，用户只能把文件和指令整条重发（那次他重发了三遍）。
///
/// 整体给到 180s，靠 20s 的**每次读**超时来兜住真卡死的连接 —— 传输在推进就不该被砍断。
fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .read_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(180))
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
    if app.robot_code.is_empty() {
        &app.app_key
    } else {
        &app.robot_code
    }
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

/// 一次下载尝试的失败：能不能靠「换条新连接再来一次」救回来。
///
/// 这个区分是整个重试的前提。没有它，重试要么不敢做、要么对着「下载码已过期」空等
/// 一整轮超时 —— 后者比不重试还糟。
#[derive(Debug)]
enum DlFail {
    /// 链路问题（连不上 / 超时 / 传一半断了 / 对端 5xx）—— 换条连接就换掉了
    Retryable(String),
    /// 下载码过期、鉴权不过、响应结构不对 —— 换几条连接都一样，重试只是白等
    Fatal(String),
}

/// 网络层错误里哪些值得再来一次。
///
/// `is_timeout` / `is_connect` / `is_request` / `is_body` 说的都是「这条连接不行」；
/// 剩下的（builder 配错、响应不是 JSON）换多少条连接都还是那样。
fn classify_net(what: &str, e: reqwest::Error) -> DlFail {
    let msg = format!("{what}: {e}");
    if e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() {
        DlFail::Retryable(msg)
    } else {
        DlFail::Fatal(msg)
    }
}

/// HTTP 状态码定可重试性。
///
/// 5xx / 408 / 429 是「对端这会儿不行」，换条连接重来有意义；其余 4xx 是「这个请求本身
/// 不行」（下载码过期、token 失效、robotCode 对不上），重试只会把失败拖长一倍。
fn classify_status(what: &str, status: reqwest::StatusCode, body: &str) -> DlFail {
    // 错误体可能是一整页 CDN 的 XML/HTML，日志里截断就够定位了
    let body: String = body.chars().take(300).collect();
    let msg = format!("{what}（HTTP {status}）: {body}");
    if status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        DlFail::Retryable(msg)
    } else {
        DlFail::Fatal(msg)
    }
}

/// 「可重试就换新连接再来一次」的骨架。
///
/// 抽出来是为了能在没有网络的情况下测到重试本身：生产代码走的是同一段循环。
async fn retry_once<T, F, Fut>(what: &str, mut attempt: F) -> Result<T, String>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, DlFail>>,
{
    const ATTEMPTS: u32 = 2;
    let mut last = String::new();
    for i in 1..=ATTEMPTS {
        match attempt(i).await {
            Ok(v) => {
                if i > 1 {
                    tracing::info!("{what} 第 {i} 次成功（第 {} 次失败于：{last}）", i - 1);
                }
                return Ok(v);
            }
            Err(DlFail::Fatal(e)) => {
                tracing::warn!("{what} 第 {i}/{ATTEMPTS} 次失败，不重试（重试也是白等）：{e}");
                return Err(e);
            }
            Err(DlFail::Retryable(e)) => {
                tracing::warn!("{what} 第 {i}/{ATTEMPTS} 次失败（可重试）：{e}");
                last = e;
            }
        }
    }
    Err(format!("{what} 重试 {ATTEMPTS} 次仍失败：{last}"))
}

/// 一次完整的取文件尝试：换下载地址 → 拉字节。
async fn download_bot_file_once(
    app: &crate::registry::DingtalkApp,
    download_code: &str,
    token: &str,
) -> Result<Vec<u8>, DlFail> {
    let robot_code = robot_code_of(app);
    // **每次尝试都新建 client**：reqwest 的连接池挂在 client 上，复用同一个 client 就可能
    // 复用那条刚刚卡住的连接 —— 而那条连接正是要换掉的东西，不换等于重试了个寂寞。
    let cli = download_client().map_err(DlFail::Fatal)?;
    // 换地址与拉字节都走 download_client：这两步都是跨境到钉钉，正是会卡的那两步。
    let resp = cli
        .post("https://api.dingtalk.com/v1.0/robot/messageFiles/download")
        .header("x-acs-dingtalk-access-token", token)
        .json(&serde_json::json!({ "downloadCode": download_code, "robotCode": robot_code }))
        .send()
        .await
        .map_err(|e| classify_net("取下载地址失败", e))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(classify_status("取下载地址被拒", status, &body));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| classify_net("下载地址响应读取失败", e))?;
    let url = v
        .get("downloadUrl")
        .and_then(|u| u.as_str())
        .ok_or_else(|| DlFail::Fatal(format!("无 downloadUrl: {v}")))?;
    let resp = cli
        .get(url)
        .send()
        .await
        .map_err(|e| classify_net("下载文件失败", e))?;
    let status = resp.status();
    // 这一步以前**不查状态码**，直接 `.bytes()`：CDN 返回的 403/404 错误页会被原样当成
    // 文件内容落到用户目录里 —— 文件名没错、大小几百字节、打开是一段 XML。比下载失败更难查。
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(classify_status("下载文件被拒", status, &body));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| classify_net("下载文件中断", e))?
        .to_vec();
    Ok(bytes)
}

/// 下载机器人「收到的」文件内容（downloadCode → downloadUrl → 字节）。
///
/// 失败会**换一条新连接**重试一次。根因见 [`download_client`] 的注释：这条跨境链路是
/// **连接级**偶发卡死 —— 线上同一份几 KB 的 .md 连撞两次 8s 超时、第三次 0.5s 就下完。
/// 单纯抬高超时上限只会让失败来得更慢，换连接才是对症的。
///
/// 「换下载地址」那一步也一起重试：它同样是发往 api.dingtalk.com 的跨境请求、同样会卡，
/// 而它是纯读、幂等（拿一个临时 URL），重来没有副作用。所以两步作为一个整体重来，
/// 而不是只重试拉字节那一半。
///
/// token 留在重试之外只取一次：它有缓存、和链路抖动无关；真失效了会是 401，那是 Fatal。
pub async fn download_bot_file(
    app: &crate::registry::DingtalkApp,
    download_code: &str,
    now_ms: u64,
) -> Result<Vec<u8>, String> {
    let token = access_token(&app.app_key, &app.app_secret, now_ms).await?;
    // 先降成 &str 再进闭包：这样闭包捕获的全是「借自本函数」的引用，产出的 future 不牵扯
    // 闭包自身的借用，`FnMut(u32) -> Fut` 才推得动。
    let token: &str = &token;
    retry_once("钉钉文件下载", move |_| {
        download_bot_file_once(app, download_code, token)
    })
    .await
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
    let nick = me
        .get("nick")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

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
        if let Err(e) = oto_send(
            app,
            staff_id,
            &token,
            "sampleImageMsg",
            serde_json::json!({ "photoURL": url }),
        )
        .await
        {
            tracing::warn!("钉钉 OTO 图片发送失败: {e}");
        }
    }
    // 内容太长被截断：把完整内容作为文件补发（失败只记日志，不影响正文已送达）
    if let Some(full) = full {
        match upload_media(
            &token,
            "完整内容.txt",
            full.as_bytes(),
            "file",
            "text/plain",
        )
        .await
        {
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
            EventKind::NewSession | EventKind::Waiting | EventKind::Finished | EventKind::Select
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
            match push_oto(
                &app,
                &staff_id,
                &text,
                ev.full_content.as_deref(),
                now_ms,
                &images,
            )
            .await
            {
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
        assert!(
            !verify_app_sign(secret, "1700000000001", &good),
            "时间戳变了签名就不该过"
        );
    }

    #[test]
    fn urlencode_escapes_base64_chars() {
        // base64 里的 + / = 必须转义，否则拼进 URL 会被解析成别的意思
        assert_eq!(urlencode("a+b/c="), "a%2Bb%2Fc%3D");
    }

    /// 可重试的失败必须真的再来一次。
    ///
    /// 这是整个修复的要害：跨境链路是**连接级**偶发卡死（线上同一份 .md 连撞两次 8s
    /// 超时、第三次 0.5s 就下完），不重试就等于把一次抖动直接判成失败，而失败的代价是
    /// 连任务正文一起不下发。
    #[tokio::test]
    async fn retryable_failure_is_retried_with_new_attempt() {
        let calls = std::cell::Cell::new(0u32);
        let got = retry_once("测试", |i| {
            calls.set(calls.get() + 1);
            async move {
                if i == 1 {
                    Err(DlFail::Retryable("超时".into()))
                } else {
                    Ok(vec![1u8, 2, 3])
                }
            }
        })
        .await;
        assert_eq!(got, Ok(vec![1, 2, 3]), "第二次成功就该返回成功");
        assert_eq!(calls.get(), 2, "可重试的失败必须换新连接再来一次");
    }

    /// 两次都是可重试的失败 → 放弃，但错误信息要带上最后一次的原因（线上就靠它定位）。
    #[tokio::test]
    async fn retryable_failure_gives_up_after_two_attempts() {
        let calls = std::cell::Cell::new(0u32);
        let got: Result<(), String> = retry_once("测试", |_| {
            calls.set(calls.get() + 1);
            async { Err(DlFail::Retryable("连不上".into())) }
        })
        .await;
        assert_eq!(calls.get(), 2, "只重试一次，不能无限重试把人晾在那");
        assert!(
            got.unwrap_err().contains("连不上"),
            "错误里要留下最后一次的原因"
        );
    }

    /// 4xx 不重试 —— 下载码过期 / 鉴权不过重试多少次都是同一个结果，
    /// 白等一轮超时反而让用户多等一倍。
    #[tokio::test]
    async fn fatal_failure_is_not_retried() {
        let calls = std::cell::Cell::new(0u32);
        let got: Result<(), String> = retry_once("测试", |_| {
            calls.set(calls.get() + 1);
            async { Err(DlFail::Fatal("下载码已过期".into())) }
        })
        .await;
        assert_eq!(calls.get(), 1, "不可重试的失败必须立刻放弃");
        assert!(got.unwrap_err().contains("下载码已过期"));
    }

    /// 状态码分类：生产代码用的就是这个函数，4xx / 5xx 分得清才谈得上「只重试该重试的」。
    #[test]
    fn status_decides_retryability() {
        use reqwest::StatusCode;
        let retryable = |s| matches!(classify_status("x", s, "body"), DlFail::Retryable(_));
        // 对端这会儿不行 → 换条连接重来有意义
        assert!(retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(retryable(StatusCode::BAD_GATEWAY));
        assert!(retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(
            retryable(StatusCode::REQUEST_TIMEOUT),
            "408 就是超时，正是要重试的那种"
        );
        assert!(
            retryable(StatusCode::TOO_MANY_REQUESTS),
            "429 限流，退一步再来"
        );
        // 请求本身不行 → 重试只是白等
        assert!(
            !retryable(StatusCode::UNAUTHORIZED),
            "401 token 不对，重试没用"
        );
        assert!(
            !retryable(StatusCode::FORBIDDEN),
            "403 下载码过期，重试没用"
        );
        assert!(!retryable(StatusCode::NOT_FOUND));
        assert!(!retryable(StatusCode::BAD_REQUEST));
    }
}
