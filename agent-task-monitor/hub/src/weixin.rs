//! 微信（个人号）机器人接入 —— 腾讯官方 iLink Bot API。
//!
//! 与钉钉 Stream 同构：hub 主动连出去，**不需要公网回调**。区别在两点：
//!
//! ① **发消息必须带 `context_token`**，而它只来自用户发来的消息 —— 协议本身是
//!    request-response 的，没有 open_id 之类可直接寻址的发送方式。实测这个 token
//!    可长期复用（1.8 小时后仍能发出），所以把最近一次收到的存进注册表，
//!    任务完成时就能主动推送。用户绑定后需要给 bot 发一句话来激活。
//! ② **收不到普通微信群的消息**（iLink bot 身份的限制），只服务私聊 —— 这与本项目
//!    「私聊遥控」的用法一致。
//!
//! 踩过的坑，改这个文件前务必看一眼：
//! - **`AuthorizationType: ilink_bot_token` 头是必需的**。少了它服务端一律回
//!   `-14 session timeout`，看起来像会话过期，实际是它不认这个 Authorization 的类型。
//!   排查时会一路怀疑扫码流程，非常费时间。
//! - 所有 POST 都要带 `base_info: {channel_version}`。
//! - 错误码在 **`errcode`** 字段（不是 `ret`）；收到的消息在 **`msgs`**（不是 `updates`）。

use serde_json::{json, Value};

const BASE: &str = "https://ilinkai.weixin.qq.com/ilink/bot";
const CHANNEL_VERSION: &str = "1.0.2";
/// 长轮询服务端大约挂 35s，客户端超时留足余量
const POLL_TIMEOUT_SECS: u64 = 60;
const CALL_TIMEOUT_SECS: u64 = 20;

/// 会话过期：需要用户重新扫码
pub const ERR_SESSION_TIMEOUT: i64 = -14;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(POLL_TIMEOUT_SECS + 10))
        .build()
        .unwrap_or_default()
}

/// `X-WECHAT-UIN`：随机 uint32 → 十进制字符串 → base64。每个请求换一个（防重放）。
fn uin() -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let n: u32 = rand::random();
    B64.encode(n.to_string())
}

async fn post(path: &str, mut body: Value, token: Option<&str>, timeout: u64) -> Result<Value, String> {
    if let Some(o) = body.as_object_mut() {
        o.entry("base_info")
            .or_insert_with(|| json!({ "channel_version": CHANNEL_VERSION }));
    }
    let mut req = client()
        .post(format!("{BASE}/{path}"))
        .timeout(std::time::Duration::from_secs(timeout))
        .header("Content-Type", "application/json")
        // ↓ 这一行少了就是满屏 -14，别删
        .header("AuthorizationType", "ilink_bot_token")
        .header("X-WECHAT-UIN", uin());
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", text.chars().take(200).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON: {e}"))
}

async fn get(path: &str) -> Result<Value, String> {
    let resp = client()
        .get(format!("{BASE}/{path}"))
        .timeout(std::time::Duration::from_secs(CALL_TIMEOUT_SECS))
        .header("Content-Type", "application/json")
        .header("AuthorizationType", "ilink_bot_token")
        .header("X-WECHAT-UIN", uin())
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let text = resp.text().await.unwrap_or_default();
    serde_json::from_str(&text).map_err(|e| format!("响应不是 JSON: {e}"))
}

/// 响应里的错误码（`errcode` 优先，回退 `ret`）。0/缺省视为成功。
fn errcode(v: &Value) -> i64 {
    v.get("errcode")
        .and_then(Value::as_i64)
        .or_else(|| v.get("ret").and_then(Value::as_i64))
        .unwrap_or(0)
}

// ───────────────────────────── 扫码登录 ─────────────────────────────

pub struct Qrcode {
    /// 轮询状态用的 id
    pub id: String,
    /// 要编码进二维码的链接（前端拿它渲染）
    pub link: String,
}

pub async fn fetch_qrcode() -> Result<Qrcode, String> {
    let v = get("get_bot_qrcode?bot_type=3").await?;
    let id = v.get("qrcode").and_then(Value::as_str).unwrap_or_default().to_string();
    let link = v
        .get("qrcode_img_content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() || link.is_empty() {
        return Err(format!("二维码响应异常: {}", v));
    }
    Ok(Qrcode { id, link })
}

pub enum ScanState {
    Waiting,
    Expired,
    /// 已确认，带回凭据
    Confirmed(crate::registry::WeixinBot),
}

/// 轮询扫码状态。**只有 `confirmed` 才算数** —— `scanned` 阶段也会给出 bot_token，
/// 拿了就用会一路 -14（这是实测踩过的坑）。
pub async fn scan_state(qrcode_id: &str) -> Result<ScanState, String> {
    let v = get(&format!("get_qrcode_status?qrcode={qrcode_id}")).await?;
    match v.get("status").and_then(Value::as_str).unwrap_or("") {
        "confirmed" => Ok(ScanState::Confirmed(crate::registry::WeixinBot {
            bot_token: v.get("bot_token").and_then(Value::as_str).unwrap_or_default().into(),
            ilink_user_id: v.get("ilink_user_id").and_then(Value::as_str).unwrap_or_default().into(),
            ilink_bot_id: v.get("ilink_bot_id").and_then(Value::as_str).unwrap_or_default().into(),
            context_token: String::new(),
            bound_at: crate::state::now_secs(),
            session_expired: false,
        })),
        "expired" => Ok(ScanState::Expired),
        _ => Ok(ScanState::Waiting),
    }
}

// ───────────────────────────── 收发消息 ─────────────────────────────

pub struct Incoming {
    pub text: String,
    pub from_user_id: String,
    pub context_token: String,
    /// 随消息带来的图片（可多张）。内容是加密的，取回要走 [`fetch_image`]。
    pub images: Vec<ImageRef>,
    /// 消息在服务端生成的时刻（毫秒）。**图那条通常晚于文字**（要先把图传上 CDN），
    /// 「图文同发却把图落下」的根因就在这个差值上，排查时靠它看清先后。
    pub create_time_ms: u64,
}

/// 一张待取的图片：CDN 直链 + 解密密钥。
pub struct ImageRef {
    pub url: String,
    /// 32 位十六进制字符串，解码成 16 字节就是 AES-128 的密钥
    pub aeskey: String,
    /// 服务端声明的明文长度，用来核对解密结果
    pub size: usize,
}

/// 单张图片上限。图片整份进内存（见 `BotPendingFile::bytes`），不设限的话
/// 连发几张大图就能把 hub 撑爆。
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// 收到文字后，等「一起发出的图片」跟上的窗口。
///
/// 微信里图和文字是两条独立消息，且图那条**生成得更晚**（要先把图传上 CDN），
/// 所以到达顺序常常是文字在前、图在后 —— 不等就会把图落下，只能等下一条任务才带走。
/// 代价是纯文字指令也会晚这么久才执行；钉钉那边内容类消息本来就攒 3s，2s 在可接受的量级。
/// 图一到就立刻往下走，不会白等满。
const IMAGE_GRACE_MS: u64 = 2_000;

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// 取一张图片并解密。
///
/// **AES-128-ECB，密钥 = `aeskey` 十六进制解码后的 16 字节，PKCS#7 补齐** ——
/// 这是拿真实图片试出来的：密文 53616 字节、声明明文 53605，差 11 正好是补齐位数；
/// 解出来 PNG 从文件头到 IEND 走完 10 个 chunk 且长度与声明分毫不差。
/// （别改成 CBC：零 IV 的 CBC 首块也能解出 PNG 文件头，看着像对的，其实后面全是花的。）
pub async fn fetch_image(img: &ImageRef) -> Result<Vec<u8>, String> {
    let resp = client()
        .get(&img.url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }
    let data = resp.bytes().await.map_err(|e| format!("下载失败: {e}"))?.to_vec();
    if data.len() > MAX_IMAGE_BYTES {
        return Err(format!("图片太大（{} MB），暂不接收", data.len() / 1024 / 1024));
    }

    decrypt_image(data, &img.aeskey, img.size)
}

/// 解密 + 去补齐。与 [`fetch_image`] 分开是为了能脱网测 —— 这段是整条链路里最容易
/// 写错又最难事后发现的：改错了 CBC/ECB，首块照样能解出正确的文件头，看着像对的。
fn decrypt_image(mut data: Vec<u8>, aeskey: &str, size: usize) -> Result<Vec<u8>, String> {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};

    let key = unhex(aeskey).filter(|k| k.len() == 16).ok_or("aeskey 不是 16 字节十六进制")?;
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(format!("密文长度异常: {}", data.len()));
    }
    let cipher = aes::Aes128::new(GenericArray::from_slice(&key));
    for block in data.chunks_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    // 去 PKCS#7 补齐
    let pad = *data.last().unwrap_or(&0) as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return Err("解密结果补齐位异常（密钥不对？）".into());
    }
    data.truncate(data.len() - pad);
    // 与服务端声明的长度对不上，说明解出来的不是原图，别往下传 ——
    // 这一条正是「看着像对的」那类错误的兜底
    if size > 0 && data.len() != size {
        return Err(format!("解密后长度 {} 与声明 {} 不符", data.len(), size));
    }
    Ok(data)
}

/// 按魔数猜扩展名 —— 消息里不带文件名，落盘总得有个像样的名字
pub(crate) fn image_ext(b: &[u8]) -> &'static str {
    match b {
        _ if b.starts_with(b"\x89PNG") => "png",
        _ if b.starts_with(&[0xff, 0xd8, 0xff]) => "jpg",
        _ if b.starts_with(b"GIF8") => "gif",
        _ if b.starts_with(b"RIFF") => "webp",
        _ => "bin",
    }
}

/// 一轮长轮询。返回 (本轮消息, 新游标)。
pub async fn get_updates(token: &str, buf: &str) -> Result<(Vec<Incoming>, String), String> {
    let v = post("getupdates", json!({ "get_updates_buf": buf }), Some(token), POLL_TIMEOUT_SECS).await?;
    let code = errcode(&v);
    if code != 0 {
        return Err(format!("errcode={code} {}", v.get("errmsg").and_then(Value::as_str).unwrap_or("")));
    }
    let next = v.get("get_updates_buf").and_then(Value::as_str).unwrap_or(buf).to_string();
    let mut out = Vec::new();
    for m in v.get("msgs").and_then(Value::as_array).cloned().unwrap_or_default() {
        // 跳过 bot 自己发的：message_type=2 是 BOT，或 from 以 @im.bot 结尾。
        // 不跳会自问自答（把自己的回复当成新指令再跑一遍）。
        if m.get("message_type").and_then(Value::as_i64) == Some(2) {
            continue;
        }
        let from = m.get("from_user_id").and_then(Value::as_str).unwrap_or_default();
        if from.ends_with("@im.bot") {
            continue;
        }
        let ct = m.get("context_token").and_then(Value::as_str).unwrap_or_default();
        if ct.is_empty() {
            continue;
        }
        // 只打字段名不打内容（内容是用户私聊原文）。留着是为了将来能按时间戳过滤
        // 陈旧消息 —— 目前只能靠「首轮全丢」，知道时间字段叫什么才能做得更准。
        if let Some(o) = m.as_object() {
            tracing::debug!("微信消息字段: {:?}", o.keys().collect::<Vec<_>>());
        }
        let created = m.get("create_time_ms").and_then(Value::as_u64).unwrap_or(0);
        // 内容散在 item_list 里：type=1 文本、type=2 图片，一条消息里可能都有
        let mut text = String::new();
        let mut images = Vec::new();
        for it in m.get("item_list").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            match it.get("type").and_then(Value::as_i64) {
                Some(1) => {
                    if let Some(t) = it.pointer("/text_item/text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
                Some(2) => {
                    let url = it
                        .pointer("/image_item/media/full_url")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let aeskey =
                        it.pointer("/image_item/aeskey").and_then(Value::as_str).unwrap_or_default();
                    if !url.is_empty() && !aeskey.is_empty() {
                        images.push(ImageRef {
                            url: url.to_string(),
                            aeskey: aeskey.to_string(),
                            size: it
                                .pointer("/image_item/hd_size")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as usize,
                        });
                    }
                }
                _ => {}
            }
        }
        if text.trim().is_empty() && images.is_empty() {
            // 语音等还不支持的类型。**不能静默丢**：发的人只会看到石沉大海，
            // 完全不知道是没收到、还是不支持（实际发生过）。带一个空消息出去，
            // 由 handle_message 回一句说明。结构打进 debug 便于以后认字段。
            tracing::debug!(
                "微信非文本消息 item_list: {}",
                m.get("item_list").map(|v| v.to_string()).unwrap_or_default()
            );
            out.push(Incoming {
                text: String::new(),
                from_user_id: from.to_string(),
                context_token: ct.to_string(),
                images: Vec::new(),
                create_time_ms: created,
            });
            continue;
        }
        out.push(Incoming {
            text,
            from_user_id: from.to_string(),
            context_token: ct.to_string(),
            images,
            create_time_ms: created,
        });
    }
    Ok((out, next))
}

/// 发一条文本。`context_token` 必须来自收到过的消息（可复用）。
pub async fn send_text(
    token: &str,
    to_user_id: &str,
    context_token: &str,
    text: &str,
) -> Result<(), String> {
    if context_token.is_empty() {
        return Err("没有 context_token（用户还没给 bot 发过消息）".into());
    }
    let body = json!({ "msg": {
        "to_user_id": to_user_id,
        "client_id": format!("am-{}", uuid::Uuid::new_v4()),
        "message_type": 2,   // BOT
        "message_state": 2,  // FINISH
        "context_token": context_token,
        "item_list": [{ "type": 1, "text_item": { "text": text } }],
    }});
    let v = post("sendmessage", body, Some(token), CALL_TIMEOUT_SECS).await?;
    let code = errcode(&v);
    if code != 0 {
        return Err(format!("errcode={code} {}", v.get("errmsg").and_then(Value::as_str).unwrap_or("")));
    }
    Ok(())
}

// ───────────────────────────── 长轮询循环 ─────────────────────────────

/// 长轮询管理器：为每个绑了微信的账号维持一条收消息循环。形状与
/// `dingtalk_stream::run` 一致（凭据变了就重起、账号没了就停）。
pub async fn run(state: crate::state::SharedState) {
    // user -> (bot_token, 任务句柄)
    let mut running: std::collections::HashMap<String, (String, tokio::task::JoinHandle<()>)> =
        std::collections::HashMap::new();
    loop {
        let desired: std::collections::HashMap<String, String> = state
            .registry
            .read()
            .await
            .weixin_users()
            .into_iter()
            // 已判过期的不再轮询：token 废了，接着打只会一秒一个 -14 刷满日志，
            // 等用户重新扫码（会走 weixin_reload 叫醒）。
            .filter(|(_, b)| !b.session_expired && !b.bot_token.is_empty())
            .map(|(u, b)| (u, b.bot_token))
            .collect();

        running.retain(|user, (tok, handle)| match desired.get(user) {
            Some(t) if t == tok && !handle.is_finished() => true,
            _ => {
                handle.abort();
                false
            }
        });

        for (user, tok) in &desired {
            if !running.contains_key(user) {
                let (st, u2, t2) = (state.clone(), user.clone(), tok.clone());
                let handle = tokio::spawn(async move { supervise(st, u2, t2).await });
                running.insert(user.clone(), (tok.clone(), handle));
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            _ = state.weixin_reload.notified() => {}
        }
    }
}

/// 单账号收消息循环：一轮长轮询 → 处理消息 → 再来一轮。出错退避重试（3s→最长 60s）。
async fn supervise(state: crate::state::SharedState, user: String, token: String) {
    let mut buf = String::new();
    let mut backoff = 3u64;
    // **首轮只取游标、丢掉消息**。空游标会让服务端把这个会话的历史消息全量回放，
    // 照单处理的后果不只是刷屏——每条老消息都会被当成新指令**重新执行一遍**
    //（`@1 重启服务` 这种重放出去是会出事的）。绑定后第一次连、以及 hub 每次重启
    // 都会走到这里，所以必须丢。
    //
    // 代价是 hub 停机期间发来的消息收不到。这是有意选的：宁可漏掉一条要你重发，
    // 也不能把半天前的旧指令翻出来执行。
    let mut priming = true;

    // 收消息与处理消息分家：长轮询只管把消息塞进队列，处理由这个**串行**任务负责。
    // 两个目的一次满足 —— ① 不阻塞长轮询（dispatch 里有等客户端取走输入的 5s 级等待，
    // 顺着做会让那期间的消息全压在服务端）；② 消息严格按到达顺序处理，
    // 「先发图、再发『看看这张图』」才能把图带上。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Incoming>();
    {
        let (st, u2, t2) = (state.clone(), user.clone(), token.clone());
        tokio::spawn(async move {
            let mut queue: std::collections::VecDeque<Incoming> = Default::default();
            loop {
                let m = match queue.pop_front() {
                    Some(m) => m,
                    None => match rx.recv().await {
                        Some(m) => m,
                        None => break, // 发端没了（会话过期/凭据变更），收工
                    },
                };
                let kind = if !m.images.is_empty() { "图片" } else { "文字" };
                tracing::info!(
                    "微信取到消息 类型={kind} 生成于={} user={u2}",
                    m.create_time_ms
                );
                // 图片、以及不支持的类型：没什么可等的，立刻处理
                if !m.images.is_empty() || m.text.trim().is_empty() {
                    handle_message(&st, &u2, &t2, m).await;
                    continue;
                }
                // 纯文字：先等一小会儿。**图和文字是两条独立消息，而图那条生成得更晚**
                //（要先把图传上 CDN），所以「同时发出」到达时往往是文字在前、图在后。
                // 只按到达顺序处理救不了这种，必须给文字留个窗口等图跟上。
                // 图一到就立刻往下走，不会白等满。
                let began = tokio::time::Instant::now();
                let deadline = began + std::time::Duration::from_millis(IMAGE_GRACE_MS);
                loop {
                    match tokio::time::timeout_at(deadline, rx.recv()).await {
                        Ok(Some(next)) if !next.images.is_empty() => {
                            // 这行就是这个窗口存在的理由：文字先到、图后到，靠等把图接住了。
                            // 差值 = 图那条比文字晚生成多久（图要先传上 CDN）。
                            tracing::info!(
                                "微信等图窗口：等了 {}ms 接到图片（图比文字晚生成 {}ms）user={u2}",
                                began.elapsed().as_millis(),
                                next.create_time_ms.saturating_sub(m.create_time_ms),
                            );
                            handle_message(&st, &u2, &t2, next).await; // 先把图挂上
                            break;
                        }
                        // 窗口里来的又是文字：排到后面，等当前这条处理完再说
                        Ok(Some(next)) => queue.push_back(next),
                        _ => {
                            tracing::info!(
                                "微信等图窗口：等满 {}ms 没有图，直接执行 user={u2}",
                                began.elapsed().as_millis()
                            );
                            break;
                        }
                    }
                }
                handle_message(&st, &u2, &t2, m).await;
            }
        });
    }

    loop {
        match get_updates(&token, &buf).await {
            Ok((msgs, next)) => {
                buf = next;
                backoff = 3;
                if priming {
                    priming = false;
                    if !msgs.is_empty() {
                        tracing::info!("微信首轮丢弃历史消息 {} 条 user={user}", msgs.len());
                    }
                    continue;
                }
                // 收到消息 = 会话活着，把可能残留的过期标记清掉
                if !msgs.is_empty() {
                    state.registry.write().await.set_weixin_expired(&user, false);
                }
                for m in msgs {
                    // 交给串行处理器，**别在这里 spawn**：微信里「图 + 说明文字」是两条
                    // 独立消息，各起一个任务就是在赛跑 —— 图那条要下载解密（几百毫秒），
                    // 文字那条立刻 dispatch，任务发出去时图还没挂上，只能等下一条任务
                    // 才被带走。实际踩到过。
                    let _ = tx.send(m);
                }
            }
            Err(e) if e.contains(&format!("errcode={ERR_SESSION_TIMEOUT}")) => {
                // 会话过期只能靠用户重新扫码，这里退出循环并置位，让前端提示
                tracing::warn!("微信会话过期 user={user}，需重新扫码");
                state.registry.write().await.set_weixin_expired(&user, true);
                state.weixin_reload.notify_one();
                return;
            }
            Err(e) => {
                // 长轮询超时是常态（服务端挂满 35s 没消息就断），不值得刷 warn
                if e.contains("timed out") || e.contains("operation timed out") {
                    continue;
                }
                tracing::warn!("微信长轮询异常 user={user}: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

/// 处理一条用户消息：刷新 context_token（主动推送要用）→ 交给渠道无关的 dispatch → 回发。
///
/// `reply` 传 None：ReplyCtx 是钉钉 webhook 专用的（「监控」持续推送走它），
/// 微信这边推送另走缓存的 context_token，见 `deliver`。
async fn handle_message(state: &crate::state::SharedState, user: &str, token: &str, m: Incoming) {
    state.registry.write().await.touch_weixin_context(user, &m.context_token);

    // 图片先落到「挂起待发」，再 dispatch —— 顺序不能反：同一条消息里图片配文字时
    // （「@2 看看这张图」），文字那条命令要能把刚挂上的图一起带走。
    let mut notes: Vec<String> = Vec::new();
    for (i, img) in m.images.iter().enumerate() {
        match fetch_image(img).await {
            Ok(bytes) => {
                let name = crate::bot::stash_weixin_image(state, user, bytes).await;
                notes.push(format!("📎 已收到图片「{name}」"));
            }
            Err(e) => {
                tracing::warn!("微信图片取回失败 user={user}: {e}");
                notes.push(format!("⚠️ 第 {} 张图片取回失败：{e}", i + 1));
            }
        }
    }

    let reply = if !m.text.trim().is_empty() {
        // 有文字就照常执行；图片已挂起，会被这条命令带上
        let out = crate::bot::dispatch(state, user, &m.text, None).await;
        if notes.is_empty() { out } else { format!("{}\n{out}", notes.join("\n")) }
    } else if !notes.is_empty() {
        format!("{}\n随下一条任务一起发出（如「@2 看看这张图」）。", notes.join("\n"))
    } else {
        "📎 微信这条通道目前只认文字和图片，语音之类还收不了 —— 烦请用文字发一遍。".to_string()
    };
    if reply.trim().is_empty() {
        return;
    }
    if let Err(e) = send_text(token, &m.from_user_id, &m.context_token, &for_weixin(&reply)).await {
        tracing::warn!("微信回复失败 user={user}: {e}");
    }
}

/// 出站适配。**微信这头是按 markdown 渲染的**（实测粗体、有序列表都生效），
/// 所以不拆标记 —— 早先按「只吃纯文本」拆掉标记是搞反了，层级全平、没法看。
///
/// 真正要处理的是换行：**单个 `\n` 会被当成软换行吃掉**，前后两行糊成一行
///（会话列表里「共 N 个活跃会话」「—— 设备 ——」「〔终端·项目〕」黏成一坨就是它）。
/// 实测行尾两空格、行尾反斜杠、`<br>` 三种硬换行写法**统统无效**，只有空行分段有用，
/// 所以每行之间插一个空行。
///
/// **表格是例外**：微信能渲染 markdown 表格（实测），但表格要求各行紧挨着 ——
/// 往里插空行会散成一堆 `| --- | --- |` 字面量。所以连续的表格行整块保持原样，
/// 只在块与块之间插空行。
pub fn for_weixin(s: &str) -> String {
    // 先走钉钉那套降级：删掉围栏行、保留代码内容。顺序不能反 —— 围栏还在的时候
    // 按行插空行会把代码块拆散成一堆独立段落。
    let downgraded = crate::mdfmt::downgrade_for_dingtalk(s);
    let mut blocks: Vec<String> = Vec::new();
    let mut table: Vec<String> = Vec::new();
    for raw in downgraded.lines() {
        let line = raw.trim_end(); // 行尾两空格是给别处的硬换行，这里没用，清掉
        if line.trim().is_empty() {
            continue; // 空行不必留：每行都会自成一段，再留就是双倍空隙
        }
        if line.trim_start().starts_with('|') {
            table.push(line.to_string());
            continue;
        }
        if !table.is_empty() {
            blocks.push(std::mem::take(&mut table).join("\n"));
        }
        blocks.push(line.to_string());
    }
    if !table.is_empty() {
        blocks.push(table.join("\n"));
    }
    blocks.join("\n\n")
}

// ───────────────────────────── 主动推送 ─────────────────────────────

/// 单条消息长度上限。协议文档没写死，取个保守值，超了按 `chunk_text` 分片逐条发
/// —— 微信这边没有「附件兜底」，砍掉就是真看不到了。
const MAX_LEN: usize = 2000;

/// 把一批事件推给已绑微信的账号。与 `dingtalk::deliver` 并行调用，互不影响。
///
/// 前提是缓存里有 `context_token`（用户绑定后给 bot 发过至少一句话）。实测这个
/// token 至少 1.8 小时后仍可用，且每收一条消息就会刷新，日常使用下不会失效。
pub async fn deliver(state: &crate::state::SharedState, events: Vec<crate::dingtalk::NotifyEvent>) {
    use crate::dingtalk::EventKind;
    for ev in events {
        // 与钉钉同口径：只推这四类，设备上线之类不推（噪音）
        if !matches!(
            ev.kind,
            EventKind::NewSession | EventKind::Waiting | EventKind::Finished | EventKind::Select
        ) {
            continue;
        }
        let Some(bot) = state.registry.read().await.weixin_bot_of(&ev.owner) else {
            tracing::debug!("微信推送跳过（{}）：该账号没绑微信", ev.owner);
            continue;
        };
        if bot.session_expired || bot.bot_token.is_empty() {
            tracing::info!("微信推送跳过（{}）：登录态失效，待重新扫码", ev.owner);
            continue;
        }
        if bot.context_token.is_empty() {
            tracing::info!("微信推送跳过（{}）：还没收到过消息，拿不到 context_token", ev.owner);
            continue;
        }

        // `{NO}` → 「#N 」视觉标签，`{N}` → 纯数字（用在「发 N」这类指令语法里）
        let no = match &ev.task_id {
            Some(id) => crate::bot::session_number(state, &ev.owner, id).await,
            None => None,
        };
        let text = match no {
            Some(n) => ev.text.replace("{NO}", &format!("#{n} ")).replace("{N}", &n.to_string()),
            None => ev.text.replace("{NO}", "").replace("{N}", "N"),
        };

        let to = if bot.ilink_user_id.is_empty() { &bot.ilink_bot_id } else { &bot.ilink_user_id };
        let chunks = crate::mdfmt::chunk_text(&for_weixin(&text), MAX_LEN);
        tracing::info!("微信推送（{}）：{} 片，收件人 {to}", ev.owner, chunks.len());
        for chunk in chunks {
            if let Err(e) = send_text(&bot.bot_token, to, &bot.context_token, &chunk).await {
                tracing::warn!("微信推送失败（{}）: {e}", ev.owner);
                // token 废了就置位，长轮询循环也会随之停下，前端提示重新扫码
                if e.contains(&format!("errcode={ERR_SESSION_TIMEOUT}")) {
                    state.registry.write().await.set_weixin_expired(&ev.owner, true);
                    state.weixin_reload.notify_one();
                }
                break; // 这条发不出去，剩下的分片也别试了
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errcode_prefers_errcode_field() {
        // 实测踩过：只看 ret 会把 -14 当成成功，20 轮错误被当成「无消息」
        assert_eq!(errcode(&json!({"errcode": -14, "errmsg": "session timeout"})), -14);
        assert_eq!(errcode(&json!({"ret": -14})), -14);
        assert_eq!(errcode(&json!({"msgs": []})), 0);
    }

    /// 联网冒烟：确认请求头/路径这套真能从服务端换回一张二维码。
    /// 默认跳过（CI 不该依赖外网），手动跑：`cargo test -p am-hub -- --ignored qrcode`
    #[tokio::test]
    #[ignore]
    async fn live_qrcode_roundtrip() {
        let q = fetch_qrcode().await.expect("取二维码应成功");
        assert!(q.link.starts_with("https://"), "link 应是可扫的链接: {}", q.link);
        assert!(!q.id.is_empty());
    }

    #[test]
    fn for_weixin_keeps_markdown_and_breaks_lines() {
        // markdown 要留着：微信这头是渲染的，拆了就没层级
        assert_eq!(for_weixin("**已发出**"), "**已发出**");
        // 单换行会被吃掉，必须扩成空行分段
        assert_eq!(for_weixin("共 2 个会话：\n1. 甲\n2. 乙"), "共 2 个会话：\n\n1. 甲\n\n2. 乙");
        // 已有的空行不叠加，行尾空格清掉
        assert_eq!(for_weixin("标题\n\n\n正文  "), "标题\n\n正文");
        // 围栏删掉、代码内容留着（与钉钉同一套降级）
        assert_eq!(for_weixin("说明\n```rust\nlet x = 1;\n```"), "说明\n\nlet x = 1;");
    }

    #[test]
    fn decrypt_image_roundtrip_and_rejects_wrong_key() {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        let aeskey = "255d38f3384640fbec2b9d9eb2daf064"; // 线上真实那张图的密钥格式
        let key = unhex(aeskey).unwrap();
        let plain = b"\x89PNG\r\n\x1a\n fake image body".to_vec();

        // 按认定的方案加密回去：AES-128-ECB + PKCS#7
        let pad = 16 - plain.len() % 16;
        let mut ct = plain.clone();
        ct.extend(std::iter::repeat(pad as u8).take(pad));
        let cipher = aes::Aes128::new(GenericArray::from_slice(&key));
        for b in ct.chunks_mut(16) {
            cipher.encrypt_block(GenericArray::from_mut_slice(b));
        }

        assert_eq!(decrypt_image(ct.clone(), aeskey, plain.len()).unwrap(), plain);
        // 声明长度对不上要拒——这是「解出来像图其实是花的」的兜底
        assert!(decrypt_image(ct.clone(), aeskey, plain.len() + 1).is_err());
        // 换个密钥必须失败，不能悄悄返回一堆乱码
        assert!(decrypt_image(ct, "00112233445566778899aabbccddeeff", plain.len()).is_err());
    }

    #[test]
    fn for_weixin_keeps_tables_contiguous() {
        // 微信能渲染 markdown 表格，但插了空行就散成 `| --- | --- |` 字面量。
        // 表格块内保持单换行，块与前后文之间才空行。
        let src = "结果如下：\n| 环节 | 结果 |\n| --- | --- |\n| 扫码 | 通过 |\n完毕";
        assert_eq!(
            for_weixin(src),
            "结果如下：\n\n| 环节 | 结果 |\n| --- | --- |\n| 扫码 | 通过 |\n\n完毕"
        );
    }

    #[test]
    fn uin_is_base64_of_decimal() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};
        let u = uin();
        let raw = String::from_utf8(B64.decode(&u).expect("应是合法 base64")).unwrap();
        assert!(raw.chars().all(|c| c.is_ascii_digit()), "解出来应是十进制数字串: {raw}");
    }
}
