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
        // 企业微信走 XML 同步回复，无会话 webhook，「监控」在此渠道不可用
        dispatch(&state, &owner, content.trim(), None).await
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
    // HTTP 模式的 sessionWebhook 也可用于「监控」持续推送
    let ctx = ReplyCtx {
        webhook: payload.get("sessionWebhook").and_then(Value::as_str).unwrap_or("").to_string(),
        expiry_ms: payload.get("sessionWebhookExpiredTime").and_then(Value::as_u64).unwrap_or(0),
        staff_id: payload.get("senderStaffId").and_then(Value::as_str).unwrap_or("").to_string(),
        robot_code: payload
            .get("robotCode")
            .or_else(|| payload.get("chatbotUserId"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    };
    let reply = dispatch(&state, &owner, &content, Some(&ctx)).await;
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
/// 回复上下文：钉钉会话 webhook + 失效时间，供「监控」注册持续推送用（企业微信暂无）
pub(crate) struct ReplyCtx {
    pub webhook: String,
    pub expiry_ms: u64,
    /// 发信人 staffId / 机器人 robotCode（Stream 渠道带；HTTP 回调可能为空）——「绑定」指令用
    #[allow(dead_code)]
    pub staff_id: String,
    #[allow(dead_code)]
    pub robot_code: String,
}

pub(crate) async fn dispatch(
    state: &SharedState,
    username: &str,
    text: &str,
    reply: Option<&ReplyCtx>,
) -> String {
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
        "监控" | "watch" => monitor_start(state, username, &arg, reply).await,
        "停止监控" | "取消监控" | "结束监控" | "unwatch" => monitor_stop(state, username).await,
        "撤回" | "recall" => recall_last(state, username, &arg).await,
        "绑定" | "bind" => bind_recipient(state, username, reply).await,
        "解绑" | "unbind" => unbind_recipient(state, username).await,
        _ => format!("未知指令「{cmd}」。发「帮助」看用法。"),
    }
}

/// 「绑定」：把当前发信人设为本账号主动推送（任务完成/会话结束）的接收人。
async fn bind_recipient(state: &SharedState, username: &str, reply: Option<&ReplyCtx>) -> String {
    let staff = reply.map(|c| c.staff_id.as_str()).unwrap_or("");
    if staff.is_empty() {
        return "拿不到你的 staffId，无法绑定（请在钉钉里私聊本企业应用机器人再发「绑定」）。".to_string();
    }
    let robot = reply.map(|c| c.robot_code.as_str()).unwrap_or("");
    if state.registry.write().await.bind_dingtalk_staff(username, staff, robot) {
        format!("✅ 已把你（staffId {staff}）绑定为推送接收人。\n任务完成 / 会话结束会私聊推给你。发「解绑」取消。")
    } else {
        "绑定失败：未找到本账号的钉钉应用配置。".to_string()
    }
}

/// 「解绑」：取消主动推送接收人。
async fn unbind_recipient(state: &SharedState, username: &str) -> String {
    if state.registry.write().await.unbind_dingtalk_staff(username) {
        "已解绑，不再主动私聊推送。需要时再发「绑定」。".to_string()
    } else {
        "当前没有绑定推送接收人。".to_string()
    }
}

/// 「监控 N」：注册对第 N 个会话的持续监控，新内容由后台循环推到当前钉钉会话
async fn monitor_start(
    state: &SharedState,
    username: &str,
    arg: &str,
    reply: Option<&ReplyCtx>,
) -> String {
    let Some(ctx) = reply else {
        return "当前渠道暂不支持持续监控。".to_string();
    };
    if ctx.webhook.is_empty() {
        return "拿不到本会话的推送地址，无法监控。".to_string();
    }
    let id = match resolve_task(state, username, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    // 起点定在「当前最后一条」，避免一上来把历史全推一遍；之后只推新增
    let msgs = state.bot_task_messages(&id).await;
    let last_ts = msgs.last().map(|m| m.timestamp.clone()).unwrap_or_default();
    state.bot_monitors.write().await.insert(
        username.to_string(),
        crate::state::BotMonitor {
            task_id: id,
            webhook: ctx.webhook.clone(),
            expiry_ms: ctx.expiry_ms,
            last_ts,
        },
    );
    "已开始监控该会话，有新内容会自动推到这里（约每 20s 检查一次）。发「停止监控」结束。\n\
     注：受钉钉会话地址时效/条数限制，长时间监控可能中断，届时再发「监控 N」即可。"
        .to_string()
}

/// 「撤回 N」：撤回第 N 个会话最近一条排队中的任务。还在 hub 队列就直接出队；
/// 已进终端原生队列就注入 ↑ 让终端撤回（与网页「撤回」同一套语义）。
async fn recall_last(state: &SharedState, username: &str, arg: &str) -> String {
    let task_id = match resolve_task(state, username, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let task = {
        let tasks = state.tasks_for(username).await;
        tasks.into_iter().find(|t| t.id == task_id)
    };
    let Some(task) = task else {
        return "会话不存在（可能已结束）。".to_string();
    };
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return "会话所属设备已离线。".to_string();
    };
    // 先撤 hub 队列里最后一条该会话的输入（还没下发给客户端，可直接撤）
    let pos = entry.pending.iter().rposition(|c| {
        c.task_id == task_id && matches!(c.action, am_core::model::ControlAction::Input)
    });
    if let Some(i) = pos {
        entry.pending.remove(i);
        return format!("已撤回排队中的任务（会话 {arg}）。");
    }
    // hub 队列里没有 → 已进终端原生队列，注入 ↑ 撤回
    entry.pending.push_back(ControlCmd {
        task_id: task_id.clone(),
        pid: task.pid,
        action: am_core::model::ControlAction::TermKey,
        text: Some("up:1".to_string()),
        id: None,
    });
    format!("已注入撤回 ↑（会话 {arg}）。Terminal.app 需在终端手动按 ↑。")
}

async fn monitor_stop(state: &SharedState, username: &str) -> String {
    if state.bot_monitors.write().await.remove(username).is_some() {
        "已停止监控。".to_string()
    } else {
        "当前没有在监控的会话。".to_string()
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
     • 撤回 N —— 撤回第 N 个会话最近一条排队中的任务\n\
     • 监控 N —— 持续把第 N 个会话的新内容推到这里\n\
     • 停止监控 —— 结束监控\n\
     • 绑定 / 解绑 —— 设为/取消「任务完成·会话结束」主动私聊推送的接收人\n\
     • 帮助 —— 显示本说明\n\
     （序号以最近一次「会话」列出的为准）"
        .to_string()
}

/// 机器人监控推送循环：每 20s 把各监控会话的新增消息推到其钉钉会话 webhook。
/// 只推「起点之后」的增量、批量合一条；webhook 失效或推送被钉钉拒（限流/过期）就停掉该监控。
pub async fn monitor_loop(state: SharedState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let now = crate::state::now_secs() * 1000;
        let monitors: Vec<(String, crate::state::BotMonitor)> = state
            .bot_monitors
            .read()
            .await
            .iter()
            .map(|(u, m)| (u.clone(), m.clone()))
            .collect();
        for (user, mon) in monitors {
            if mon.expiry_ms > 0 && now >= mon.expiry_ms {
                state.bot_monitors.write().await.remove(&user);
                let _ = push_webhook(
                    &mon.webhook,
                    "监控已到期（钉钉会话地址时效结束）。如需继续，请再发「监控 N」。",
                )
                .await;
                continue;
            }
            let msgs = state.bot_task_messages(&mon.task_id).await;
            let fresh: Vec<&am_core::model::MessageBrief> = msgs
                .iter()
                .filter(|m| m.timestamp.as_str() > mon.last_ts.as_str())
                .collect();
            if fresh.is_empty() {
                continue;
            }
            let text = render_monitor_push(&fresh);
            match push_webhook(&mon.webhook, &text).await {
                Ok(true) => {
                    if let Some(m) = state.bot_monitors.write().await.get_mut(&user) {
                        m.last_ts = fresh.last().map(|x| x.timestamp.clone()).unwrap_or_default();
                    }
                }
                // 钉钉返回错误（多为会话地址限流/过期）：停掉监控，避免空转刷错误
                Ok(false) => {
                    state.bot_monitors.write().await.remove(&user);
                }
                Err(_) => { /* 网络抖动：留着下轮重试 */ }
            }
        }
    }
}

/// 推到钉钉会话 webhook。返回 Ok(true)=成功、Ok(false)=钉钉判失败(errcode≠0)、Err=网络错。
async fn push_webhook(webhook: &str, content: &str) -> Result<bool, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(webhook)
        .json(&json!({ "msgtype": "text", "text": { "content": content } }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok(body.get("errcode").and_then(Value::as_i64).unwrap_or(0) == 0)
}

/// 把一批新消息渲染成一条推送文本（限长，避免超钉钉单条上限）
fn render_monitor_push(msgs: &[&am_core::model::MessageBrief]) -> String {
    let mut lines = vec!["🔔 会话新动态：".to_string()];
    for m in msgs.iter().rev().take(6).rev() {
        let who = match m.role.as_str() {
            "user" => "🧑 ",
            "assistant" => "🤖 ",
            _ => "• ",
        };
        let c: String = m.content.chars().take(280).collect();
        lines.push(format!("{who}{c}"));
    }
    let mut out = lines.join("\n");
    if out.chars().count() > 1800 {
        out = out.chars().take(1800).collect::<String>() + "…";
    }
    out
}

/// 会话在「发 N / 暂停 N」里的序号：与 resolve_task 同源 —— 优先用最近「会话」列出的顺序，
/// 没有就即时按同排序补一份。用于钉钉推送里带上编号，让人能直接「发 N」回应。
pub(crate) async fn session_number(
    state: &SharedState,
    username: &str,
    task_id: &str,
) -> Option<usize> {
    if let Some(ids) = state.bot_last_list.read().await.get(username) {
        if let Some(i) = ids.iter().position(|x| x == task_id) {
            return Some(i + 1);
        }
    }
    sorted_active_tasks(state, username)
        .await
        .iter()
        .position(|t| t.id == task_id)
        .map(|i| i + 1)
}

/// 活跃会话按「设备名 → 终端 → 项目 → 状态」稳定排序。「会话」列表顺序、以及
/// 「发 N / 暂停 N …」的序号都以它为准 —— 两处共用同一份排序，序号才不会对不上。
async fn sorted_active_tasks(state: &SharedState, username: &str) -> Vec<am_core::model::Task> {
    let mut tasks = state.tasks_for(username).await;
    // 已结束的会话不列出——机器人只关心还能操作的活跃会话
    tasks.retain(|t| t.status != TaskStatus::Finished);
    let rank = |s: TaskStatus| match s {
        TaskStatus::Running => 0,
        TaskStatus::Paused => 1,
        TaskStatus::Idle => 2,
        TaskStatus::Finished => 3,
    };
    tasks.sort_by(|a, b| {
        a.hostname
            .cmp(&b.hostname)
            .then(a.provider_dsr.cmp(&b.provider_dsr))
            .then(a.project_name.cmp(&b.project_name))
            .then(rank(a.status).cmp(&rank(b.status)))
    });
    tasks
}

async fn list_sessions(state: &SharedState, username: &str) -> String {
    let tasks = sorted_active_tasks(state, username).await;
    if tasks.is_empty() {
        return "当前没有活跃会话。".to_string();
    }
    let mut ids = Vec::with_capacity(tasks.len());
    let mut lines = vec![format!("共 {} 个活跃会话：", tasks.len())];
    let mut cur_dev = String::new();
    let mut cur_group = String::new(); // 终端·项目 子分组
    for t in &tasks {
        if t.hostname != cur_dev {
            cur_dev = t.hostname.clone();
            cur_group.clear(); // 换设备后子分组重置，第一条必出子标题
            lines.push(format!("—— 📱 {} ——", cur_dev));
        }
        let group = format!("{} · {}", t.provider_dsr, t.project_name);
        if group != cur_group {
            cur_group = group.clone();
            lines.push(format!("  〔{group}〕"));
        }
        ids.push(t.id.clone());
        // 子标题里已带终端·项目，行内只留状态 + 会话标题
        let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
        let title: String = title.chars().take(24).collect();
        lines.push(format!("  {}. [{}] {}", ids.len(), status_zh(t.status), title));
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
        .map_err(|_| "请给会话序号，如「暂停 1」。发「会话」看序号。".to_string())?;
    // 序号表还没建（从没发过「会话」，或 hub 重启清空了它）时即时补一份：
    // 用与「会话」完全相同的排序，让「发 N / 暂停 N」不必先发「会话」也能用。
    if !state.bot_last_list.read().await.contains_key(username) {
        let ids: Vec<String> =
            sorted_active_tasks(state, username).await.into_iter().map(|t| t.id).collect();
        state.bot_last_list.write().await.insert(username.to_string(), ids);
    }
    let list = state.bot_last_list.read().await;
    let ids = list.get(username).ok_or("当前没有活跃会话。".to_string())?;
    ids.get(n.wrapping_sub(1))
        .cloned()
        .ok_or(format!("没有第 {n} 个会话，发「会话」看最新列表。"))
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
