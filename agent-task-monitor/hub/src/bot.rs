//! 机器人指令网关：每个用户自助接入自己的企业微信自建应用 / 钉钉企业应用，
//! 用文字指令遥控自己的会话（查看/暂停/恢复/中断/终止/发布输入）。
//!
//! 路由靠回调 URL 里的 channel：`/monitor/int/{wecom|dingtalk}/<channel>`。
//! channel 反查到配置所属用户 → 指令即以该用户身份执行（URL 即绑定，无需绑定码）。

use crate::state::SharedState;
use crate::{dingtalk, wecom};
use am_core::model::{ControlAction, ControlCmd, TaskStatus};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

// ---------- 企业微信自建应用回调（每用户 channel） ----------

#[derive(Deserialize)]
pub struct WecomCbQuery {
    msg_signature: String,
    timestamp: String,
    nonce: String,
    #[serde(default)]
    echostr: String,
}

/// GET /monitor/int/wecom/:channel —— 企业微信「接收消息」URL 验证
pub async fn wecom_verify(
    State(state): State<SharedState>,
    Path(channel): Path<String>,
    Query(q): Query<WecomCbQuery>,
) -> impl IntoResponse {
    let Some((_, app)) = state.registry.read().await.wecom_app_by_channel(&channel) else {
        return (StatusCode::NOT_FOUND, "无效的回调地址".to_string());
    };
    let Some(cfg) = wecom::WecomConfig::from_parts(&app.token, &app.aes_key, &app.corp_id) else {
        return (StatusCode::BAD_REQUEST, "配置的 EncodingAESKey 非法".to_string());
    };
    let sig = wecom::msg_signature(&cfg.token, &q.timestamp, &q.nonce, &q.echostr);
    if sig != q.msg_signature {
        return (StatusCode::FORBIDDEN, "签名校验失败".to_string());
    }
    match wecom::decrypt(&cfg, &q.echostr) {
        Ok(plain) => (StatusCode::OK, plain),
        Err(e) => (StatusCode::BAD_REQUEST, e),
    }
}

/// POST /monitor/int/wecom/:channel —— 企业微信收消息 + 被动回复（加密）
pub async fn wecom_message(
    State(state): State<SharedState>,
    Path(channel): Path<String>,
    Query(q): Query<WecomCbQuery>,
    body: String,
) -> impl IntoResponse {
    let Some((owner, app)) = state.registry.read().await.wecom_app_by_channel(&channel) else {
        return (StatusCode::NOT_FOUND, String::new());
    };
    let Some(cfg) = wecom::WecomConfig::from_parts(&app.token, &app.aes_key, &app.corp_id) else {
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let Some(encrypt) = wecom::xml_field(&body, "Encrypt") else {
        return (StatusCode::BAD_REQUEST, String::new());
    };
    if wecom::msg_signature(&cfg.token, &q.timestamp, &q.nonce, &encrypt) != q.msg_signature {
        return (StatusCode::FORBIDDEN, String::new());
    }
    let inner = match wecom::decrypt(&cfg, &encrypt) {
        Ok(x) => x,
        Err(_) => return (StatusCode::BAD_REQUEST, String::new()),
    };
    let msg_type = wecom::xml_field(&inner, "MsgType").unwrap_or_default();
    let content = wecom::xml_field(&inner, "Content").unwrap_or_default();
    let reply = if msg_type == "text" {
        dispatch(&state, &owner, content.trim()).await
    } else {
        "只认文字指令，发「帮助」看用法。".to_string()
    };
    let rand16 = rand16();
    let xml = wecom::build_reply(&cfg, &reply, &q.timestamp, &q.nonce, &rand16);
    (StatusCode::OK, xml)
}

// ---------- 钉钉企业应用回调（每用户 channel，同步回复） ----------

/// POST /monitor/int/dingtalk/:channel —— 钉钉企业应用「消息接收(HTTP)」回调
pub async fn dingtalk_message(
    State(state): State<SharedState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Json<Value> {
    let Some((owner, app)) = state.registry.read().await.dingtalk_app_by_channel(&channel) else {
        return Json(json!({}));
    };
    // 验签：header timestamp + sign
    let ts = headers.get("timestamp").and_then(|v| v.to_str().ok()).unwrap_or("");
    let sign = headers.get("sign").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !dingtalk::verify_app_sign(&app.app_secret, ts, sign) {
        return Json(json!({}));
    }
    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let content = payload
        .pointer("/text/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let reply = dispatch(&state, &owner, &content).await;
    // 同步回复：钉钉直接把响应体当作机器人回复消息
    Json(json!({ "msgtype": "text", "text": { "content": reply } }))
}

fn rand16() -> [u8; 16] {
    let b = uuid::Uuid::new_v4().into_bytes();
    let mut r = [0u8; 16];
    r.copy_from_slice(&b[..16]);
    r
}

// ---------- 指令调度（渠道无关，以账号身份执行） ----------

/// 指令分发，返回给用户的文字回复。username 已由回调 URL 的 channel 确定。
async fn dispatch(state: &SharedState, username: &str, text: &str) -> String {
    let (cmd, arg) = split_cmd(text);
    match cmd.as_str() {
        "帮助" | "help" | "?" | "？" | "菜单" | "" => help_text(),
        "会话" | "列表" | "ls" | "任务" => list_sessions(state, username).await,
        "设备" | "devices" => list_devices(state, username).await,
        "暂停" => control(state, username, &arg, ControlAction::Pause, "已暂停").await,
        "恢复" | "继续" => control(state, username, &arg, ControlAction::Resume, "已恢复").await,
        "中断" => control(state, username, &arg, ControlAction::Interrupt, "已中断").await,
        "终止" | "停止" => control(state, username, &arg, ControlAction::Stop, "已终止").await,
        "发" | "发送" | "回复" | "输入" => send_input(state, username, &arg).await,
        _ => format!("未知指令「{cmd}」。发「帮助」看用法。"),
    }
}

fn split_cmd(text: &str) -> (String, String) {
    let text = text.trim();
    match text.split_once(char::is_whitespace) {
        Some((c, rest)) => (c.to_string(), rest.trim().to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn help_text() -> String {
    "终端监控机器人 · 指令：\n\
     • 会话 —— 列出当前会话（带序号）\n\
     • 设备 —— 列出名下设备\n\
     • 暂停 N / 恢复 N / 中断 N / 终止 N —— 控制第 N 个会话\n\
     • 发 N 内容 —— 向第 N 个会话发布一条输入\n\
     • 帮助 —— 显示本说明\n\
     （序号以最近一次「会话」列出的为准）"
        .to_string()
}

async fn list_sessions(state: &SharedState, username: &str) -> String {
    let mut tasks = state.tasks_for(username).await;
    tasks.sort_by_key(|t| match t.status {
        TaskStatus::Running => 0,
        TaskStatus::Paused => 1,
        TaskStatus::Idle => 2,
        TaskStatus::Finished => 3,
    });
    if tasks.is_empty() {
        return "当前没有会话。".to_string();
    }
    let mut ids = Vec::with_capacity(tasks.len());
    let mut lines = vec![format!("共 {} 个会话：", tasks.len())];
    for (i, t) in tasks.iter().enumerate() {
        ids.push(t.id.clone());
        let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
        let title: String = title.chars().take(24).collect();
        lines.push(format!("{}. [{}] {} · {}", i + 1, status_zh(t.status), title, t.project_name));
    }
    state.bot_last_list.write().await.insert(username.to_string(), ids);
    lines.push("\n操作示例：暂停 1 / 发 1 继续".to_string());
    lines.join("\n")
}

async fn list_devices(state: &SharedState, username: &str) -> String {
    let devs = state.devices_for(username).await;
    if devs.is_empty() {
        return "名下没有设备。".to_string();
    }
    let mut lines = vec![format!("共 {} 台设备：", devs.len())];
    for d in &devs {
        lines.push(format!(
            "• {} · {} · {} 会话",
            d.hostname,
            if d.online { "在线" } else { "离线" },
            d.session_count
        ));
    }
    lines.join("\n")
}

fn status_zh(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Running => "执行中",
        TaskStatus::Paused => "已暂停",
        TaskStatus::Idle => "等待",
        TaskStatus::Finished => "已结束",
    }
}

async fn resolve_task(state: &SharedState, username: &str, arg: &str) -> Result<String, String> {
    let n: usize = arg
        .trim()
        .parse()
        .map_err(|_| "请给会话序号，如「暂停 1」。先发「会话」看序号。".to_string())?;
    let list = state.bot_last_list.read().await;
    let ids = list.get(username).ok_or("请先发「会话」列出序号。".to_string())?;
    ids.get(n.wrapping_sub(1))
        .cloned()
        .ok_or(format!("没有第 {n} 个会话，先发「会话」看最新列表。"))
}

async fn control(
    state: &SharedState,
    username: &str,
    arg: &str,
    action: ControlAction,
    ok_word: &str,
) -> String {
    let task_id = match resolve_task(state, username, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match queue_command(state, username, &task_id, action, None).await {
        Ok(_) => format!("{ok_word}（会话 {arg}）。"),
        Err(e) => e,
    }
}

async fn send_input(state: &SharedState, username: &str, arg: &str) -> String {
    let (idx, text) = split_cmd(arg);
    if text.is_empty() {
        return "用法：发 <序号> <内容>，如「发 1 继续」。".to_string();
    }
    let task_id = match resolve_task(state, username, &idx).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match queue_command(state, username, &task_id, ControlAction::Input, Some(text.clone())).await {
        Ok(_) => format!("已发送到会话 {idx}：{text}"),
        Err(e) => e,
    }
}

async fn queue_command(
    state: &SharedState,
    username: &str,
    task_id: &str,
    action: ControlAction,
    text: Option<String>,
) -> Result<(), String> {
    let task = {
        let tasks = state.tasks_for(username).await;
        tasks.into_iter().find(|t| t.id == task_id)
    };
    let Some(task) = task else {
        return Err("会话不存在（可能已结束），先发「会话」看最新列表。".into());
    };
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return Err("会话所属设备已离线。".into());
    };
    if entry.last_report.elapsed().as_secs() >= crate::state::OFFLINE_AFTER_SECS {
        return Err("会话所属设备已离线。".into());
    }
    entry.pending.push_back(ControlCmd {
        task_id: task_id.to_string(),
        pid: task.pid,
        action,
        text,
        id: Some(uuid::Uuid::new_v4().to_string()),
        enqueued_ms: crate::state::now_secs() * 1000,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::split_cmd;

    #[test]
    fn split_command() {
        assert_eq!(split_cmd("会话"), ("会话".into(), "".into()));
        assert_eq!(split_cmd("暂停 3"), ("暂停".into(), "3".into()));
        assert_eq!(split_cmd("发 2 继续执行"), ("发".into(), "2 继续执行".into()));
    }
}
