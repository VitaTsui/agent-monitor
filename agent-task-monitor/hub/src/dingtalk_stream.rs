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

        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
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
                let content = m
                    .pointer("/text/content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let session_webhook =
                    m.get("sessionWebhook").and_then(Value::as_str).unwrap_or("").to_string();
                tracing::info!(
                    "钉钉 Stream 收到机器人消息 user={user} 内容={content:?} 有回发地址={}",
                    !session_webhook.is_empty()
                );

                // 先 ACK 该帧（钉钉据此认为已消费）
                ack(&mut ws, &message_id).await?;

                // dispatch + 通过 sessionWebhook 回发，另起任务避免阻塞收帧（心跳要及时）
                if !session_webhook.is_empty() {
                    let st = state.clone();
                    let u = user.to_string();
                    let cl = client.clone();
                    tokio::spawn(async move {
                        let reply = crate::bot::dispatch(&st, &u, &content).await;
                        match cl
                            .post(&session_webhook)
                            .json(&json!({ "msgtype": "text", "text": { "content": reply } }))
                            .send()
                            .await
                        {
                            Ok(resp) => tracing::info!(
                                "钉钉 Stream 已回发 user={u} http={}",
                                resp.status()
                            ),
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
