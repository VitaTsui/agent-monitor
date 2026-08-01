//! MCP 端点：把「遥控自己的终端会话」这套能力暴露给 AI 客户端，用于**多会话编排** ——
//! 一个 agent 当总控，给其它终端派活、等它们做完、汇总结果。
//!
//! 传输用 MCP 的 Streamable HTTP：单个 `POST /mcp` 收 JSON-RPC 2.0。选它而不是本地 stdio
//! server，是因为 hub 本来就在线上跑着、有 HTTPS 和账号体系 —— 任何设备上的 AI 客户端填个
//! URL + token 就能用，不必分发、升级任何本地程序：
//!
//! ```text
//! claude mcp add --transport http agent-monitor https://<hub>/mcp \
//!   --header "Authorization: Bearer <登录 token>"
//! ```
//!
//! 会话寻址与钉钉机器人**完全同源**（复用 bot::resolve_task / sorted_active_tasks）：既能用
//! 号位（`"9"`，跟着终端窗口固定），也能用会话 id。两个入口行为一致，不会各说各话。

use crate::state::SharedState;
use am_core::model::{ControlAction, TaskStatus};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

/// 回显客户端请求的协议版本（兼容性最好）；客户端没给就用这个已知版本。
const FALLBACK_PROTOCOL: &str = "2025-06-18";

/// `wait_until_idle` 单次调用最多等多久 —— 超过这个时长要还给客户端，免得撞上它的请求超时。
/// 没等到就返回当前状态，由调用方（AI）决定要不要再等一轮。
const WAIT_MAX_SECS: u64 = 120;
const WAIT_DEFAULT_SECS: u64 = 60;

/// POST /mcp —— MCP over Streamable HTTP。
pub async fn mcp_post(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    // 通知（无 id）不需要响应体，回 202 即可
    if id.is_none() {
        return (StatusCode::ACCEPTED, Json(json!({})));
    }
    let params = req.get("params").cloned().unwrap_or(json!({}));

    // MCP 客户端的惯例写法是 `Authorization: Bearer <token>`，而 hub 的 auth_user 把**整个**
    // 头值当 token。这里两种写法都收：带 Bearer 前缀就剥掉再验 —— 否则用户照惯例配置反而连不上，
    // 而且报的是「未授权」，极难联想到是前缀问题。
    let headers = {
        let mut h = headers.clone();
        if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            let bare = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "));
            if let Some(raw) = bare {
                if let Ok(hv) = raw.trim().parse() {
                    h.insert("authorization", hv);
                }
            }
        }
        h
    };
    // initialize / tools/list 不碰用户数据，免鉴权也无妨；但既然要暴露工具清单，
    // 统一要求鉴权更简单也更安全 —— 未授权时给出明确的 JSON-RPC 错误，方便排查配置。
    let Some(user) = crate::admin::auth_user(&state, &headers).await else {
        return (
            StatusCode::OK,
            Json(rpc_err(id, -32001, "未授权：请在 Authorization 头带上登录 token")),
        );
    };

    let result = match method {
        "initialize" => {
            let ver = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(FALLBACK_PROTOCOL);
            Ok(json!({
                "protocolVersion": ver,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agent-monitor", "version": env!("CARGO_PKG_VERSION") },
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_defs() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(&state, &user, name, &args).await {
                // MCP 约定：工具自身的失败走 isError，而不是 JSON-RPC 错误 ——
                // 那样模型才能看到失败原因并自己调整，而不是整个请求报错。
                Ok(text) => Ok(json!({ "content": [{ "type": "text", "text": text }] })),
                Err(text) => Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": true,
                })),
            }
        }
        _ => Err((-32601, format!("未知方法「{method}」"))),
    };

    match result {
        Ok(r) => (StatusCode::OK, Json(json!({ "jsonrpc": "2.0", "id": id, "result": r }))),
        Err((code, msg)) => (StatusCode::OK, Json(rpc_err(id, code, &msg))),
    }
}

/// GET /mcp —— 本端点不提供服务端推送（SSE）流，只做请求-响应。
pub async fn mcp_get() -> (StatusCode, &'static str) {
    (StatusCode::METHOD_NOT_ALLOWED, "本 MCP 端点只支持 POST（无 SSE 流）")
}

fn rpc_err(id: Option<Value>, code: i64, msg: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

/// 「会话」参数的通用 schema：号位或会话 id 都收
fn session_arg() -> Value {
    json!({
        "type": "string",
        "description": "会话号位（如 \"9\"，跟着终端窗口固定不变，见 list_sessions）或会话 id",
    })
}

fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_sessions",
            "description": "列出当前可操作的所有 agent 会话：号位、设备、项目、状态、当前任务标题、排队条数。\
                编排的起点 —— 先看有哪些终端可以派活。号位跟着终端窗口固定，可能不连号（关掉的终端会留下空号）。",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        }),
        json!({
            "name": "session_detail",
            "description": "看某个会话在做什么：状态、当前任务、排队中的输入、最近若干条对话摘要。\
                派活前确认它闲着、派活后查看它做了什么，都用这个。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "messages": { "type": "integer", "description": "带回最近多少条对话（默认 10，最多 50）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "send_to_session",
            "description": "给某个会话下发一条任务（等同于在那个终端里敲一段话并回车）。\
                会话正忙时，输入会排进它的队列、等当前任务结束再执行 —— 返回值会说明是立即执行还是已排队。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "text": { "type": "string", "description": "要下发的任务内容" },
                },
                "required": ["session", "text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "wait_until_idle",
            "description": "阻塞等待某个会话干完活（状态变为等待输入且队列排空），用于「派活 → 等完成 → 收结果」。\
                最多等 timeout_secs（默认 60，上限 120）；没等到不算失败，会返回当前状态，可以再调一次继续等。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "timeout_secs": { "type": "integer", "description": "最多等多少秒（默认 60，上限 120）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "control_session",
            "description": "控制会话：pause 暂停 / resume 恢复 / interrupt 打断当前任务（相当于按一次 Esc）/ stop 终止进程。\
                interrupt 用于「它跑偏了，让它停下重来」；stop 会结束整个 agent 进程，慎用。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "action": { "type": "string", "enum": ["pause", "resume", "interrupt", "stop"] },
                },
                "required": ["session", "action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "recall_last",
            "description": "撤回该会话最近一条**还在排队、尚未开始执行**的输入。派错活了可以及时收回。",
            "inputSchema": {
                "type": "object",
                "properties": { "session": session_arg() },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
    ]
}

async fn call_tool(
    state: &SharedState,
    user: &str,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let sess = || -> Result<String, String> {
        args.get("session")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .ok_or_else(|| "缺少参数 session".to_string())
    };
    match name {
        "list_sessions" => list_sessions(state, user).await,
        "session_detail" => {
            let n = args.get("messages").and_then(Value::as_u64).unwrap_or(10).min(50) as usize;
            session_detail(state, user, &sess()?, n).await
        }
        "send_to_session" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or("缺少参数 text（要下发的内容）")?;
            send_to_session(state, user, &sess()?, text).await
        }
        "wait_until_idle" => {
            let secs = args
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(WAIT_DEFAULT_SECS)
                .min(WAIT_MAX_SECS);
            wait_until_idle(state, user, &sess()?, secs).await
        }
        "control_session" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            control_session(state, user, &sess()?, action).await
        }
        "recall_last" => recall_last(state, user, &sess()?).await,
        _ => Err(format!("未知工具「{name}」")),
    }
}

/// 会话寻址：号位或会话 id 都收。数字一律按号位解析（与钉钉的「@9」同源）。
async fn resolve(state: &SharedState, user: &str, sess: &str) -> Result<String, String> {
    let s = sess.trim();
    if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
        return crate::bot::resolve_task(state, user, s).await;
    }
    // 当会话 id 用：确认它确实在该用户名下，避免越权操作别人的会话
    if state.tasks_for(user).await.iter().any(|t| t.id == s) {
        Ok(s.to_string())
    } else {
        Err(format!("找不到会话「{s}」。用 list_sessions 看当前可操作的会话。"))
    }
}

fn status_zh(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Running => "执行中",
        TaskStatus::Paused => "已暂停",
        TaskStatus::Idle => "等待输入",
        TaskStatus::Finished => "已结束",
    }
}

async fn list_sessions(state: &SharedState, user: &str) -> Result<String, String> {
    let tasks = crate::bot::sorted_active_tasks(state, user).await;
    if tasks.is_empty() {
        return Ok("当前没有活跃会话。".into());
    }
    let mut lines = vec![format!("共 {} 个活跃会话：", tasks.len())];
    for (t, no) in &tasks {
        let queued = crate::bot::read_queue(state, user, &t.id)
            .await
            .map(|(hub, term)| hub.len() + term.len())
            .unwrap_or(0);
        let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
        lines.push(format!(
            "[{no}] {} · {} · {} —— {}{}",
            t.hostname,
            t.project_name,
            status_zh(t.status),
            title.chars().take(60).collect::<String>(),
            if queued > 0 { format!("（排队 {queued} 条）") } else { String::new() },
        ));
    }
    Ok(lines.join("\n"))
}

async fn session_detail(
    state: &SharedState,
    user: &str,
    sess: &str,
    want_msgs: usize,
) -> Result<String, String> {
    let id = resolve(state, user, sess).await?;
    let task = state
        .tasks_for(user)
        .await
        .into_iter()
        .find(|t| t.id == id)
        .ok_or("会话不存在（可能刚结束）")?;
    let mut out = vec![format!(
        "设备：{}\n项目：{}\n状态：{}\n当前任务：{}",
        task.hostname,
        task.project_name,
        status_zh(task.status),
        if task.prompt.is_empty() { "（无）" } else { task.prompt.as_str() },
    )];
    if let Some((hub_pending, term_q)) = crate::bot::read_queue(state, user, &id).await {
        let all: Vec<String> = term_q.into_iter().chain(hub_pending).collect();
        if !all.is_empty() {
            out.push(format!("排队中（{} 条）：", all.len()));
            for (i, q) in all.iter().enumerate() {
                out.push(format!("  {}. {}", i + 1, q.chars().take(80).collect::<String>()));
            }
        }
    }
    let msgs = state.bot_task_messages(&id).await;
    if !msgs.is_empty() && want_msgs > 0 {
        out.push(format!("最近对话（{} 条）：", msgs.len().min(want_msgs)));
        for m in msgs.iter().rev().take(want_msgs).rev() {
            let who = match m.role.as_str() {
                "user" => "用户",
                "assistant" => "助手",
                other => other,
            };
            out.push(format!("  [{who}] {}", m.content.chars().take(200).collect::<String>()));
        }
    }
    Ok(out.join("\n"))
}

async fn send_to_session(
    state: &SharedState,
    user: &str,
    sess: &str,
    text: &str,
) -> Result<String, String> {
    let id = resolve(state, user, sess).await?;
    crate::bot::queue_command(state, user, &id, ControlAction::Input, Some(text.to_string()), "mcp")
        .await?;
    // 状态只是「下发那一刻」的快照：忙就是会排队。真要确认有没有被吃进去，用 wait_until_idle。
    let busy = state
        .tasks_for(user)
        .await
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.status == TaskStatus::Running)
        .unwrap_or(false);
    Ok(if busy {
        format!("已下发到会话「{sess}」。该会话正忙，这条会排队等当前任务结束后执行。")
    } else {
        format!("已下发到会话「{sess}」。可用 wait_until_idle 等它做完。")
    })
}

async fn wait_until_idle(
    state: &SharedState,
    user: &str,
    sess: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let id = resolve(state, user, sess).await?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let task = state.tasks_for(user).await.into_iter().find(|t| t.id == id);
        let Some(task) = task else {
            return Ok(format!("会话「{sess}」已消失（终端可能已关闭）。"));
        };
        let queued = crate::bot::read_queue(state, user, &id)
            .await
            .map(|(hub, term)| hub.len() + term.len())
            .unwrap_or(0);
        // 「干完活」= 不在跑 且 队列排空。只看状态不够：刚下发时它可能还没开始跑。
        if task.status != TaskStatus::Running && queued == 0 {
            return Ok(format!(
                "会话「{sess}」已就绪（{}）。当前任务：{}",
                status_zh(task.status),
                if task.prompt.is_empty() { "（无）" } else { task.prompt.as_str() },
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(format!(
                "等待超时（{timeout_secs}s）：会话「{sess}」仍在 {}{}。可以再调一次继续等。",
                status_zh(task.status),
                if queued > 0 { format!("、排队 {queued} 条") } else { String::new() },
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn control_session(
    state: &SharedState,
    user: &str,
    sess: &str,
    action: &str,
) -> Result<String, String> {
    let id = resolve(state, user, sess).await?;
    let (act, word) = match action {
        "pause" => (ControlAction::Pause, "已暂停"),
        "resume" => (ControlAction::Resume, "已恢复"),
        "interrupt" => (ControlAction::Interrupt, "已打断当前任务"),
        "stop" => (ControlAction::Stop, "已终止"),
        other => return Err(format!("未知动作「{other}」，可用：pause / resume / interrupt / stop")),
    };
    crate::bot::queue_command(state, user, &id, act, None, "mcp").await?;
    Ok(format!("会话「{sess}」{word}。"))
}

async fn recall_last(state: &SharedState, user: &str, sess: &str) -> Result<String, String> {
    let id = resolve(state, user, sess).await?;
    let (hub_pending, term_q) =
        crate::bot::read_queue(state, user, &id).await.ok_or("会话不存在")?;
    if hub_pending.is_empty() && term_q.is_empty() {
        return Err(format!("会话「{sess}」当前没有排队中的输入可撤回。"));
    }
    // 与钉钉「撤回」走同一条路径：先撤 hub 队列里还没下发的，撤不到才注入 ↑。
    // 不能无条件按 ↑ —— hub 侧还压着一条时那样会撤掉终端里**另一条**输入。
    match crate::bot::recall_input(state, user, &id).await? {
        crate::bot::Recalled::FromHubQueue => {
            Ok(format!("已撤回会话「{sess}」最近一条排队中的输入（尚未下发，直接丢弃）。"))
        }
        crate::bot::Recalled::InjectedUpKey => Ok(format!(
            "会话「{sess}」那条输入已进终端队列，已注入 ↑ 撤回（macOS Terminal.app 需手动按 ↑）。"
        )),
    }
}
