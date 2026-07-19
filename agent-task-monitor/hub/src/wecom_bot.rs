//! 企业微信机器人：回调接入 + 指令调度。
//! 用户在企业微信里给自建应用发文字指令，机器人代其操作会话（查看/暂停/
//! 恢复/中断/终止/发布输入）。仅覆盖前台核心功能，不含设置面板类操作。

use crate::state::SharedState;
use crate::wecom;
use am_core::model::{ControlAction, ControlCmd, TaskStatus};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::time::Instant;

/// 绑定码有效期（分钟）
const BIND_CODE_TTL_SECS: u64 = 10 * 60;

#[derive(Deserialize)]
pub struct CallbackQuery {
    msg_signature: String,
    timestamp: String,
    nonce: String,
    #[serde(default)]
    echostr: String,
}

/// GET /monitor/wecom/callback —— 企业微信「接收消息」URL 验证。
/// 校验签名 + 解密 echostr，明文原样返回。
pub async fn verify(
    State(state): State<SharedState>,
    Query(q): Query<CallbackQuery>,
) -> impl IntoResponse {
    let Some(cfg) = state.wecom.clone() else {
        return (StatusCode::NOT_FOUND, "企业微信机器人未启用".to_string());
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

/// POST /monitor/wecom/callback —— 收成员消息，同步返回被动回复（加密）。
pub async fn message(
    State(state): State<SharedState>,
    Query(q): Query<CallbackQuery>,
    body: String,
) -> impl IntoResponse {
    let Some(cfg) = state.wecom.clone() else {
        return (StatusCode::NOT_FOUND, String::new());
    };
    // 外层 XML 取 Encrypt，验签 + 解密
    let Some(encrypt) = wecom::xml_field(&body, "Encrypt") else {
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let sig = wecom::msg_signature(&cfg.token, &q.timestamp, &q.nonce, &encrypt);
    if sig != q.msg_signature {
        return (StatusCode::FORBIDDEN, String::new());
    }
    let inner = match wecom::decrypt(&cfg, &encrypt) {
        Ok(x) => x,
        Err(_) => return (StatusCode::BAD_REQUEST, String::new()),
    };

    let from = wecom::xml_field(&inner, "FromUserName").unwrap_or_default();
    let msg_type = wecom::xml_field(&inner, "MsgType").unwrap_or_default();
    let content = wecom::xml_field(&inner, "Content").unwrap_or_default();

    // 非文本（图片/事件等）：回一句提示，不报错
    let reply = if msg_type == "text" {
        dispatch(&state, &from, content.trim()).await
    } else {
        "只认文字指令，发「帮助」看用法。".to_string()
    };

    // 被动回复：企业微信要求密文 XML；random16 用 uuid 派生（非确定性即可）
    let rand16 = {
        let b = uuid::Uuid::new_v4().into_bytes();
        let mut r = [0u8; 16];
        r.copy_from_slice(&b[..16]);
        r
    };
    let xml = wecom::build_reply(&cfg, &reply, &q.timestamp, &q.nonce, &rand16);
    (StatusCode::OK, xml)
}

/// 指令分发。返回给用户的文字回复。
async fn dispatch(state: &SharedState, wecom_userid: &str, text: &str) -> String {
    // 绑定指令：无需先绑定
    if let Some(code) = text
        .strip_prefix("绑定")
        .or_else(|| text.strip_prefix("bind"))
        .or_else(|| text.strip_prefix("BIND"))
    {
        return do_bind(state, wecom_userid, code.trim()).await;
    }

    // 其余指令都需要已绑定账号
    let Some(username) = state.registry.read().await.wecom_user_of(wecom_userid) else {
        return "尚未绑定账号。\n请到网页端「设置 → 账户」生成绑定码，再发送：\n绑定 <绑定码>".to_string();
    };

    let (cmd, arg) = split_cmd(text);
    match cmd.as_str() {
        "帮助" | "help" | "?" | "？" | "菜单" => help_text(),
        "会话" | "列表" | "ls" | "任务" => list_sessions(state, wecom_userid, &username).await,
        "设备" | "devices" => list_devices(state, &username).await,
        "暂停" => control_by_index(state, wecom_userid, &username, &arg, ControlAction::Pause, "已暂停").await,
        "恢复" | "继续" => control_by_index(state, wecom_userid, &username, &arg, ControlAction::Resume, "已恢复").await,
        "中断" => control_by_index(state, wecom_userid, &username, &arg, ControlAction::Interrupt, "已中断").await,
        "终止" | "停止" => control_by_index(state, wecom_userid, &username, &arg, ControlAction::Stop, "已终止").await,
        "发" | "发送" | "回复" | "输入" => send_input(state, wecom_userid, &username, &arg).await,
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

async fn do_bind(state: &SharedState, wecom_userid: &str, code: &str) -> String {
    if code.is_empty() {
        return "用法：绑定 <绑定码>（在网页端「设置 → 账户」生成）".to_string();
    }
    let hit = {
        let mut map = state.wecom_bind_codes.write().await;
        map.retain(|_, (_, t)| t.elapsed().as_secs() < BIND_CODE_TTL_SECS);
        map.remove(&code.to_uppercase())
    };
    match hit {
        Some((username, _)) => {
            state.registry.write().await.bind_wecom(wecom_userid, &username);
            format!("绑定成功，你现在可以遥控账号「{username}」的会话了。发「帮助」看指令。")
        }
        None => "绑定码无效或已过期，请到网页端重新生成。".to_string(),
    }
}

/// 列会话并记录序号→task_id，供后续按序号操作
async fn list_sessions(state: &SharedState, wecom_userid: &str, username: &str) -> String {
    let mut tasks = state.tasks_for(username).await;
    // 稳定排序：活跃在前
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
        lines.push(format!(
            "{}. [{}] {} · {}",
            i + 1,
            status_zh(t.status),
            title,
            t.project_name
        ));
    }
    state.wecom_last_list.write().await.insert(wecom_userid.to_string(), ids);
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

/// 解析序号 → 最近一次列出的 task_id
async fn resolve_task(state: &SharedState, wecom_userid: &str, arg: &str) -> Result<String, String> {
    let n: usize = arg
        .trim()
        .parse()
        .map_err(|_| "请给会话序号，如「暂停 1」。先发「会话」看序号。".to_string())?;
    let list = state.wecom_last_list.read().await;
    let ids = list.get(wecom_userid).ok_or("请先发「会话」列出序号。".to_string())?;
    ids.get(n.wrapping_sub(1))
        .cloned()
        .ok_or(format!("没有第 {n} 个会话，先发「会话」看最新列表。"))
}

async fn control_by_index(
    state: &SharedState,
    wecom_userid: &str,
    username: &str,
    arg: &str,
    action: ControlAction,
    ok_word: &str,
) -> String {
    let task_id = match resolve_task(state, wecom_userid, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match queue_command(state, username, &task_id, action, None).await {
        Ok(_) => format!("{ok_word}（会话 {arg}）。"),
        Err(e) => e,
    }
}

async fn send_input(state: &SharedState, wecom_userid: &str, username: &str, arg: &str) -> String {
    let (idx, text) = split_cmd(arg);
    if text.is_empty() {
        return "用法：发 <序号> <内容>，如「发 1 继续」。".to_string();
    }
    let task_id = match resolve_task(state, wecom_userid, &idx).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match queue_command(state, username, &task_id, ControlAction::Input, Some(text.clone())).await {
        Ok(_) => format!("已发送到会话 {idx}：{text}"),
        Err(e) => e,
    }
}

/// 校验归属 + 目标机在线，向机器命令队列压入一条命令
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
    });
    Ok(())
}

/// 生成一次性绑定码（前台调用）：6 位大写字母数字，存入 state，10 分钟有效
pub async fn gen_bind_code(state: &SharedState, username: &str) -> String {
    use rand::Rng;
    const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code: String = {
        let mut rng = rand::thread_rng();
        (0..6).map(|_| ALPHA[rng.gen_range(0..ALPHA.len())] as char).collect()
    };
    let mut map = state.wecom_bind_codes.write().await;
    map.retain(|_, (_, t)| t.elapsed().as_secs() < BIND_CODE_TTL_SECS);
    map.insert(code.clone(), (username.to_string(), Instant::now()));
    code
}

#[cfg(test)]
mod tests {
    use super::split_cmd;

    #[test]
    fn split_command() {
        assert_eq!(split_cmd("会话"), ("会话".into(), "".into()));
        assert_eq!(split_cmd("暂停 3"), ("暂停".into(), "3".into()));
        assert_eq!(split_cmd("发 2 继续执行"), ("发".into(), "2 继续执行".into()));
        assert_eq!(split_cmd("  绑定   A1B2 "), ("绑定".into(), "A1B2".into()));
    }
}
