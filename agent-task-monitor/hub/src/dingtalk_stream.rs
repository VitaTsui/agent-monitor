//! 钉钉 Stream 模式（长连接）客户端。
//!
//! 为什么用它：钉钉企业机器人「HTTP 回调」要求钉钉的中国服务器能公网入站访问我们的
//! 回调地址；hub 部署在海外(Vultr)时中国→海外常不可达，回调地址校验就失败。Stream 模式
//! 反过来——**hub 主动**用 AppKey/AppSecret 向钉钉网关建 WebSocket 长连接来收消息，不需要
//! 任何公网入站地址，绕开可达性问题。
//!
//! 流程：POST /v1.0/gateway/connections/open 拿 {endpoint,ticket} → 连 `endpoint?ticket=…`
//! → 收帧：SYSTEM/ping 回 pong、SYSTEM/disconnect 断开重连、机器人消息帧解析 text.content
//! → 复用 bot::dispatch 得回复 → 通过消息里的 sessionWebhook 回发 → 每帧回 ACK(code 200)。
//! 掉线由 supervise 退避重连；凭据变更由 run 管理器 abort 重启。

use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::state::SharedState;

/// 后台管理器：按 registry 里配置了 Stream 的钉钉应用，各维持一条长连接；凭据变更或
/// 配置删除时重启/停掉对应连接。
pub async fn run(state: SharedState) {
    // user -> (凭据签名, 监督任务句柄)
    let mut running: HashMap<String, (String, tokio::task::JoinHandle<()>)> = HashMap::new();
    loop {
        let desired: HashMap<String, (String, String)> = state
            .registry
            .read()
            .await
            .dingtalk_stream_apps()
            .into_iter()
            .map(|(u, k, s)| (u, (k, s)))
            .collect();

        // 停掉「已删除」或「凭据已变」的连接
        running.retain(|user, (sig, handle)| match desired.get(user) {
            Some((k, s)) if &format!("{k}\u{0}{s}") == sig && !handle.is_finished() => true,
            _ => {
                handle.abort();
                false
            }
        });

        // 启动缺失的连接
        for (user, (k, s)) in &desired {
            let sig = format!("{k}\u{0}{s}");
            if !running.contains_key(user) {
                let st = state.clone();
                let (user2, k2, s2) = (user.clone(), k.clone(), s.clone());
                let handle = tokio::spawn(async move { supervise(st, user2, k2, s2).await });
                running.insert(user.clone(), (sig, handle));
            }
        }

        // 平时 30s 扫一轮；用户刚改完机器人配置会立刻叫醒我们，免得他在钉钉那头
        // 等半分钟没反应、以为配错了。
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            _ = state.dingtalk_reload.notified() => {}
        }
    }
}

/// 单用户连接监督：断了就退避重连（3s→最长 60s）。
async fn supervise(state: SharedState, user: String, app_key: String, app_secret: String) {
    let mut backoff = 3u64;
    loop {
        match connect_once(&state, &user, &app_key, &app_secret).await {
            Ok(()) => {
                tracing::info!("钉钉 Stream 正常断开，准备重连 user={user}");
                backoff = 3;
            }
            Err(e) => {
                tracing::warn!("钉钉 Stream 连接异常 user={user}: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

/// 一条连接的完整生命周期：open → 连 ws → 收帧处理，直到断开或出错返回。
async fn connect_once(
    state: &SharedState,
    user: &str,
    app_key: &str,
    app_secret: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // 1) 拿网关连接端点 + ticket
    let open: Value = client
        .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
        .json(&json!({
            "clientId": app_key,
            "clientSecret": app_secret,
            "subscriptions": [{ "type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get" }],
            "ua": "agent-monitor-hub/1.0",
            "localIp": "127.0.0.1"
        }))
        .send()
        .await?
        .json()
        .await?;
    let endpoint = open
        .get("endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("open 无 endpoint（AppKey/AppSecret 是否正确？）: {open}"))?;
    let ticket = open
        .get("ticket")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("open 无 ticket: {open}"))?;
    let url = format!("{endpoint}?ticket={}", crate::dingtalk::urlencode(ticket));

    // 2) 建 WebSocket
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await?;
    tracing::info!("钉钉 Stream 已连接 user={user}");

    // 3) 收帧循环（读/回在同一流上顺序处理；耗时的回复另起任务，不挡 ping）
    while let Some(msg) = ws.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(p) => {
                ws.send(Message::Pong(p)).await?;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };
        let frame: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let ftype = frame.get("type").and_then(Value::as_str).unwrap_or("");
        let topic = frame.pointer("/headers/topic").and_then(Value::as_str).unwrap_or("");
        let message_id = frame
            .pointer("/headers/messageId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        match (ftype, topic) {
            // 心跳：回 pong（原样带回 data）
            ("SYSTEM", "ping") => {
                let data = frame.get("data").cloned().unwrap_or_else(|| json!("{}"));
                let pong = json!({
                    "specVersion": "1.0",
                    "type": "SYSTEM",
                    "headers": { "contentType": "application/json", "messageId": message_id, "topic": "pong" },
                    "data": data
                });
                ws.send(Message::Text(pong.to_string())).await?;
            }
            // 网关要求断开（会给新连接）：跳出去重连
            ("SYSTEM", "disconnect") => break,
            // 机器人消息
            (_, "/v1.0/im/bot/messages/get") => {
                // data 是一段 JSON 字符串
                let data_str = frame.get("data").and_then(Value::as_str).unwrap_or("{}");
                let m: Value = serde_json::from_str(data_str).unwrap_or(Value::Null);
                let msgtype = m.get("msgtype").and_then(Value::as_str).unwrap_or("");
                // 文字：text 直接取；richText（图文一起发）取其中的文字段
                let content = if msgtype == "richText" {
                    m.pointer("/content/richText")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|it| it.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                } else {
                    m.pointer("/text/content").and_then(Value::as_str).unwrap_or("").trim().to_string()
                };
                // 文件/图片：file(带 fileName) / picture / richText 内嵌多图 → 全部暂存待发
                let files = extract_files(&m, msgtype);
                let session_webhook =
                    m.get("sessionWebhook").and_then(Value::as_str).unwrap_or("").to_string();
                let webhook_expiry =
                    m.get("sessionWebhookExpiredTime").and_then(Value::as_u64).unwrap_or(0);
                // 捕获发信人身份：主动推送（任务完成/需要操作）靠 OTO 发给这个人。
                // robotCode 缺省回落到 app_key（Stream 机器人一般二者一致）。
                let staff_id =
                    m.get("senderStaffId").and_then(Value::as_str).unwrap_or("").to_string();
                let sender_nick =
                    m.get("senderNick").and_then(Value::as_str).unwrap_or("").to_string();
                let robot_code = m
                    .get("robotCode")
                    .or_else(|| m.get("chatbotUserId"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                tracing::info!(
                    "钉钉 Stream 收到机器人消息 user={user} 内容={content:?} 有回发地址={}",
                    !session_webhook.is_empty()
                );

                // 先 ACK 该帧（钉钉据此认为已消费）
                ack(&mut ws, &message_id).await?;

                // 「绑定 <码>」抢在认人之前：需要它的人正是还认不出来的那个。
                // 处理完直接回执，不进后面的文件暂存/dispatch。
                let bind_reply =
                    crate::bot::try_bind_command(&state, &staff_id, &sender_nick, &content).await;

                // 认归属账号；未绑定 → 回引导（不落文件、不 dispatch）
                let account = match &bind_reply {
                    Some(_) => Err(String::new()), // 绑定指令：账号无关，下面按 bind_reply 回
                    None => {
                        crate::bot::resolve_account(&state, user, &staff_id, &robot_code, &sender_nick)
                            .await
                    }
                };

                // 带文件/图片：仅对已绑定账号暂存（按账号存，send_input 也按账号取）。
                // 多张图片/文件全部累积，落盘名去重避免互相覆盖（原来只取一张就是同名覆盖导致）。
                if !files.is_empty() {
                    if let Ok(acct) = &account {
                        let now = crate::state::now_secs();
                        let mut map = state.bot_pending_files.write().await;
                        let list = map.entry(acct.clone()).or_default();
                        for (code, name) in files {
                            let base = if name.trim().is_empty() {
                                format!("钉钉文件-{}", &code[..code.len().min(8)])
                            } else {
                                name
                            };
                            let fname = unique_name(list, &base);
                            list.push(crate::state::BotPendingFile {
                                download_code: code,
                                file_name: fname.clone(),
                                app_user: user.to_string(),
                                at: now,
                            });
                            tracing::info!("钉钉 Stream 暂存待发文件 account={acct} name={fname}");
                        }
                    }
                }
                // 文件-only（没带文字指令）：回执提示，不进 dispatch
                let file_only = content.is_empty();

                // dispatch + 通过 sessionWebhook 回发，另起任务避免阻塞收帧（心跳要及时）
                if !session_webhook.is_empty() {
                    let st = state.clone();
                    let cl = client.clone();
                    let sw = session_webhook.clone();
                    tokio::spawn(async move {
                        let ctx = crate::bot::ReplyCtx {
                            webhook: sw.clone(),
                            expiry_ms: webhook_expiry,
                            staff_id,
                            robot_code,
                        };
                        let reply = match account {
                            // 绑定指令：直接回它的结果（此时 account 是占位的 Err）
                            _ if bind_reply.is_some() => bind_reply.unwrap_or_default(),
                            // 未绑定：回引导（登录链接 + 绑定码两条路）
                            Err(guide) => guide,
                            Ok(acct) if file_only => {
                                let _ = &acct;
                                "📎 已收到文件，随下一条任务一起发出（如「@2 处理这个文件」），\
                                 会存到该会话目录的 tmp/ 下并把路径拼到任务开头。"
                                    .to_string()
                            }
                            Ok(acct) => crate::bot::dispatch(&st, &acct, &content, Some(&ctx)).await,
                        };
                        match cl
                            .post(&sw)
                            .json(&json!({ "msgtype": "text", "text": { "content": reply } }))
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                tracing::info!("钉钉 Stream 已回发 http={}", resp.status())
                            }
                            Err(e) => tracing::warn!("钉钉 Stream 回发失败: {e}"),
                        }
                    });
                } else {
                    tracing::warn!("钉钉 Stream 机器人消息无 sessionWebhook，无法回发");
                }
            }
            // 其它事件帧：也 ACK 掉，避免网关重投
            _ => {
                // 心跳(ping)已在上面单列，这里记录其它未知帧便于排查
                if ftype != "SYSTEM" {
                    tracing::info!("钉钉 Stream 收到未处理帧 type={ftype} topic={topic}");
                }
                if !message_id.is_empty() {
                    ack(&mut ws, &message_id).await?;
                }
            }
        }
    }
    Ok(())
}

/// 回一个 code=200 的 ACK 帧
async fn ack<S>(ws: &mut S, message_id: &str) -> anyhow::Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let ack = json!({
        "code": 200,
        "headers": { "contentType": "application/json", "messageId": message_id },
        "message": "OK",
        "data": "{}"
    });
    ws.send(Message::Text(ack.to_string())).await?;
    Ok(())
}

/// 从机器人消息里抽取文件/图片的 (downloadCode, fileName)。支持 file / picture / richText 内嵌图片。
/// 无附件返回 (None, "")。
/// 从一条消息里抽出**全部**待发文件：(downloadCode, 建议文件名)。richText 内嵌多图会全取，
/// 不再只取第一张。名字可能重复（多张「图片.jpg」），去重交由存储时的 `unique_name`。
fn extract_files(m: &Value, msgtype: &str) -> Vec<(String, String)> {
    match msgtype {
        "file" => m
            .pointer("/content/downloadCode")
            .and_then(Value::as_str)
            .map(|c| {
                let name = m
                    .pointer("/content/fileName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                vec![(c.to_string(), name)]
            })
            .unwrap_or_default(),
        "picture" => m
            .pointer("/content/downloadCode")
            .or_else(|| m.pointer("/content/pictureDownloadCode"))
            .and_then(Value::as_str)
            .map(|c| vec![(c.to_string(), "图片.jpg".to_string())])
            .unwrap_or_default(),
        "richText" => m
            .pointer("/content/richText")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|it| it.get("downloadCode").and_then(Value::as_str))
                    .enumerate()
                    .map(|(i, c)| (c.to_string(), format!("图片{}.jpg", i + 1)))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 文件名去重：已存在同名就在扩展名前加 -2/-3…，避免多张「图片.jpg」落盘时互相覆盖。
fn unique_name(existing: &[crate::state::BotPendingFile], name: &str) -> String {
    let taken = |n: &str| existing.iter().any(|f| f.file_name == n);
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut i = 2;
    loop {
        let cand = format!("{stem}-{i}{ext}");
        if !taken(&cand) {
            return cand;
        }
        i += 1;
    }
}
