//! 机器人指令网关：每个用户在前台自助接入自己的钉钉企业应用，
//! 用文字指令遥控自己的会话（查看/暂停/恢复/中断/终止/发布输入）。
//!
//! 路由靠回调 URL 里的 channel：`/monitor/int/dingtalk/<channel>`。
//! channel 反查到配置所属用户 → 指令即以该用户身份执行（URL 即绑定，无需绑定码）。

use crate::state::{BotBatch, SharedState};
use crate::dingtalk;
use am_core::model::{ControlAction, ControlCmd, TaskStatus};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value};

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
    let nick = payload.get("senderNick").and_then(Value::as_str).unwrap_or("");
    // 绑定指令要抢在认人之前：需要它的人正是还认不出来的那个
    let reply = if let Some(r) = try_bind_command(&state, &ctx.staff_id, nick, &content).await {
        r
    } else {
        match resolve_account(&state, &app_owner, &ctx.staff_id, &ctx.robot_code, nick).await {
            Ok(account) => dispatch(&state, &account, &content, Some(&ctx)).await,
            Err(guide) => guide,
        }
    };
    // 同步回复：钉钉直接把响应体当作机器人回复消息
    Json(json!({ "msgtype": "text", "text": { "content": reply } }))
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
    "暂停", "恢复", "中断", "终止", "停止", "撤回", "监控", "watch", "排队", "队列", "queue",
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
    // 只发「@9」不带内容 = 把连续对话切到 9 号（之后不带 @ 的文本都投给它）。
    // 多目标时没有「当前会话」可言，退回用法提示。
    if rest.is_empty() {
        return match targets.as_slice() {
            [n] => Some(vec![("锁定".to_string(), n.clone())]),
            _ => Some(vec![]),
        };
    }
    let (first, tail) = split_cmd(rest);
    // 首词是会话指令、**且后面没有别的内容**时才当指令。这些指令都不吃额外参数（序号已经由
    // `@N` 给出），所以「@3 暂停」是暂停会话，而「@3 暂停一下再继续」是发一条任务 ——
    // 「暂停 / 停止」这类词也是很自然的任务开头，只看首词会把正文整条吞掉。
    // （注：「继续」已不作指令 —— 它是最常见的「让 agent 接着做」输入，一律当内容发。）
    let cmds = if SESSION_CMDS.contains(&first.as_str()) && tail.is_empty() {
        targets.iter().map(|n| (first.clone(), n.clone())).collect()
    } else {
        targets.iter().map(|n| ("发".to_string(), format!("{n} {rest}"))).collect()
    };
    Some(cmds)
}

/// `run_command` 认识的全部一级指令词（含别名）。**在 run_command 里新增指令时必须同步这里。**
///
/// 只服务于钉钉合并窗口的豁免判断（[`is_immediate`]）：命中的消息立即执行、不进窗口。
/// 漏加一个词的后果是那条指令可能被并进同批内容里、当成正文发进终端 —— 宁可多列，别漏。
/// 刻意没有复用 run_command 的 match：那边靠「不认识就返回 None 且无副作用」来试探，
/// 试探本身会把认识的指令执行掉，没法用来做「要不要攒着」的前置判断。
const ALL_CMDS: &[&str] = &[
    "帮助", "help", "?", "？", "菜单",
    "会话", "列表", "ls", "任务",
    "设备", "devices",
    "暂停", "恢复", "中断", "终止", "停止",
    "发", "发送", "回复", "输入",
    "排队", "队列", "queue",
    "监控", "watch",
    "停止监控", "取消监控", "结束监控", "unwatch",
    "撤回", "recall",
    "锁定",
    "历史", "history",
    "文件", "附件", "files",
    "清空文件", "清空附件", "清空",
    "删除文件", "删文件", "删附件", "删除附件",
];

/// 连续对话冷却后的确认词（见 [`sticky_send`]）：同样必须立即执行 ——
/// 被并进内容里，那句「回『确认』即发出」就永远等不到确认了。
const CONFIRM_WORDS: &[&str] = &["确认", "确定", "是", "y", "Y", "ok", "OK"];

/// 这条钉钉消息该立即执行，还是先进合并窗口攒着？
///
/// 判据只有一条：**它是不是指令**。指令的语义依赖「单独成条」——「@2 暂停」跟后面一条内容
/// 拼在一起，`parse_at_commands` 见 tail 非空就整段当内容，暂停指令当场消失。内容则相反：
/// 逐条转发的那几条本就该拼成一段，agent 才能一次看全（否则第一条就带着它开跑了）。
pub(crate) fn is_immediate(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || CONFIRM_WORDS.contains(&t) {
        return true;
    }
    // 「@N …」：解析成会话级指令（暂停/撤回/锁定…）才算指令；
    // 「@N 正文」解析出来的是「发」，那是内容，要参与合并。
    if let Some(cmds) = parse_at_commands(t) {
        // 空 = @ 用法错误，立即回提示，别攒
        return cmds.is_empty() || cmds.iter().any(|(c, _)| c != "发");
    }
    let (cmd, _) = split_cmd(t);
    ALL_CMDS.contains(&cmd.as_str())
}

/// 把一条内容消息投进钉钉合并窗口，返回本次的世代号（交给 [`batch_flush`] 比对）。
///
/// 为什么要攒：钉钉逐条转发给机器人的是几次**完全独立**的回调，payload 里没有转发标记、
/// 没有批次号、也没有「共 N 条」—— hub 无从知道一批有几条，只能拿「消息是连着到的」当判据。
///
/// **必须在收帧循环里按到达顺序同步调用，不能挪进 spawn 的任务里。** 任务的启动顺序由
/// tokio 调度决定，与消息到达顺序无关。入队一旦放进任务里，就会出现这种局面：一批四条
/// 转发，第三条的任务起晚了一步，1/2/4 先攒齐、窗口到期、合并下发，它才 push 进来自成
/// 一批单独发出 —— 用户看到的是「明明一起转发的，却有一条被单独下发」，而且顺序还是跳的。
/// 线上抓到过一次（合并 3 条 + 「一起显示」单独一条）。
pub(crate) async fn batch_push(
    state: &SharedState,
    username: &str,
    text: &str,
    ctx: &ReplyCtx,
) -> u64 {
    {
        let mut map = state.bot_pending_batch.write().await;
        let b = map.entry(username.to_string()).or_insert_with(|| BotBatch {
            lines: Vec::new(),
            webhook: String::new(),
            expiry_ms: 0,
            staff_id: String::new(),
            robot_code: String::new(),
            gen: 0,
        });
        b.lines.push(text.to_string());
        // 回执地址取最新的一条：窗口 3s 远短于 sessionWebhook 的有效期，用哪条都行，
        // 用最新的最稳妥（前面几条离过期更近）。
        b.webhook = ctx.webhook.clone();
        b.expiry_ms = ctx.expiry_ms;
        b.staff_id = ctx.staff_id.clone();
        b.robot_code = ctx.robot_code.clone();
        b.gen += 1;
        b.gen
    }
}

/// 等满 [`BOT_BATCH_WINDOW_MS`] 后把这一批合并成一段、一次性下发。
///
/// 返回 `Some(回执)` = 这一批已到期并发出，由本次调用负责回复；
/// 返回 `None` = 窗口被后来的消息重置了，本次静默退场，改由最后那条负责回。
pub(crate) async fn batch_flush(
    state: &SharedState,
    username: &str,
    my_gen: u64,
) -> Option<String> {
    tokio::time::sleep(std::time::Duration::from_millis(crate::state::BOT_BATCH_WINDOW_MS)).await;

    let batch = {
        let mut map = state.bot_pending_batch.write().await;
        // 世代号变了 = 睡着的这 3s 里又来了消息，窗口被它重置 —— 那批由它 flush
        match map.get(username) {
            Some(b) if b.gen == my_gen => map.remove(username)?,
            _ => return None,
        }
    };

    let n = batch.lines.len();
    let merged = batch.lines.join("\n");
    let ctx = ReplyCtx {
        webhook: batch.webhook,
        expiry_ms: batch.expiry_ms,
        staff_id: batch.staff_id,
        robot_code: batch.robot_code,
    };
    let reply = dispatch(state, username, &merged, Some(&ctx)).await;
    // 单条时行为与合并前完全一致（只是晚了一个窗口），不必多嘴
    Some(if n > 1 { format!("✅ 已合并 {n} 条\n{reply}") } else { reply })
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
            let r = run_command(state, username, &cmd, &arg, reply).await;
            out.push(r.unwrap_or_else(|| format!("未知指令「{cmd}」。发「帮助」看用法。")));
        }
        return out.join("\n\n");
    }
    let (cmd, arg) = split_cmd(text);
    // 先当指令试 —— 不认识时 run_command 返回 None 且不产生任何副作用。
    // 指令照常执行**且不解除锁定**：中途查个「会话」「排队」不该打断对话。
    if let Some(out) = run_command(state, username, &cmd, &arg, reply).await {
        return out;
    }
    // 不是指令 → 连续对话：投给锁定的会话，不用每条都带 @
    sticky_send(state, username, text, reply).await
}

/// 把一条普通文本投给「连续对话」锁定的会话。
///
/// 锁定冷却（久未对话）时不直接下发：先回一句「当前锁的是 N 号」并把内容暂存，用户回
/// 「确认」即发出 —— 隔了小半天随手发一句，很容易忘了当前锁着哪个终端。
async fn sticky_send(
    state: &SharedState,
    username: &str,
    text: &str,
    reply: Option<&ReplyCtx>,
) -> String {
    let Some(n) = crate::slots::sticky_of(state, username).await else {
        return "未知指令。发「帮助」看用法，或用「@号位 内容」下发任务。".to_string();
    };
    // 冷却后的第一条：确认流程
    if crate::slots::sticky_cooled(state, username).await {
        let confirming = matches!(text.trim(), "确认" | "确定" | "是" | "y" | "Y" | "ok" | "OK");
        let pending = state.bot_sticky_pending.write().await.remove(username);
        match (confirming, pending) {
            // 回「确认」→ 发暂存的那条（20 分钟内有效），省得重打一遍
            (true, Some((held, at))) if crate::state::now_secs().saturating_sub(at) < 20 * 60 => {
                return send_input(state, username, &format!("{n} {held}"), reply).await;
            }
            // 回「确认」但没有有效暂存 → 只解除冷却，等下一条内容
            (true, _) => {
                crate::slots::set_sticky(state, username, n).await;
                return format!("好的，继续对话会话 {n}，直接发内容即可。");
            }
            // 其它内容：说明用户已看过提示、知道在跟谁说话 → 直接发（不再要求确认）
            (false, Some(_)) => {
                return send_input(state, username, &format!("{n} {text}"), reply).await;
            }
            // 冷却后的第一条内容 → 暂存 + 提示确认
            (false, None) => {}
        }
        let now = crate::state::now_secs();
        state
            .bot_sticky_pending
            .write()
            .await
            .insert(username.to_string(), (text.to_string(), now));
        let label = sticky_label(state, username, n).await;
        let preview = one_line(text, 40);
        return format!(
            "⏸ 距上次对话已有一段时间，先确认下目标会话：\n\
             当前锁定 {label}\n\
             待发内容：{preview}\n\n\
             确认无误回「确认」即发出；要换会话发「@号位」；发「会话」看列表。"
        );
    }
    send_input(state, username, &format!("{n} {text}"), reply).await
}

/// 「N 号（项目 · 标题）」——冷却确认时用，让人一眼认出是哪个终端
async fn sticky_label(state: &SharedState, username: &str, no: u32) -> String {
    let Ok(task_id) = resolve_task(state, username, &no.to_string()).await else {
        return format!("{no} 号（该会话可能已结束）");
    };
    state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .map(|t| {
            let s = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
            format!("{no} 号（{} · {}）", t.project_name, one_line(&s, 24))
        })
        .unwrap_or_else(|| format!("{no} 号"))
}

/// 单条指令分发（@N 速记逐条走这里，常规消息也走这里）。
///
/// 返回 `None` = 这个词不是指令 —— 由调用方决定拿它怎么办（连续对话时当内容发给锁定的
/// 会话，否则回「未知指令」）。**不认识时不产生任何副作用**，所以可以先试着当指令跑。
/// 刻意用返回值而不是另维护一份「已知指令清单」：清单和 match 分支迟早会不同步，届时新加的
/// 指令会被当成聊天内容直接发进终端。
async fn run_command(
    state: &SharedState,
    username: &str,
    cmd: &str,
    arg: &str,
    reply: Option<&ReplyCtx>,
) -> Option<String> {
    Some(match cmd {
        "帮助" | "help" | "?" | "？" | "菜单" | "" => help_text(),
        "会话" | "列表" | "ls" | "任务" => list_sessions(state, username).await,
        "设备" | "devices" => list_devices(state, username).await,
        "暂停" => control(state, username, arg, ControlAction::Pause, "已暂停").await,
        "恢复" => control(state, username, arg, ControlAction::Resume, "已恢复").await,
        "中断" => control(state, username, arg, ControlAction::Interrupt, "已中断").await,
        "终止" | "停止" => control(state, username, arg, ControlAction::Stop, "已终止").await,
        "发" | "发送" | "回复" | "输入" => send_input(state, username, arg, reply).await,
        "排队" | "队列" | "queue" => list_queued(state, username, arg).await,
        "监控" | "watch" => monitor_start(state, username, arg, reply).await,
        "停止监控" | "取消监控" | "结束监控" | "unwatch" => {
            monitor_stop(state, username, arg).await
        }
        "撤回" | "recall" => recall_last(state, username, arg).await,
        "锁定" => lock_session(state, username, arg).await,
        "历史" | "history" => list_history(state, username, arg).await,
        "文件" | "附件" | "files" => list_pending_files(state, username).await,
        "清空文件" | "清空附件" | "清空" => clear_pending_files(state, username).await,
        "删除文件" | "删文件" | "删附件" | "删除附件" => {
            remove_pending_file(state, username, arg).await
        }
        _ => return None,
    })
}

/// 待绑定链接 / 绑定码的有效期：30 分钟够走完「收到链接 → 登录 → 绑定」。
pub const BIND_TOKEN_TTL_SECS: u64 = 30 * 60;

/// 「绑定 <码>」：把发消息的这个钉钉号，绑到取码的那个账号。
///
/// **必须在认人之前拦下**：需要它的人恰恰是还没绑定、认不出来的那个 ——
/// 走到 resolve_account 只会拿回一句「你还没关联账号」，指令永远没机会执行。
///
/// 返回 Some(回复) 表示这条消息是绑定指令、已处理完；None 表示不是，继续正常流程。
pub(crate) async fn try_bind_command(
    state: &SharedState,
    staff_id: &str,
    nick: &str,
    text: &str,
) -> Option<String> {
    let t = text.trim();
    let code = ["绑定", "bind", "綁定"]
        .iter()
        .find_map(|p| t.strip_prefix(*p))?
        .trim()
        .to_uppercase();
    if code.is_empty() {
        return Some(
            "用法：绑定 <码>\n码在网页或客户端的「机器人管理」里取（也可扫码直接得到这条指令）。"
                .to_string(),
        );
    }
    if staff_id.is_empty() {
        return Some("拿不到你的钉钉身份，无法绑定。".to_string());
    }
    let now = crate::state::now_secs();
    // 取码即用掉：成功与否都移除，避免一个码被反复试
    let entry = state.dingtalk_bind_codes.write().await.remove(&code);
    let Some(pending) = entry else {
        return Some("绑定码无效或已过期，请在「机器人管理」里重新取一个。".to_string());
    };
    if now.saturating_sub(pending.at) >= BIND_TOKEN_TTL_SECS {
        return Some("绑定码已过期，请在「机器人管理」里重新取一个。".to_string());
    }
    state.registry.write().await.bind_dingtalk_id(staff_id, &pending.user, nick);
    Some(format!(
        "✅ 已绑定到账号「{}」。\n之后任务完成 / 需要你决定时会私聊推给你，也能在这直接发指令遥控会话。\n发「帮助」看用法。",
        pending.user
    ))
}

/// 消息归属：两种机器人，两套认人方式。
///
/// - **个人机器人**（用户自己在前台配的）：谁配的就归谁，不看发信人是谁。
/// - **全局机器人**（管理员配的那一个，服务所有人）：只能靠发信人的 staffId
///   认出他是谁；没绑过就回一段引导（登录链接 / 绑定码两条路都给）。
///
/// 顺带记下 robotCode 与「对面是谁」，主动推送要用。
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
    let is_global = state.registry.read().await.is_global_dingtalk_app(app_owner);
    if !is_global {
        // 个人机器人：记下对面是谁（推送要用），消息直接归应用主人
        state.registry.write().await.capture_dingtalk_peer(app_owner, robot_code, staff_id);
        return Ok(app_owner.to_string());
    }
    // 全局机器人：robotCode 仍要记（推送用），但收件人由各自的绑定决定
    state.registry.write().await.capture_dingtalk_peer(app_owner, robot_code, "");
    if let Some(account) = state.registry.read().await.dingtalk_user_of(staff_id) {
        return Ok(account);
    }
    // 全局机器人也是**它主人自己的**机器人：他没道理还要先给自己绑一次钉钉号。
    // staff_id 对得上（或还没认过主人，即他刚配好第一次说话）就直接归他。
    {
        let mut reg = state.registry.write().await;
        let owner_staff = reg.dingtalk_app_of(app_owner).map(|a| a.staff_id).unwrap_or_default();
        if owner_staff.is_empty() || owner_staff == staff_id {
            reg.capture_dingtalk_peer(app_owner, robot_code, staff_id);
            return Ok(app_owner.to_string());
        }
    }
    // 未绑定 → 回引导。**同一个人反复发消息要给同一个链接**：否则他每说一句就收到
    // 一个新链接，不知道该点哪个；待绑定表里也会堆一串等价项。
    let now = crate::state::now_secs();
    let existing = state
        .dingtalk_binds
        .read()
        .await
        .iter()
        .find(|(_, p)| p.staff_id == staff_id && now.saturating_sub(p.at) < BIND_TOKEN_TTL_SECS)
        .map(|(t, _)| t.clone());
    let token = match existing {
        Some(t) => t,
        None => {
            let t = crate::state::new_bind_token();
            state.dingtalk_binds.write().await.insert(
                t.clone(),
                crate::state::PendingDingtalkBind {
                    staff_id: staff_id.to_string(),
                    nick: nick.to_string(),
                    at: now,
                },
            );
            t
        }
    };
    let link = format!("{}/?dtbind={token}", crate::server::public_base());
    Err(format!(
        "👋 你的钉钉还没关联 agent-monitor 账号，两种方式任选：

         ① 点链接登录即绑定（30 分钟内有效）：
{link}

         ② 在网页/客户端「机器人管理」里取一个绑定码，回来发「绑定 <码>」

         绑定后：任务完成 / 需要你决定时会私聊推给你，也能在这直接发指令遥控会话。"
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
    let title = one_line(&title, 20);
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
    match recall_input(state, username, &task_id).await {
        Ok(Recalled::FromHubQueue) => format!("已撤回排队中的任务（会话 {arg}）。"),
        Ok(Recalled::InjectedUpKey) => {
            format!("已注入撤回 ↑（会话 {arg}）。Terminal.app 需在终端手动按 ↑。")
        }
        Err(e) => e,
    }
}

/// 撤回结果：撤的是 hub 队列里还没下发的，还是已进终端、只能注入 ↑
pub(crate) enum Recalled {
    FromHubQueue,
    InjectedUpKey,
}

/// 撤回该会话最近一条排队中的输入。
///
/// 两级：**先撤 hub 队列里还没被客户端取走的**（直接删掉即可，干净），撤不到才说明它已经进了
/// 终端原生队列，只能注入 ↑ 让终端自己退。顺序不能反 —— 若 hub 侧还压着一条却去按 ↑，
/// 动到的是终端里**另一条**已排队的输入，等于撤错了人。
pub(crate) async fn recall_input(
    state: &SharedState,
    username: &str,
    task_id: &str,
) -> Result<Recalled, String> {
    let task = state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .ok_or("会话不存在（可能已结束）。")?;
    let mut machines = state.machines.write().await;
    let entry = machines.get_mut(&task.machine_id).ok_or("会话所属设备已离线。")?;
    let pos = entry.pending.iter().rposition(|c| {
        c.task_id == task_id && matches!(c.action, am_core::model::ControlAction::Input)
    });
    if let Some(i) = pos {
        entry.pending.remove(i);
        return Ok(Recalled::FromHubQueue);
    }
    entry.pending.push_back(ControlCmd {
        task_id: task_id.to_string(),
        pid: task.pid,
        action: am_core::model::ControlAction::TermKey,
        text: Some("up:1".to_string()),
        id: None,
        from_select: false,
    });
    Ok(Recalled::InjectedUpKey)
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
     • 历史 [N] —— 回看最近结束的会话及其结果（默认 5 条）\n\
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
     【速记 / 连续对话】\n\
     • @N 接内容或任意会话指令 —— @2 重启服务 / @2 暂停 / @2 排队\n\
     • 发过一次 @N 后，直接发内容就一直发给它，不用再带 @\n\
     • @N（单独发）—— 切换到 N 号继续对话\n\
     • 查指令（帮助/会话/排队…）不会打断对话，之后继续直接发即可\n\
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

/// 单行化 + 截断。**会话标题来自用户的首条提示词，很可能是多行的**，直接嵌进
/// 一行文案会把那行撕成两段：漏出去的第二段在微信那种「单换行被当软换行」的渲染里
/// 还会黏到下一行标题上 —— 实际见过「二级弹 —— 📱 MacBook Pro ——」这种。
///
/// 先压平再截断，顺序不能反：否则 24 字的额度会被换行和多余空白吃掉，
/// 看得见的内容不足 24 字。
fn one_line(s: &str, limit: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(limit).collect()
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
pub(crate) async fn sorted_active_tasks(
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
    // 当前连续对话锁定的号位：列表里标出来，免得「不带 @ 直接发」时不知道会进哪个终端
    let sticky = crate::slots::sticky_of(state, username).await;
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
        let title = one_line(&title, 24);
        let mark = if sticky == Some(*no) { " ← 当前" } else { "" };
        lines.push(format!("  {}. [{}] {}{}", no, status_zh(t.status), title, mark));
    }
    // 号位绑终端窗口、不随列表刷新重排，所以中间可能有空号（终端关掉了）——那是正常的
    lines.push("\n号位跟着终端窗口固定不变，可能不连号。".to_string());
    match sticky {
        Some(n) => lines.push(format!(
            "当前对话：{n} 号 —— 直接发内容即可，不用带 @；发「@其它号」可切换。"
        )),
        None => lines.push("发「@2 内容」下发任务，之后直接发内容就一直发给 2 号。".to_string()),
    }
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

pub(crate) async fn resolve_task(state: &SharedState, username: &str, arg: &str) -> Result<String, String> {
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
    match queue_command(state, username, &task_id, action, None, "dingtalk").await {
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
pub(crate) async fn read_queue(
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
    // 微信那条路收消息时就把内容取好了（直链会过期），直接用；钉钉才需要现在去下载。
    let bytes = match &pf.bytes {
        Some(b) => b.clone(),
        None => {
            // 下载要用「收到该文件的那个应用」的凭据（多租户下 app_user 可能 != 归属账号）
            let app = state
                .registry
                .read()
                .await
                .dingtalk_app_of(&pf.app_user)
                .ok_or("未配置钉钉应用")?;
            let now_ms = crate::state::now_secs() * 1000;
            crate::dingtalk::download_bot_file(&app, &pf.download_code, now_ms).await?
        }
    };
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
        // 钉钉转发的附件一律整份下发：走的是钉钉自己的下载接口，文件已完整落在 hub 内存里，
        // 再切片没有意义（切片是为了让**上行**的大文件不必一次性穿过 hub）。
        chunk_index: 0,
        chunk_total: 0,
    });
    // 回填路径：目标在项目目录内 → 用相对 `./子路径`，否则用绝对路径（Claude 才找得到）。
    let rel = target
        .strip_prefix(&format!("{cwd}{sep}"))
        .map(|r| format!("./{}", r.replace('\\', "/")))
        .unwrap_or(target);
    Ok(rel)
}

/// 微信附件入挂起队列。内容已在收消息时下载并解密好（微信直链会过期，见
/// `BotPendingFile::bytes`），这里只负责起名和排队。返回落盘用的文件名。
///
/// `origin` 是消息里带的原文件名：**文件有，图片没有** —— 图片只好按魔数猜扩展名
/// 另起一个。传进来的名字只取 basename，防路径穿越。
pub(crate) async fn stash_weixin_file(
    state: &SharedState,
    username: &str,
    bytes: Vec<u8>,
    origin: &str,
) -> String {
    let base = match std::path::Path::new(origin.trim())
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(n) => n,
        // 图片没有原名，按魔数猜扩展名 + 时间戳凑一个
        None => format!("微信图片-{}.{}", crate::state::now_secs(), crate::weixin::image_ext(&bytes)),
    };
    let mut map = state.bot_pending_files.write().await;
    let list = map.entry(username.to_string()).or_default();
    let name = crate::dingtalk_stream::unique_name(list, &base);
    list.push(crate::state::BotPendingFile {
        download_code: String::new(),
        file_name: name.clone(),
        app_user: username.to_string(),
        at: crate::state::now_secs(),
        bytes: Some(bytes),
    });
    tracing::info!("微信暂存待发附件 account={username} name={name}");
    name
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

/// 「历史 [N]」：回看最近的远程往来，按时间正序排成对话流。
///
/// 参数是**号位**（与 `@N` 同源）时只看那个会话；不带参数看全部。
/// 「@9 历史」经速记展开成「历史 9」，落到这里也是只看 9 号 —— 与「在某个会话里点历史」
/// 的直觉一致。
async fn list_history(state: &SharedState, username: &str, arg: &str) -> String {
    let arg = arg.trim();
    // 号位 → 该会话；解析不出号位就当「看全部」，条数仍支持（历史 20）
    let (session, n) = if arg.is_empty() {
        (None, 10)
    } else if let Ok(id) = resolve_task(state, username, arg).await {
        (Some(id), 30)
    } else {
        (None, arg.parse::<usize>().unwrap_or(10).clamp(1, 30))
    };
    let list = crate::history::list_for(state, username, session.as_deref(), n).await;
    if list.is_empty() {
        return if session.is_some() {
            "这个会话还没有远程往来记录。".to_string()
        } else {
            "还没有远程往来记录。从这里或网页下发任务后，一问一答都会记进来。".to_string()
        };
    }
    let mut lines = vec![format!("最近 {} 条往来（旧 → 新）：", list.len())];
    let mut last_session = String::new();
    for e in &list {
        // 换会话时插一行分隔，否则多个终端的往来混在一起读不出是谁说的
        if e.session_id != last_session {
            last_session = e.session_id.clone();
            let slot = e.slot.map(|n| format!("{n} 号 · ")).unwrap_or_default();
            let title: String =
                if e.title.is_empty() { e.provider.clone() } else { e.title.clone() };
            lines.push(format!(
                "\n—— {slot}{} · {} ——",
                e.project,
                one_line(&title, 24)
            ));
        }
        let when = chrono::DateTime::from_timestamp(e.at as i64, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let who = if e.role == "user" { "🧑 我" } else { "🤖" };
        // 每条只给前 3 行，钉钉里堆全文没法翻；完整内容去网页看
        let body: String = e
            .content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(format!("{who}（{when}）\n{body}"));
    }
    lines.push("\n完整内容可在网页输入框旁的「历史」里查看。".to_string());
    lines.join("\n")
}

/// 「@N」（不带内容）：把连续对话切到 N 号，之后不带 @ 的文本都投给它。
async fn lock_session(state: &SharedState, username: &str, arg: &str) -> String {
    // 先解析一次，确认这个号确实有会话 —— 免得锁到一个空号上，后面每条消息都报错
    let task_id = match resolve_task(state, username, arg).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let Ok(n) = arg.trim().parse::<u32>() else {
        return "请给会话号位，如「@2」。发「会话」看号位。".to_string();
    };
    crate::slots::set_sticky(state, username, n).await;
    let title = state
        .tasks_for(username)
        .await
        .into_iter()
        .find(|t| t.id == task_id)
        .map(|t| {
            let s = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
            format!("（{} · {}）", t.project_name, one_line(&s, 20))
        })
        .unwrap_or_default();
    format!("✅ 已锁定会话 {n}{title}\n之后直接发内容即可，不用带 @。发「@其它号」可切换。")
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
    // 下发成功即锁定该会话：后续不带 @ 的文本都投给它（连续对话）
    if let Ok(n) = idx.trim().parse::<u32>() {
        crate::slots::set_sticky(state, username, n).await;
    }
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
        queue_command(state, username, &task_id, ControlAction::Input, Some(text.clone()), "dingtalk").await
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
    // 判定阶段：轮询最多 ~6s，等客户端取走并上报回队列状态（活跃机约 1.5s 一轮，留 ~4 轮
    // 足够可靠地判出「排队」；判出排队会提前 break，只有「已执行」才等满窗口）。检到排队即推。
    let mut queued_list: Option<Vec<String>> = None;
    for _ in 0..4 {
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

/// OTO 主动私聊给账号本人（网页/客户端下发的状态推送用；无 sessionWebhook 可回）。
/// 机器人一对一：用他自己的应用，发给跟这个机器人说过话的那个钉钉号。
async fn push_oto_owner(state: &SharedState, owner: &str, text: &str) {
    let now_ms = crate::state::now_secs() * 1000;
    let target = state.registry.read().await.dingtalk_push_target(owner);
    let Some((app, staff_id)) = target else { return };
    if app.app_secret.is_empty() {
        return;
    }
    let _ = crate::dingtalk::push_oto(&app, &staff_id, text, None, now_ms).await;
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
                let title = one_line(&title, 20);
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

/// 给会话排一条命令。`source` 标记下发来源（dingtalk / web / mcp），只用于历史记录的展示，
/// 让你回看时知道「这条是我在手机上发的还是在网页发的」。
pub(crate) async fn queue_command(
    state: &SharedState,
    username: &str,
    task_id: &str,
    action: ControlAction,
    text: Option<String>,
    source: &str,
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
        text: text.clone(),
        id: Some(uuid::Uuid::new_v4().to_string()),
        // 钉钉侧没有选择卡的作答入口（「⌨️ 需要你选择」只是通知），一律按普通下发处理
        from_select: false,
    });
    drop(machines); // 记历史要拿别的锁，先放掉

    // 下发的任务进「远程交互历史」的 user 侧。钉钉 / 网页 / MCP 三个入口都汇到这里，
    // 所以只需在此记一次；控制类指令（暂停/中断…）不入流，它们不是对话内容。
    if matches!(action, ControlAction::Input) {
        if let Some(content) = text {
            let slot = crate::slots::slot_of(state, username, &crate::slots::anchor_of(&task)).await;
            crate::history::append(
                state,
                crate::history::HistoryEntry {
                    id: crate::history::new_id(),
                    owner: username.to_string(),
                    session_id: task_id.to_string(),
                    role: "user".into(),
                    content,
                    at: crate::state::now_secs(),
                    source: source.to_string(),
                    slot,
                    hostname: task.hostname.clone(),
                    project: task.project_name.clone(),
                    title: task.title.clone(),
                    provider: task.provider_dsr.clone(),
                },
            )
            .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_line_flattens_multiline_titles() {
        use super::one_line;
        // 就是「二级弹」那条：多行提示词嵌进列表行，换行必须被压掉
        assert_eq!(
            one_line("报文大小和报文分析的请求体响应体切换删除\n二级弹要看清楚", 24),
            "报文大小和报文分析的请求体响应体切换删除 二级弹" // 20 字 + 空格 + 3 字 = 24
        );
        // 先压平再截断：额度不该被换行/多余空白吃掉
        assert_eq!(one_line("甲\n\n  乙   丙", 5), "甲 乙 丙");
        assert_eq!(one_line("abcdefgh", 3), "abc");
    }

    use super::{is_immediate, parse_at_commands, split_cmd};

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
        // 指令词开头、但后面还有正文 → 是任务内容，不是指令（实测踩过：「@3 继续…」
        // 被当成「恢复 3」，任务整条丢失）
        assert_eq!(
            c("@3 继续修复登录 bug"),
            Some(vec![("发".into(), "3 继续修复登录 bug".into())])
        );
        assert_eq!(c("@3 暂停一下再说"), Some(vec![("发".into(), "3 暂停一下再说".into())]));
        assert_eq!(c("@3 停止服务后重启"), Some(vec![("发".into(), "3 停止服务后重启".into())]));
        // 「继续」已彻底不作会话指令：单独发也当内容 —— 它几乎总是「让 claude 接着干活」，
        // 真要解除暂停有「恢复」。其余指令词单独出现时仍是指令（见上面的 @2 暂停/排队/撤回）。
        assert_eq!(c("@3 继续"), Some(vec![("发".into(), "3 继续".into())]));
        // 去重目标
        assert_eq!(c("@1 @1 x"), Some(vec![("发".into(), "1 x".into())]));
        // 非 @ → None（走常规分发）；无效目标 → Some(空)（提示用法）
        assert_eq!(c("发 2 继续"), None);
        assert_eq!(c("@abc"), Some(vec![]));
        // 单发「@N」= 切到 N 号继续对话；多目标时没有「当前会话」可言，退回用法提示
        assert_eq!(c("@2"), Some(vec![("锁定".into(), "2".into())]));
        assert_eq!(c("@1 @2"), Some(vec![]));
    }

    /// 钉钉合并窗口的豁免判断：指令必须单独成条立即执行，内容才攒着合并。
    /// 判错的代价不对称 —— 指令被误判成内容，会原样发进终端当正文。
    #[test]
    fn immediate_vs_batched() {
        // —— 立即执行：指令 ——
        assert!(is_immediate("会话"));
        assert!(is_immediate("暂停 3"));
        assert!(is_immediate("发 2 继续执行"));
        assert!(is_immediate("清空文件"));
        assert!(is_immediate("@2 暂停")); // 会话级指令
        assert!(is_immediate("@2")); // = 锁定 2 号
        assert!(is_immediate("@abc")); // @ 用法错误 → 立即回提示
        assert!(is_immediate("确认")); // sticky 冷却确认，攒了就等不到了
        assert!(is_immediate("OK"));

        // —— 进合并窗口：内容 ——
        assert!(!is_immediate("@2 帮我看这段日志")); // @N + 正文 = 发内容
        assert!(!is_immediate("重启一下服务"));
        assert!(!is_immediate("[转发] 昨天那个报错又出现了"));
        // 「继续」不是指令（见 at_commands），自然也该参与合并
        assert!(!is_immediate("@3 继续"));
        // 指令词开头但后面还有正文 → 是内容
        assert!(!is_immediate("@3 暂停一下再说"));
    }
}
