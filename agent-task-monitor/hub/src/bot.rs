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
    let Some((app_owner, app)) = state.registry.read().await.dingtalk_app_by_channel(&channel) else {
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
    // 按 staffId 找归属账号；未绑定 → 回登录链接（不按渠道 owner 兜底）
    let nick = payload.get("senderNick").and_then(Value::as_str).unwrap_or("");
    let reply = match resolve_account(&state, &app_owner, &ctx.staff_id, &ctx.robot_code, nick).await
    {
        Ok(account) => dispatch(&state, &account, &content, Some(&ctx)).await,
        Err(link_reply) => link_reply,
    };
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

/// 会话级指令（吃一个会话号位 N）：`@x` 速记与多目标都只对这些指令 + 「内容(=发)」生效。
const SESSION_CMDS: &[&str] = &[
    "暂停", "恢复", "继续", "中断", "终止", "停止", "撤回", "监控", "watch", "排队", "队列", "queue",
];

/// 解析「@N …」速记为一组 (cmd, arg)。支持多目标：`@1 @2 xxx`、`@1 @2 暂停`、`@x 排队`。
/// - 非 @ 开头 → None（交常规分发）。
/// - @ 开头但没解析出有效目标/内容 → Some(空) → 提示用法。
/// - rest 首词是会话级指令 → 每个目标一条「指令 N …」；否则整段当内容 → 每个目标一条「发 N …」。
fn parse_at_commands(text: &str) -> Option<Vec<(String, String)>> {
    let mut rest = text.trim();
    if !rest.starts_with('@') {
        return None;
    }
    let mut targets: Vec<String> = Vec::new();
    loop {
        rest = rest.trim_start();
        let Some(r) = rest.strip_prefix('@') else { break };
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break; // 「@abc」不是会话号
        }
        if !targets.contains(&digits) {
            targets.push(digits.clone());
        }
        rest = &r[digits.len()..];
    }
    if targets.is_empty() {
        return Some(vec![]);
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(vec![]);
    }
    let (first, tail) = split_cmd(rest);
    let cmds = if SESSION_CMDS.contains(&first.as_str()) {
        targets
            .iter()
            .map(|n| {
                let arg = if tail.is_empty() { n.clone() } else { format!("{n} {tail}") };
                (first.clone(), arg)
            })
            .collect()
    } else {
        targets.iter().map(|n| ("发".to_string(), format!("{n} {rest}"))).collect()
    };
    Some(cmds)
}

pub(crate) async fn dispatch(
    state: &SharedState,
    username: &str,
    text: &str,
    reply: Option<&ReplyCtx>,
) -> String {
    // 「@N …」速记（多目标 + 全部会话级指令）：逐条 run_command，回复拼接
    if let Some(cmds) = parse_at_commands(text) {
        if cmds.is_empty() {
            return "用法：@号位 接内容或会话指令，可多个。\n\
                    例：@1 @2 重启服务 / @1 排队 / @2 暂停 / @1 撤回"
                .to_string();
        }
        let mut out = Vec::new();
        for (cmd, arg) in cmds {
            out.push(run_command(state, username, &cmd, &arg, reply).await);
        }
        return out.join("\n\n");
    }
    let (cmd, arg) = split_cmd(text);
    run_command(state, username, &cmd, &arg, reply).await
}

/// 单条指令分发（@N 速记逐条走这里，常规消息也走这里）。
async fn run_command(
    state: &SharedState,
    username: &str,
    cmd: &str,
    arg: &str,
    reply: Option<&ReplyCtx>,
) -> String {
    match cmd {
        "帮助" | "help" | "?" | "？" | "菜单" | "" => help_text(),
        "会话" | "列表" | "ls" | "任务" => list_sessions(state, username).await,
        "设备" | "devices" => list_devices(state, username).await,
        "暂停" => control(state, username, arg, ControlAction::Pause, "已暂停").await,
        "恢复" | "继续" => control(state, username, arg, ControlAction::Resume, "已恢复").await,
        "中断" => control(state, username, arg, ControlAction::Interrupt, "已中断").await,
        "终止" | "停止" => control(state, username, arg, ControlAction::Stop, "已终止").await,
        "发" | "发送" | "回复" | "输入" => send_input(state, username, arg, reply).await,
        "排队" | "队列" | "queue" => list_queued(state, username, arg).await,
        "监控" | "watch" => monitor_start(state, username, arg, reply).await,
        "停止监控" | "取消监控" | "结束监控" | "unwatch" => {
            monitor_stop(state, username, arg).await
        }
        "撤回" | "recall" => recall_last(state, username, arg).await,
        "文件" | "附件" | "files" => list_pending_files(state, username).await,
        "清空文件" | "清空附件" | "清空" => clear_pending_files(state, username).await,
        "删除文件" | "删文件" | "删附件" | "删除附件" => {
            remove_pending_file(state, username, arg).await
        }
        _ => format!("未知指令「{cmd}」。发「帮助」看用法。"),
    }
}

/// 渠道收到消息后先按 staffId 找归属账号：
/// - 找到 → Ok(账号)，按该账号身份执行指令 / 收推送；
/// - 没找到（未绑定的钉钉 id）→ Err(登录链接回复)：生成一次性 token、回一段带登录链接的文案，
///   用户登录后带 token 调 bind 接口，把这个 staffId 绑到登录进的账号。
/// `app_owner` = 消息经由的钉钉应用配置账号（推送凭据/robotCode 来源）。
pub(crate) async fn resolve_account(
    state: &SharedState,
    app_owner: &str,
    staff_id: &str,
    robot_code: &str,
    nick: &str,
) -> Result<String, String> {
    if staff_id.is_empty() {
        return Err("拿不到你的钉钉身份（senderStaffId 为空），无法关联账号。".to_string());
    }
    // 顺手把应用的 robotCode 记新（推送要用）
    if !robot_code.is_empty() {
        state.registry.write().await.capture_dingtalk_robot_code(app_owner, robot_code);
    }
    if let Some(account) = state.registry.read().await.dingtalk_user_of(staff_id) {
        return Ok(account);
    }
    // 未绑定 → 生成一次性 token，回登录链接
    let token = crate::state::new_bind_token();
    state.dingtalk_binds.write().await.insert(
        token.clone(),
        crate::state::PendingDingtalkBind {
            staff_id: staff_id.to_string(),
            nick: nick.to_string(),
            app_user: app_owner.to_string(),
            robot_code: robot_code.to_string(),
            at: crate::state::now_secs(),
        },
    );
    let link = format!("{}/?dtbind={token}", crate::server::public_base());
    Err(format!(
        "👋 你的钉钉还没关联 agent-monitor 账号。\n\
         点下面链接登录，即可把当前钉钉号绑定到你的账号（绑定后：任务完成/需要操作会私聊推给你，\
         也能在这直接发指令遥控会话）：\n{link}\n\
         链接 30 分钟内有效。"
    ))
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
    {
        let mut map = state.bot_monitors.write().await;
        let list = map.entry(username.to_string()).or_default();
        // 同一会话重复「监控」→ 覆盖旧的（刷新 webhook/起点），不叠加
        list.retain(|m| m.task_id != id);
        list.push(crate::state::BotMonitor {
            task_id: id.clone(),
            webhook: ctx.webhook.clone(),
            expiry_ms: ctx.expiry_ms,
            last_ts,
        });
    }
    let label = session_label(state, username, &id).await;
    format!(
        "已开始监控{label}，有新内容会自动推到这里（约每 20s，仅推对话内容、跳过执行过程）。\n\
         可同时监控多个；发「停止监控 N」停某个、「停止监控」停全部。\n\
         注：受钉钉会话地址时效/条数限制，长时间监控可能中断，届时再发「监控 N」即可。"
    )
}

/// 把一个会话描述成「会话 N（标题）」，用于监控开始/停止的回执。
async fn session_label(state: &SharedState, username: &str, task_id: &str) -> String {
    let n = session_number(state, username, task_id).await;
    let title = state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .map(|t| {
            if !t.title.is_empty() {
                t.title
            } else if !t.prompt.is_empty() {
                t.prompt
            } else {
                t.project_name
            }
        })
        .unwrap_or_default();
    let title: String = title.chars().take(20).collect();
    match (n, title.is_empty()) {
        (Some(n), false) => format!("会话 {n}（{title}）"),
        (Some(n), true) => format!("会话 {n}"),
        (None, false) => format!("会话「{title}」"),
        (None, true) => "该会话".to_string(),
    }
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

/// 「停止监控 [N]」：带号位停某个会话；不带号位停该用户全部监控。
async fn monitor_stop(state: &SharedState, username: &str, arg: &str) -> String {
    // 不带号位 → 停全部
    if arg.trim().is_empty() {
        let n = state.bot_monitors.write().await.remove(username).map(|v| v.len()).unwrap_or(0);
        return if n > 0 {
            format!("已停止监控全部 {n} 个会话。")
        } else {
            "当前没有在监控的会话。".to_string()
        };
    }
    // 带号位 → 只停该会话
    let id = match resolve_task(state, username, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let removed = remove_monitor(state, username, &id).await;
    let label = session_label(state, username, &id).await;
    if removed {
        format!("已停止监控{label}。")
    } else {
        format!("{label}当前未在监控。")
    }
}

/// 移除某用户对某会话的监控；用户名下监控清空则连键一起删。返回是否真的移除了一条。
async fn remove_monitor(state: &SharedState, username: &str, task_id: &str) -> bool {
    let mut map = state.bot_monitors.write().await;
    let Some(list) = map.get_mut(username) else {
        return false;
    };
    let before = list.len();
    list.retain(|m| m.task_id != task_id);
    let removed = list.len() != before;
    if list.is_empty() {
        map.remove(username);
    }
    removed
}

/// 推进某会话监控的增量游标（已推送到的最后时间戳）。
async fn advance_monitor_ts(state: &SharedState, username: &str, task_id: &str, ts: &str) {
    if let Some(list) = state.bot_monitors.write().await.get_mut(username) {
        if let Some(m) = list.iter_mut().find(|m| m.task_id == task_id) {
            m.last_ts = ts.to_string();
        }
    }
}

/// 监控推送只保留「对话内容」：用户提示、助手回复、方案、待选择；过滤掉执行过程
/// （工具调用/结果、todos、后台任务）——用户要的是对话，不是一屏工具执行流水。
fn is_monitor_content(role: &str) -> bool {
    matches!(role, "user" | "assistant" | "plan" | "select")
}

fn split_cmd(text: &str) -> (String, String) {
    let text = text.trim();
    match text.split_once(char::is_whitespace) {
        Some((c, rest)) => (c.to_string(), rest.trim().to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn help_text() -> String {
    "终端监控机器人 · 指令（N = 会话号位，发「会话」看）\n\
     号位跟着终端窗口固定：终端不关，号就一直是它，可能不连号。\n\
     \n\
     【查看】\n\
     • 会话 —— 列出当前会话（带号位）\n\
     • 设备 —— 列出名下设备\n\
     • 排队 [N] —— 查看排队中的任务（不带 N 汇总全部）\n\
     • 文件 —— 查看挂起待发的文件\n\
     \n\
     【控制会话】\n\
     • 暂停 N / 恢复 N / 中断 N / 终止 N —— 控制 N 号会话\n\
     • 发 N 内容 —— 向 N 号会话发一条输入（排队则回队列，执行后通知）\n\
     • 撤回 N —— 撤回 N 号会话最近一条排队中的任务\n\
     \n\
     【监控】\n\
     • 监控 N —— 把 N 号会话的对话内容持续推到这里（可多个，跳过执行过程）\n\
     • 停止监控 [N] —— 停某个会话；不带号位停全部\n\
     \n\
     【文件】\n\
     • 直接发文件/图片给我 → 暂存，随下一条任务（如「@2 处理这些文件」）落到会话 tmp/ 并把路径拼到开头\n\
     • 删除文件 N —— 删某个；清空文件 —— 全部丢弃\n\
     \n\
     【速记】\n\
     • @N 接内容或任意会话指令 —— @2 重启服务 / @2 暂停 / @2 排队\n\
     • @1 @2 内容 —— 同一任务发给多个会话\n\
     \n\
     • 帮助 —— 显示本说明"
        .to_string()
}

/// 机器人监控推送循环：每 20s 把各监控会话的新增消息推到其钉钉会话 webhook。
/// 只推「起点之后」的增量、批量合一条；webhook 失效或推送被钉钉拒（限流/过期）就停掉该监控。
pub async fn monitor_loop(state: SharedState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let now = crate::state::now_secs() * 1000;
        // 拍平成 (user, 单个监控) —— 每用户可监控多个会话
        let monitors: Vec<(String, crate::state::BotMonitor)> = state
            .bot_monitors
            .read()
            .await
            .iter()
            .flat_map(|(u, ms)| ms.iter().map(|m| (u.clone(), m.clone())).collect::<Vec<_>>())
            .collect();
        for (user, mon) in monitors {
            if mon.expiry_ms > 0 && now >= mon.expiry_ms {
                remove_monitor(&state, &user, &mon.task_id).await;
                let _ = push_webhook(
                    &mon.webhook,
                    "监控已到期（钉钉会话地址时效结束）。如需继续，请再发「监控 N」。",
                )
                .await;
                continue;
            }
            let msgs = state.bot_task_messages(&mon.task_id).await;
            let all_new: Vec<&am_core::model::MessageBrief> = msgs
                .iter()
                .filter(|m| m.timestamp.as_str() > mon.last_ts.as_str())
                .collect();
            if all_new.is_empty() {
                continue;
            }
            // 游标推进到「本轮见到的最后一条」，含被过滤的执行过程 —— 否则下轮反复重扫
            let new_last = all_new.last().map(|x| x.timestamp.clone()).unwrap_or_default();
            // 只推对话内容，跳过执行过程（工具/todos/后台任务）
            let content: Vec<&am_core::model::MessageBrief> =
                all_new.iter().copied().filter(|m| is_monitor_content(&m.role)).collect();
            if content.is_empty() {
                // 本轮全是执行过程：不推送，但推进游标
                advance_monitor_ts(&state, &user, &mon.task_id, &new_last).await;
                continue;
            }
            let text = render_monitor_push(&content);
            match push_webhook(&mon.webhook, &text).await {
                Ok(true) => advance_monitor_ts(&state, &user, &mon.task_id, &new_last).await,
                // 钉钉返回错误（多为会话地址限流/过期）：停掉该会话监控，避免空转刷错误
                Ok(false) => {
                    remove_monitor(&state, &user, &mon.task_id).await;
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

/// 会话的号位（「发 N / 暂停 N」里的 N），用于钉钉推送里带上号让人「@N」回应。
///
/// 按**终端锚**反查而不是 task_id：同一个终端窗口在 /clear、--resume 前后是不同的会话 id，
/// 却该报出同一个号。锚下的代表会话被去重掉时也照样能拿到号。
pub(crate) async fn session_number(
    state: &SharedState,
    username: &str,
    task_id: &str,
) -> Option<usize> {
    let list = sorted_active_tasks(state, username).await;
    // 常见路径：会话本身就在列表里
    if let Some((_, no)) = list.iter().find(|(t, _)| t.id == task_id) {
        return Some(*no as usize);
    }
    // 没命中 = 它被同锚去重掉了（/clear 前后两条会话短暂并存）：报它所在终端的号
    let anchor = state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .map(|t| crate::slots::anchor_of(&t))?;
    list.into_iter().find(|(t, _)| crate::slots::anchor_of(t) == anchor).map(|(_, no)| no as usize)
}

/// 活跃会话 + 各自号位，按「设备名 → 终端 → 项目 → 号位」排序。「会话」列表、「@N / 发 N /
/// 暂停 N」、推送里的 `#N` 全部以它为准。
///
/// 号位来自 [`crate::slots`]：绑定终端窗口（shell pid + start）并落盘，所以它既不随
/// title/prompt 变化漂移，也不随会话增减、hub 重启重排 —— 位置序号那套正是「@2 打到列表
/// 第 5 位」错位的根因。排序也直接用号位，于是同组内号是递增的、好扫视。
///
/// 同一终端锚下若有多个活跃会话（罕见：/clear 后旧会话短暂并存），只留最近活动的那条：
/// 一个终端窗口一个号，否则同号出现两行、用户没法指名。
async fn sorted_active_tasks(
    state: &SharedState,
    username: &str,
) -> Vec<(am_core::model::Task, u32)> {
    let mut tasks = state.tasks_for(username).await;
    // 已结束的会话不列出——机器人只关心还能操作的活跃会话
    tasks.retain(|t| t.status != TaskStatus::Finished);
    // 同锚去重：先按活动时间降序，再按锚首见保留 → 留下的是每个终端最近活动的会话
    tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms).then(a.id.cmp(&b.id)));
    let mut seen = std::collections::HashSet::new();
    tasks.retain(|t| seen.insert(crate::slots::anchor_of(t)));
    // 号位首次分配的顺序 = 用户在列表里看到的分组顺序（全用不随运行时变化的字段），
    // 这样同一分组里新终端拿到的号也是从小到大接着来的
    tasks.sort_by(|a, b| {
        a.hostname
            .cmp(&b.hostname)
            .then(a.provider_dsr.cmp(&b.provider_dsr))
            .then(a.project_name.cmp(&b.project_name))
            .then(a.id.cmp(&b.id))
    });
    let nos = crate::slots::ensure(state, username, &tasks).await;
    // ensure 一定给了号（锚就是从这批会话来的）；真没拿到就宁可不列出，也不显示一个
    // 解析不到的「0.」让用户去发「@0」。
    let mut out: Vec<(am_core::model::Task, u32)> = tasks
        .into_iter()
        .filter_map(|t| nos.get(&crate::slots::anchor_of(&t)).copied().map(|no| (t, no)))
        .collect();
    out.sort_by(|(a, na), (b, nb)| {
        a.hostname
            .cmp(&b.hostname)
            .then(a.provider_dsr.cmp(&b.provider_dsr))
            .then(a.project_name.cmp(&b.project_name))
            .then(na.cmp(nb))
    });
    out
}

async fn list_sessions(state: &SharedState, username: &str) -> String {
    let tasks = sorted_active_tasks(state, username).await;
    if tasks.is_empty() {
        return "当前没有活跃会话。".to_string();
    }
    let mut lines = vec![format!("共 {} 个活跃会话：", tasks.len())];
    let mut cur_dev = String::new();
    let mut cur_group = String::new(); // 终端·项目 子分组
    for (t, no) in &tasks {
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
        // 子标题里已带终端·项目，行内只留状态 + 会话标题
        let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
        let title: String = title.chars().take(24).collect();
        lines.push(format!("  {}. [{}] {}", no, status_zh(t.status), title));
    }
    // 号位绑终端窗口、不随列表刷新重排，所以中间可能有空号（终端关掉了）——那是正常的
    lines.push("\n号位跟着终端窗口固定不变，可能不连号。操作示例：@2 继续 / 暂停 2".to_string());
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
    let n: u32 = arg
        .trim()
        .parse()
        .map_err(|_| "请给会话号位，如「暂停 2」。发「会话」看号位。".to_string())?;
    if n == 0 {
        return Err("号位从 1 开始。发「会话」看号位。".to_string());
    }
    // 号位由终端锚决定并已落盘，这里直接按号反查即可 —— 不再依赖「上次列过什么」，
    // 所以不必先发「会话」，hub 重启也不会让号位改指向。
    let tasks = sorted_active_tasks(state, username).await;
    if tasks.is_empty() {
        return Err("当前没有活跃会话。".to_string());
    }
    tasks
        .into_iter()
        .find(|(_, no)| *no == n)
        .map(|(t, _)| t.id)
        .ok_or(format!("没有 {n} 号会话（终端可能已关）。发「会话」看当前号位。"))
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

/// 归一化文本用于队列比对（折叠空白、去首尾）
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 读某会话当前的排队状态：(hub 待下发队列文本, 终端原生队列文本)。
/// hub 待下发 = 还没被客户端取走的输入；终端原生 = 已注入终端、claude 排队中。
async fn read_queue(
    state: &SharedState,
    username: &str,
    task_id: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    let task = state.tasks_for(username).await.into_iter().find(|t| t.id == task_id)?;
    let terminal_q = task.queued_inputs.clone();
    let machines = state.machines.read().await;
    let hub_pending: Vec<String> = machines
        .get(&task.machine_id)
        .map(|e| {
            e.pending
                .iter()
                .filter(|c| c.task_id == task_id && matches!(c.action, ControlAction::Input))
                .filter_map(|c| c.text.clone())
                .collect()
        })
        .unwrap_or_default();
    Some((hub_pending, terminal_q))
}

/// 下载挂起的钉钉文件并下发到会话项目目录的 tmp/ 下，返回回填用的相对路径 `./tmp/<name>`。
async fn attach_pending_file(
    state: &SharedState,
    username: &str,
    task_id: &str,
    pf: &crate::state::BotPendingFile,
) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    // 下载要用「收到该文件的那个应用」的凭据（多租户下 app_user 可能 != 归属账号）
    let app = state
        .registry
        .read()
        .await
        .dingtalk_app_of(&pf.app_user)
        .ok_or("未配置钉钉应用")?;
    let now_ms = crate::state::now_secs() * 1000;
    let bytes = crate::dingtalk::download_bot_file(&app, &pf.download_code, now_ms).await?;
    let task = state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .ok_or("会话不存在")?;
    let cwd = task.project.trim_end_matches(['/', '\\']).to_string();
    if cwd.is_empty() {
        return Err("会话无项目目录".into());
    }
    let sep = if cwd.contains('\\') { '\\' } else { '/' };
    // 该项目配置的接收目录；未配置则默认 `<cwd>/tmp`。配置值可为绝对路径或相对(相对项目)。
    let configured = state.registry.read().await.dingtalk_recv_dir(username, &cwd);
    let dir = match configured {
        Some(d) if d.starts_with('/') || d.contains(":\\") => d, // 绝对路径直接用
        Some(d) => format!("{cwd}{sep}{}", d.trim_matches(['/', '\\'])), // 相对项目
        None => format!("{cwd}{sep}tmp"),
    };
    // 只留 basename，防路径穿越
    let safe = std::path::Path::new(&pf.file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "file.bin".into());
    let target = format!("{}{sep}{safe}", dir.trim_end_matches(['/', '\\']));
    let mut machines = state.machines.write().await;
    let entry = machines.get_mut(&task.machine_id).ok_or("会话所属设备已离线")?;
    entry.pending_files.push_back(am_core::model::FileTransfer {
        dir,
        filename: safe.clone(),
        content_b64: B64.encode(&bytes),
    });
    // 回填路径：目标在项目目录内 → 用相对 `./子路径`，否则用绝对路径（Claude 才找得到）。
    let rel = target
        .strip_prefix(&format!("{cwd}{sep}"))
        .map(|r| format!("./{}", r.replace('\\', "/")))
        .unwrap_or(target);
    Ok(rel)
}

/// 「文件」：列出当前挂起待发的文件（随下一条任务一起落到会话目录）。
async fn list_pending_files(state: &SharedState, username: &str) -> String {
    let files = state.bot_pending_files.read().await.get(username).cloned().unwrap_or_default();
    if files.is_empty() {
        return "当前没有挂起待发的文件。发文件/图片给我即可暂存，随下一条任务一起发出。".to_string();
    }
    let mut lines = vec![format!("📎 待发文件（{} 个，随下一条任务发出）：", files.len())];
    for (i, f) in files.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, f.file_name));
    }
    lines.push("发「删除文件 N」删某个、「清空文件」清空。".to_string());
    lines.join("\n")
}

/// 「清空文件」：丢弃全部挂起待发文件。
async fn clear_pending_files(state: &SharedState, username: &str) -> String {
    let n = state.bot_pending_files.write().await.remove(username).map(|v| v.len()).unwrap_or(0);
    if n > 0 {
        format!("已清空 {n} 个待发文件。")
    } else {
        "当前没有挂起待发的文件。".to_string()
    }
}

/// 「删除文件 N」：删掉第 N 个挂起待发文件（序号以「文件」列出的为准）。
async fn remove_pending_file(state: &SharedState, username: &str, arg: &str) -> String {
    let n: usize = match arg.trim().parse() {
        Ok(n) if n >= 1 => n,
        _ => return "用法：删除文件 <序号>，如「删除文件 2」。发「文件」看序号。".to_string(),
    };
    let mut map = state.bot_pending_files.write().await;
    let Some(list) = map.get_mut(username) else {
        return "当前没有挂起待发的文件。".to_string();
    };
    if n > list.len() {
        return format!("没有第 {n} 个文件，发「文件」看列表。");
    }
    let removed = list.remove(n - 1);
    let remaining = list.len();
    if list.is_empty() {
        map.remove(username);
    }
    format!("已删除「{}」。剩 {remaining} 个待发文件。", removed.file_name)
}

async fn send_input(
    state: &SharedState,
    username: &str,
    arg: &str,
    reply: Option<&ReplyCtx>,
) -> String {
    let (idx, mut text) = split_cmd(arg);
    if text.is_empty() {
        return "用法：发 <号位> <内容>，如「发 2 继续」。发「会话」看号位。".to_string();
    }
    let task_id = match resolve_task(state, username, &idx).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    // 挂起待发文件（可多个）：随本条任务落到会话目录，相对路径按序拼到任务开头（空格隔开）。
    // 超 20 分钟没跟任务的挂起文件视为过期，丢弃不附。
    let pending = state.bot_pending_files.write().await.remove(username).unwrap_or_default();
    let mut rels: Vec<String> = Vec::new();
    for pf in &pending {
        if crate::state::now_secs().saturating_sub(pf.at) > 20 * 60 {
            continue;
        }
        match attach_pending_file(state, username, &task_id, pf).await {
            Ok(rel) => rels.push(rel),
            Err(e) => return format!("附带文件下发失败：{e}"),
        }
    }
    if !rels.is_empty() {
        text = format!("{} {text}", rels.join(" "));
    }
    if let Err(e) =
        queue_command(state, username, &task_id, ControlAction::Input, Some(text.clone())).await
    {
        return e;
    }

    // 即时回执，不阻塞用户；是否排队/已执行由后台判定后经 sessionWebhook 再推一条。
    // 没有 webhook（罕见）时退回一句简单确认。
    match reply {
        Some(ctx) if !ctx.webhook.is_empty() => {
            let st = state.clone();
            let (wh, exp, user, tid, txt, i) = (
                ctx.webhook.clone(),
                ctx.expiry_ms,
                username.to_string(),
                task_id.clone(),
                text.clone(),
                idx.clone(),
            );
            tokio::spawn(async move { confirm_and_watch(st, wh, exp, user, tid, txt, i).await });
            format!("📤 已下发到会话 {idx}：{text}\n确认排队/执行中，稍后通知…")
        }
        _ => format!("已发送到会话 {idx}：{text}"),
    }
}

/// 后台判定「排队 or 已执行」并经 sessionWebhook 推结果；若排队，继续监控到执行为止再推一条。
async fn confirm_and_watch(
    state: SharedState,
    webhook: String,
    expiry_ms: u64,
    username: String,
    task_id: String,
    text: String,
    idx: String,
) {
    let tn = norm(&text);
    // 判定阶段：轮询最多 ~9s，等客户端取走并上报回队列状态（活跃机约 1.5s 一轮）
    let mut queued_list: Option<Vec<String>> = None;
    for _ in 0..6 {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        if expiry_ms > 0 && crate::state::now_secs() * 1000 >= expiry_ms {
            return;
        }
        let Some((hub_pending, term_q)) = read_queue(&state, &username, &task_id).await else {
            continue;
        };
        if term_q.iter().any(|t| norm(t) == tn) || hub_pending.iter().any(|t| norm(t) == tn) {
            let mut list = term_q.clone();
            list.extend(hub_pending);
            queued_list = Some(list);
            break;
        }
    }
    match queued_list {
        None => {
            let _ = push_webhook(&webhook, &format!("✅ 已执行（会话 {idx}）：{text}")).await;
        }
        Some(list) => {
            let mut lines = vec![format!("⏳ 已排队（会话 {idx}），暂未执行。当前排队：")];
            for (n, t) in list.iter().enumerate() {
                let mark = if norm(t) == tn { " ← 本条" } else { "" };
                lines.push(format!("{}. {}{}", n + 1, t, mark));
            }
            lines.push("被终端接收执行后会再通知你。发「排队 N」可随时查看。".to_string());
            let _ = push_webhook(&webhook, &lines.join("\n")).await;
            // 继续监控到它被纳入执行
            watch_dequeue(state, webhook, expiry_ms, username, task_id, text, idx).await;
        }
    }
}

/// OTO 主动私聊给某账号绑定的所有钉钉 id（网页/客户端下发的状态推送用；无 sessionWebhook 可回）。
async fn push_oto_owner(state: &SharedState, owner: &str, text: &str) {
    let now_ms = crate::state::now_secs() * 1000;
    for (staff_id, app_user) in state.registry.read().await.dingtalk_ids_of(owner) {
        let Some(app) = state.registry.read().await.dingtalk_app_of(&app_user) else {
            continue;
        };
        if !app.app_key.is_empty() && !app.app_secret.is_empty() {
            let _ = crate::dingtalk::push_oto(&app, &staff_id, text, None, now_ms).await;
        }
    }
}

/// 网页/客户端（非钉钉）下发任务后，把「排队中 / 执行中」状态主动推到钉钉（OTO 私聊）。
/// 判定同 confirm_and_watch，但用 OTO 而非 sessionWebhook；排队的还会盯到执行后再推一条。
pub(crate) async fn notify_web_dispatch(
    state: SharedState,
    owner: String,
    task_id: String,
    text: String,
) {
    let tn = norm(&text);
    let snippet: String = text.chars().take(200).collect();
    // 判定阶段：轮询 ~9s，等客户端取走并上报回队列状态
    let mut queued_list: Option<Vec<String>> = None;
    for _ in 0..6 {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let Some((hub_pending, term_q)) = read_queue(&state, &owner, &task_id).await else {
            continue;
        };
        if term_q.iter().any(|t| norm(t) == tn) || hub_pending.iter().any(|t| norm(t) == tn) {
            let mut list = term_q.clone();
            list.extend(hub_pending);
            queued_list = Some(list);
            break;
        }
    }
    let no = session_number(&state, &owner, &task_id)
        .await
        .map(|x| format!("#{x} "))
        .unwrap_or_default();
    match queued_list {
        None => {
            push_oto_owner(
                &state,
                &owner,
                &format!("**▶️ 任务执行中**（网页下发 · 会话 {no}）\n\n{snippet}"),
            )
            .await;
        }
        Some(list) => {
            let mut lines =
                vec![format!("**⏳ 任务已排队**（网页下发 · 会话 {no}）\n\n{snippet}\n\n当前排队：")];
            for (n, t) in list.iter().enumerate() {
                let mark = if norm(t) == tn { " ← 本条" } else { "" };
                lines.push(format!("{}. {}{}", n + 1, t, mark));
            }
            push_oto_owner(&state, &owner, &lines.join("\n")).await;
            // 盯到它被纳入执行
            for _ in 0..360 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let Some((hub_pending, term_q)) = read_queue(&state, &owner, &task_id).await else {
                    return;
                };
                let still =
                    term_q.iter().any(|t| norm(t) == tn) || hub_pending.iter().any(|t| norm(t) == tn);
                if !still {
                    push_oto_owner(
                        &state,
                        &owner,
                        &format!("**▶️ 排队任务已开始执行**（网页下发 · 会话 {no}）\n\n{snippet}"),
                    )
                    .await;
                    return;
                }
            }
        }
    }
}

/// 监控某条排队输入，等它离开队列（被终端纳入执行）后经 sessionWebhook 主动推一条。
async fn watch_dequeue(
    state: SharedState,
    webhook: String,
    expiry_ms: u64,
    username: String,
    task_id: String,
    text: String,
    idx: String,
) {
    let tn = norm(&text);
    // 最多盯 30 分钟；webhook 过期就停
    for _ in 0..360 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if expiry_ms > 0 && crate::state::now_secs() * 1000 >= expiry_ms {
            return;
        }
        let Some((hub_pending, term_q)) = read_queue(&state, &username, &task_id).await else {
            // 会话消失（结束）：别再盯
            return;
        };
        let still = term_q.iter().any(|t| norm(t) == tn) || hub_pending.iter().any(|t| norm(t) == tn);
        if !still {
            let _ = push_webhook(
                &webhook,
                &format!("▶️ 排队任务已开始执行（会话 {idx}）：{text}"),
            )
            .await;
            return;
        }
    }
}

/// 「排队 [N]」：查看排队中的任务。给了 N 看该会话；没给就汇总所有有排队的会话。
async fn list_queued(state: &SharedState, username: &str, arg: &str) -> String {
    let arg = arg.trim();
    if !arg.is_empty() {
        let task_id = match resolve_task(state, username, arg).await {
            Ok(id) => id,
            Err(e) => return e,
        };
        let Some((hub_pending, term_q)) = read_queue(state, username, &task_id).await else {
            return "会话不存在。".to_string();
        };
        let mut list = term_q.clone();
        list.extend(hub_pending);
        if list.is_empty() {
            return format!("会话 {arg} 当前没有排队中的任务。");
        }
        let mut lines = vec![format!("会话 {arg} 排队中（{} 条）：", list.len())];
        for (n, t) in list.iter().enumerate() {
            lines.push(format!("{}. {}", n + 1, t));
        }
        return lines.join("\n");
    }
    // 汇总：按「会话」号位遍历，列出各会话的排队
    let tasks = sorted_active_tasks(state, username).await;
    let mut out: Vec<String> = Vec::new();
    for (t, no) in tasks.iter() {
        if let Some((hub_pending, term_q)) = read_queue(state, username, &t.id).await {
            let mut list = term_q.clone();
            list.extend(hub_pending);
            if !list.is_empty() {
                let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
                let title: String = title.chars().take(20).collect();
                out.push(format!("【{}. {}】{} 条：", no, title, list.len()));
                for (n, x) in list.iter().enumerate() {
                    out.push(format!("  {}. {}", n + 1, x));
                }
            }
        }
    }
    if out.is_empty() {
        "当前没有任何排队中的任务。".to_string()
    } else {
        format!("排队中的任务：\n{}", out.join("\n"))
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
    use super::{parse_at_commands, split_cmd};

    #[test]
    fn split_command() {
        assert_eq!(split_cmd("会话"), ("会话".into(), "".into()));
        assert_eq!(split_cmd("暂停 3"), ("暂停".into(), "3".into()));
        assert_eq!(split_cmd("发 2 继续执行"), ("发".into(), "2 继续执行".into()));
    }

    #[test]
    fn at_commands() {
        let c = |s: &str| parse_at_commands(s);
        // @N + 内容 → 发 N 内容
        assert_eq!(c("@2 重启服务"), Some(vec![("发".into(), "2 重启服务".into())]));
        assert_eq!(c("@2重启服务"), Some(vec![("发".into(), "2 重启服务".into())]));
        // @N + 会话级指令（含排队）→ 指令 N
        assert_eq!(c("@2 暂停"), Some(vec![("暂停".into(), "2".into())]));
        assert_eq!(c("@2 排队"), Some(vec![("排队".into(), "2".into())]));
        assert_eq!(c("@2 撤回"), Some(vec![("撤回".into(), "2".into())]));
        // 多目标：同一内容/指令下发到多个会话
        assert_eq!(
            c("@1 @2 重启服务"),
            Some(vec![("发".into(), "1 重启服务".into()), ("发".into(), "2 重启服务".into())])
        );
        assert_eq!(
            c("@1 @2 暂停"),
            Some(vec![("暂停".into(), "1".into()), ("暂停".into(), "2".into())])
        );
        // 去重目标
        assert_eq!(c("@1 @1 x"), Some(vec![("发".into(), "1 x".into())]));
        // 非 @ → None（走常规分发）；无效目标/空 → Some(空)（提示用法）
        assert_eq!(c("发 2 继续"), None);
        assert_eq!(c("@abc"), Some(vec![]));
        assert_eq!(c("@2"), Some(vec![]));
    }
}
