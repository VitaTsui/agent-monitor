use crate::admin::{self, auth_user, err, ok};
use crate::model::{ControlCmd, ControlReq, ReportPayload, Task, TaskStatus};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use crate::state::{MachineEntry, SharedState, OFFLINE_AFTER_SECS};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: SharedState) -> Router {
    let mut router = Router::new()
        // ---- vita-admin 契约 ----
        .route("/auth/access/getCryptoKey", get(admin::get_crypto_key))
        .route("/auth/access/isNeedLoginCaptcha", get(admin::is_need_captcha))
        .route("/auth/access/dingtalk/url", get(admin::dingtalk_url))
        .route("/auth/access/login", post(admin::login))
        .route("/auth/access/register", post(admin::register))
        .route("/auth/access/logout", get(admin::logout))
        .route("/sys/menu/getMenuATopATopMenu", get(admin::menus))
        .route("/sys/menu/getStringPermissions", get(admin::permissions))
        // ---- 后管（admin token 锁 + 仅用户管理）----
        .route("/sys/admin/verify", post(admin::verify_admin_token))
        .route("/sys/user/page", get(admin::user_page))
        .route("/sys/user/add", post(admin::user_add))
        .route("/sys/user/upd", post(admin::user_upd))
        .route("/sys/user/resetPwd", post(admin::user_reset_pwd))
        .route("/sys/user/del", get(admin::user_del))
        // ---- 任务监控 API（前台公开使用）----
        .route("/monitor/tasks", get(list_tasks))
        .route("/monitor/tasks/page", get(page_tasks))
        .route("/monitor/tasks/detail/:id", get(task_detail))
        .route("/monitor/tasks/:id/messages", get(task_messages))
        .route("/monitor/tasks/:id/slash-commands", get(task_slash_commands))
        .route("/monitor/tasks/:id/control", post(control_task))
        .route("/monitor/tasks/:id/input", post(input_task))
        .route("/monitor/machines", get(machines))
        .route("/monitor/agent", get(agent_status))
        .route("/monitor/quota", get(get_quota).post(set_quota))
        .route("/monitor/ws", get(ws_handler))
        // ---- 设备管理（信任设备）----
        .route("/monitor/devices", get(list_devices))
        .route("/monitor/devices/:id/trust", post(trust_device))
        .route("/monitor/devices/:id/untrust", post(untrust_device))
        .route("/monitor/devices/:id", axum::routing::delete(delete_device))
        .route("/monitor/devices/:id/upload", post(upload_file))
        // ---- agent → hub 上报 ----
        .route("/monitor/report", post(report))
        .with_state(state);

    // CORS：默认同源（开发经 webpack 代理、生产由 hub 自托管前端，均无需跨域）。
    // 特殊部署（前端独立域名）可用 AM_CORS_ALLOW_ANY=1 放开。
    if std::env::var("AM_CORS_ALLOW_ANY").ok().as_deref() == Some("1") {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    // 静态托管前端构建产物（存在时），SPA 路由回退 index.html
    if let Some(dist) = web_dist_dir() {
        tracing::info!("托管前端静态资源: {}", dist.display());
        let index = dist.join("index.html");
        router = router.fallback_service(ServeDir::new(&dist).not_found_service(ServeFile::new(index)));
    }
    router
}

/// 前端构建产物目录：AM_WEB_DIST > 可执行文件旁的 web/ > ../agent-monitor-web/dist
fn web_dist_dir() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("AM_WEB_DIST") {
        candidates.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("web"));
            candidates.push(dir.join("../Resources/web")); // macOS .app 包内
        }
    }
    candidates.push("../agent-monitor-web/dist".into());
    candidates
        .into_iter()
        .find(|p| p.join("index.html").is_file())
}

// ---------- 简单过滤（前台用平铺参数） ----------

#[derive(Deserialize, Default)]
struct PlainQuery {
    /// 逗号分隔状态过滤
    status: Option<String>,
    /// 模糊过滤（项目/提示词/主机名）
    keyword: Option<String>,
}

impl PlainQuery {
    fn matches(&self, t: &Task) -> bool {
        if let Some(status) = &self.status {
            if !status.is_empty() {
                let want: Vec<&str> = status.split(',').map(str::trim).collect();
                if !want.contains(&status_key(t.status)) {
                    return false;
                }
            }
        }
        if let Some(k) = &self.keyword {
            let k = k.trim().to_lowercase();
            if !k.is_empty()
                && !t.project.to_lowercase().contains(&k)
                && !t.prompt.to_lowercase().contains(&k)
                && !t.hostname.to_lowercase().contains(&k)
            {
                return false;
            }
        }
        true
    }
}

fn status_key(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Running => "running",
        TaskStatus::Idle => "idle",
        TaskStatus::Paused => "paused",
        TaskStatus::Finished => "finished",
    }
}

/// GET /monitor/tasks —— 前台会话列表（登录用户可见 + 只显示活跃会话）
async fn list_tasks(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<PlainQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let tasks = state.tasks_for(&user).await;
    // 前台只展示活跃会话（有存活进程）：退出后已结束的会话不再累积
    let filtered: Vec<_> = tasks
        .into_iter()
        .filter(|t| t.status != TaskStatus::Finished && q.matches(t))
        .collect();
    ok(json!({ "list": filtered }))
}

// ---------- vita-admin Query 格式（后管 Panel.List 用） ----------

#[derive(Deserialize, Default)]
struct AdminPageQuery {
    /// URL 编码的 JSON 查询对象（Query.ts 的 toEncode 产物）
    query: Option<String>,
}

/// GET /monitor/tasks/page —— 解析 vita-admin 的 query JSON，返回 ListRes
async fn page_tasks(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<AdminPageQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let tasks = state.tasks_for(&user).await;

    let parsed: Value = q
        .query
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);

    // 过滤条件 r[].w[]: { k, v, m }
    let mut filtered: Vec<Task> = tasks
        .into_iter()
        .filter(|t| match_query_filters(t, &parsed))
        .collect();

    // 排序 o[0]: { k, t }
    let (order_key, desc) = parsed
        .get("o")
        .and_then(Value::as_array)
        .and_then(|o| o.first())
        .map(|o| {
            (
                o.get("k").and_then(Value::as_str).unwrap_or("crtTm").to_string(),
                o.get("t").and_then(Value::as_str).unwrap_or("desc") == "desc",
            )
        })
        .unwrap_or(("crtTm".into(), true));
    sort_tasks(&mut filtered, &order_key, desc);

    // 分页 p: { n, s }
    let page_num = parsed
        .pointer("/p/n")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let page_size = parsed
        .pointer("/p/s")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 200) as usize;

    let total = filtered.len();
    let items: Vec<_> = filtered
        .into_iter()
        .skip((page_num - 1) * page_size)
        .take(page_size)
        .collect();
    ok(json!({
        "list": items,
        "page": { "pageNum": page_num, "pageSize": page_size, "total": total }
    }))
}

fn match_query_filters(t: &Task, parsed: &Value) -> bool {
    let Some(rules) = parsed.get("r").and_then(Value::as_array) else {
        return true;
    };
    for rule in rules {
        let Some(conds) = rule.get("w").and_then(Value::as_array) else {
            continue;
        };
        for cond in conds {
            let k = cond.get("k").and_then(Value::as_str).unwrap_or("");
            let v = cond.get("v").cloned().unwrap_or(Value::Null);
            let m = cond.get("m").and_then(Value::as_str).unwrap_or("LK");
            if !match_one(t, k, &v, m) {
                return false;
            }
        }
    }
    true
}

fn field_of(t: &Task, k: &str) -> Option<String> {
    let s = match k {
        "id" => t.id.clone(),
        "project" => t.project.clone(),
        "projectName" => t.project_name.clone(),
        "prompt" => t.prompt.clone(),
        "status" => status_key(t.status).to_string(),
        "provider" => t.provider.clone(),
        "hostname" => t.hostname.clone(),
        "platform" => t.platform.clone(),
        "machineId" => t.machine_id.clone(),
        "ideDsr" => t.ide_dsr.clone(),
        "gitBranch" => t.git_branch.clone().unwrap_or_default(),
        _ => return None,
    };
    Some(s)
}

fn match_one(t: &Task, k: &str, v: &Value, m: &str) -> bool {
    // 未知字段不参与过滤（宽松处理，避免前端加字段导致空列表）
    let Some(field) = field_of(t, k) else { return true };
    let vs = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(_) => String::new(),
        _ => return true,
    };
    match m {
        "EQ" => field == vs,
        "NE" => field != vs,
        "IN" => v
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.as_u64().map(|n| n.to_string())))
                    .any(|x| x == field)
            })
            .unwrap_or(true),
        "LK" => field.to_lowercase().contains(&vs.to_lowercase()),
        "LLK" => field.to_lowercase().ends_with(&vs.to_lowercase()),
        "RLK" => field.to_lowercase().starts_with(&vs.to_lowercase()),
        "NLK" => !field.to_lowercase().contains(&vs.to_lowercase()),
        _ => true,
    }
}

fn sort_tasks(tasks: &mut [Task], key: &str, desc: bool) {
    match key {
        // crtTm（基类默认排序键）映射到最近活动时间
        "crtTm" | "mtimeMs" | "lastActiveAt" | "updTm" => {
            tasks.sort_by(|a, b| a.mtime_ms.cmp(&b.mtime_ms));
        }
        "startedAt" => tasks.sort_by(|a, b| a.started_at.cmp(&b.started_at)),
        "hostname" => tasks.sort_by(|a, b| a.hostname.cmp(&b.hostname)),
        "projectName" => tasks.sort_by(|a, b| a.project_name.cmp(&b.project_name)),
        "status" => tasks.sort_by_key(|t| status_key(t.status)),
        _ => tasks.sort_by(|a, b| a.mtime_ms.cmp(&b.mtime_ms)),
    }
    if desc {
        tasks.reverse();
    }
}

/// GET /monitor/tasks/detail/:id
async fn task_detail(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let tasks = state.tasks_for(&user).await;
    match tasks.into_iter().find(|t| t.id == id) {
        Some(t) => ok(serde_json::to_value(t).unwrap_or(Value::Null)),
        None => err(404, "任务不存在"),
    }
}

#[derive(Deserialize)]
struct MsgQuery {
    limit: Option<usize>,
}

/// GET /monitor/tasks/:id/messages —— 本机实时解析；远程机器读上报缓存
async fn task_messages(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<MsgQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let limit = q.limit.unwrap_or(120).clamp(1, 500);
    let machine_id = {
        let tasks = state.tasks_for(&user).await;
        tasks.iter().find(|t| t.id == id).map(|t| t.machine_id.clone())
    };
    let Some(machine_id) = machine_id else {
        return err(404, "任务不存在");
    };

    if machine_id == state.config.machine_id {
        let scanner = state.scanner.lock().await;
        match scanner.messages(&id, limit) {
            Ok(list) => ok(json!({ "list": list })),
            Err(_) => ok(json!({ "list": [] })),
        }
    } else {
        let machines = state.machines.read().await;
        let list = machines
            .get(&machine_id)
            .and_then(|m| m.messages.get(&id))
            .cloned()
            .unwrap_or_default();
        ok(json!({ "list": list }))
    }
}

/// GET /monitor/tasks/:id/slash-commands —— 该会话模型的可用斜杠命令（只读扫描）
async fn task_slash_commands(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    // 仅本机会话可扫描自定义命令目录；远程会话给内置命令
    let project = if task.machine_id == state.config.machine_id {
        task.project.clone()
    } else {
        String::new()
    };
    let list = crate::commands::collect(&task.provider, &project);
    ok(json!({ "list": list }))
}

/// POST /monitor/tasks/:id/control —— 本机直接执行；远程机器进命令队列
async fn control_task(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ControlReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    // 安全：请求方传入的 pid 必须与任务快照的 pid 一致，
    // 防止借任意 pid 对目标机器上无关进程发信号/注入输入
    let pid = match (req.pid, task.pid) {
        (Some(p), Some(tp)) if p != tp => {
            let _ = (p, tp);
            return err(403, "pid 与任务不符");
        }
        (Some(p), None) => {
            let _ = p;
            return err(403, "该任务没有关联进程，不接受外部 pid");
        }
        _ => task.pid,
    };

    if task.machine_id == state.config.machine_id {
        let Some(pid) = pid else {
            return err(400, "该任务没有存活进程，无法控制");
        };
        match crate::process::control(pid, req.action) {
            Ok(label) => {
                {
                    let mut paused = state.paused.write().await;
                    match req.action {
                        crate::model::ControlAction::Pause => {
                            paused.insert(pid);
                        }
                        _ => {
                            paused.remove(&pid);
                        }
                    }
                }
                tracing::info!("控制任务 {id}: pid={pid} {label}");
                ok(json!({ "pid": pid, "result": label }))
            }
            Err(e) => err(500, &e.to_string()),
        }
    } else {
        let mut machines = state.machines.write().await;
        let Some(entry) = machines.get_mut(&task.machine_id) else {
            return err(404, "任务所属机器已离线");
        };
        if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
            return err(500, "任务所属机器已离线，无法下发命令");
        }
        entry.pending.push_back(ControlCmd {
            task_id: id.clone(),
            pid,
            action: req.action,
            text: None,
        });
        tracing::info!("已向机器 {} 下发控制命令: {id}", task.machine_id);
        ok(json!({ "pid": pid, "result": "命令已下发，等待执行" }))
    }
}

#[derive(Deserialize)]
struct InputReq {
    text: String,
    pid: Option<u32>,
}

/// POST /monitor/tasks/:id/input —— 向会话注入一行输入（前台发布任务）
/// 本机直接 TTY 注入；远程机器进命令队列由该机 agent 执行。
async fn input_task(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<InputReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return err(400, "输入内容为空");
    }
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    // 安全：请求方传入的 pid 必须与任务快照的 pid 一致，
    // 防止借任意 pid 对目标机器上无关进程发信号/注入输入
    let pid = match (req.pid, task.pid) {
        (Some(p), Some(tp)) if p != tp => {
            let _ = (p, tp);
            return err(403, "pid 与任务不符");
        }
        (Some(p), None) => {
            let _ = p;
            return err(403, "该任务没有关联进程，不接受外部 pid");
        }
        _ => task.pid,
    };

    if task.machine_id == state.config.machine_id {
        let Some(pid) = pid else {
            return err(400, "该任务没有存活进程，无法发布");
        };
        match crate::process::send_input(pid, &text) {
            Ok(label) => {
                tracing::info!("向任务 {id} (pid={pid}) 注入输入: {}", truncate_log(&text));
                ok(json!({ "pid": pid, "result": label }))
            }
            Err(e) => err(500, &e.to_string()),
        }
    } else {
        let mut machines = state.machines.write().await;
        let Some(entry) = machines.get_mut(&task.machine_id) else {
            return err(404, "任务所属机器已离线");
        };
        if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
            return err(500, "任务所属机器已离线，无法下发");
        }
        entry.pending.push_back(ControlCmd {
            task_id: id.clone(),
            pid,
            action: crate::model::ControlAction::Input,
            text: Some(text),
        });
        ok(json!({ "pid": pid, "result": "已下发到目标机器" }))
    }
}

fn truncate_log(s: &str) -> String {
    s.chars().take(60).collect()
}

/// GET /monitor/machines —— 当前用户名下的设备（含未信任 pending）
async fn machines(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let list = state.devices_for(&user).await;
    ok(json!({ "list": list }))
}

/// GET /monitor/devices —— 设备管理列表（同 machines，语义更清晰）
async fn list_devices(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    machines(State(state), headers).await
}

/// 校验当前用户对设备的管理权限（超级管理员或归属本人）
async fn ensure_owner(state: &SharedState, headers: &HeaderMap, id: &str) -> Result<String, Json<Value>> {
    let Some(user) = auth_user(state, headers).await else {
        return Err(err(401, "未登录"));
    };
    if !state.registry.read().await.owned_by(id, &user) {
        return Err(err(403, "无权管理该设备"));
    }
    Ok(user)
}

/// POST /monitor/devices/:id/trust —— 信任设备
async fn trust_device(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    if state.registry.write().await.set_trust(&id, true) {
        ok(json!({ "result": "已信任" }))
    } else {
        err(404, "设备不存在")
    }
}

/// POST /monitor/devices/:id/untrust —— 撤销信任（撤销后不再监控其会话）
async fn untrust_device(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    if state.registry.write().await.set_trust(&id, false) {
        ok(json!({ "result": "已撤销信任" }))
    } else {
        err(404, "设备不存在")
    }
}

/// DELETE /monitor/devices/:id —— 删除设备记录
async fn delete_device(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    if id == state.config.machine_id {
        return err(400, "不能删除 hub 本机");
    }
    state.machines.write().await.remove(&id);
    state.registry.write().await.delete_device(&id);
    ok(json!({ "result": "已删除" }))
}

/// POST /monitor/devices/:id/upload —— 传输文件到该设备的指定目录。
/// multipart 字段：dir（目标目录）、file（文件）。本机直接写入；远程进文件队列由 agent 拉取写入。
async fn upload_file(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    if !state.registry.read().await.owned_by(&id, &user) {
        return err(403, "无权向该设备传输文件");
    }

    let mut dir = String::new();
    let mut filename = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "dir" => dir = field.text().await.unwrap_or_default(),
            "file" => {
                filename = field.file_name().unwrap_or("file.bin").to_string();
                bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            }
            _ => {}
        }
    }
    let dir = dir.trim().to_string();
    if dir.is_empty() || filename.is_empty() || bytes.is_empty() {
        return err(400, "缺少目标目录或文件内容");
    }
    // 防止文件名穿越
    let safe_name = std::path::Path::new(&filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.bin".into());

    if id == state.config.machine_id {
        // 本机直接写入
        let target_dir = std::path::PathBuf::from(&dir);
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            return err(500, &format!("创建目录失败: {e}"));
        }
        let target = target_dir.join(&safe_name);
        match std::fs::write(&target, &bytes) {
            Ok(_) => {
                tracing::info!("已写入文件: {}", target.display());
                ok(json!({ "path": target.to_string_lossy(), "size": bytes.len() }))
            }
            Err(e) => err(500, &format!("写入失败: {e}")),
        }
    } else {
        // 远程设备：进文件队列由 agent 拉取写入
        let mut machines = state.machines.write().await;
        let Some(entry) = machines.get_mut(&id) else {
            return err(404, "设备不存在或已离线");
        };
        if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
            return err(500, "设备已离线，无法传输");
        }
        entry.pending_files.push_back(crate::model::FileTransfer {
            dir,
            filename: safe_name,
            content_b64: B64.encode(&bytes),
        });
        ok(json!({ "result": "已下发到目标设备，等待写入", "size": bytes.len() }))
    }
}

/// GET /monitor/quota —— 查询 5h token 上限与当前用量汇总
async fn get_quota(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let limit = state.registry.read().await.quota_limit();
    let tasks = state.tasks_for(&user).await;
    let used: u64 = tasks.iter().map(|t| t.used_tokens_5h).sum();
    ok(json!({ "limit": limit, "used": used }))
}

#[derive(Deserialize)]
struct SetQuotaReq {
    limit: u64,
}

/// POST /monitor/quota { limit } —— 设置 5h token 上限（0 = 不限制）
async fn set_quota(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<SetQuotaReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // 额度是全局配置，会触发所有用户会话的自动暂停/恢复 —— 仅超级管理员可改
    let mut reg = state.registry.write().await;
    if !reg.is_super_user(&user) {
        return err(403, "仅管理员可设置额度上限");
    }
    reg.set_quota_limit(req.limit);
    ok(json!({ "limit": req.limit }))
}

/// GET /monitor/agent —— 监控端状态（按当前用户可见范围统计）
async fn agent_status(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let tasks = state.tasks_for(&user).await;
    let running = tasks.iter().filter(|t| t.status == TaskStatus::Running).count();
    let process_count = tasks.iter().filter(|t| t.process.is_some()).count();
    let machines = state.devices_for(&user).await;
    let scanner = state.scanner.lock().await;
    ok(json!({
        "hostname": state.config.hostname,
        "platform": state.config.platform,
        "platformDsr": crate::model::platform_dsr(&state.config.platform),
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": state.started_at.to_rfc3339(),
        "projectsDir": scanner.projects_dir().to_string_lossy(),
        "sessionCount": tasks.len(),
        "runningCount": running,
        "processCount": process_count,
        "machineCount": machines.len(),
        "onlineMachineCount": machines.iter().filter(|m| m.online).count(),
    }))
}

/// POST /monitor/report —— agent 上报快照，响应携带待执行命令。
/// 未知设备登记为该 owner 的 pending（未信任）；非信任设备的会话不对外暴露。
async fn report(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(payload): Json<ReportPayload>,
) -> Json<Value> {
    // 上报鉴权：agent 必须持有与 hub 相同的 X-Agent-Token，
    // 否则任何能连到端口的人都能伪造设备快照 / 窃取命令队列与待传文件
    let token = headers
        .get("x-agent-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if token != state.config.agent_token {
        return err(401, "agent 上报令牌无效（AM_AGENT_TOKEN 需与 hub 一致）");
    }
    if payload.machine_id == state.config.machine_id {
        return err(400, "machineId 与 hub 本机冲突，请为 agent 指定 AM_MACHINE_ID");
    }
    // 登记设备（首次见到 → pending，等 owner 在设备管理里信任）
    state
        .registry
        .write()
        .await
        .ensure_device(&payload.machine_id, payload.owner.as_deref(), false);

    let mut machines = state.machines.write().await;
    let entry = machines
        .entry(payload.machine_id.clone())
        .or_insert_with(|| MachineEntry {
            hostname: payload.hostname.clone(),
            platform: payload.platform.clone(),
            version: payload.version.clone(),
            is_hub: false,
            tasks: Vec::new(),
            last_report: Instant::now(),
            pending: VecDeque::new(),
            pending_files: VecDeque::new(),
            messages: HashMap::new(),
        });
    entry.hostname = payload.hostname;
    entry.platform = payload.platform;
    entry.version = payload.version;
    entry.last_report = Instant::now();
    let mut tasks = payload.tasks;
    for t in tasks.iter_mut() {
        if !t.recent_messages.is_empty() {
            entry.messages.insert(t.id.clone(), std::mem::take(&mut t.recent_messages));
        }
    }
    entry.tasks = tasks;
    let commands: Vec<ControlCmd> = entry.pending.drain(..).collect();
    let files: Vec<crate::model::FileTransfer> = entry.pending_files.drain(..).collect();
    // 告知 agent 是否已被信任：未信任时 agent 不应再上报任何会话数据
    let trusted = state.registry.read().await.device_meta(&payload.machine_id).trusted;
    ok(json!({ "commands": commands, "files": files, "trusted": trusted }))
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

/// GET /monitor/ws?token=xxx —— 按用户过滤的实时任务快照（浏览器 WS 无法带 header，token 走 query）
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    Query(q): Query<WsQuery>,
) -> impl IntoResponse {
    let user = match &q.token {
        Some(t) => state.tokens.read().await.get(t).cloned(),
        None => None,
    };
    ws.on_upgrade(move |socket| ws_loop(socket, state, user))
}

async fn ws_loop(socket: WebSocket, state: SharedState, user: Option<String>) {
    let (mut tx, mut rx) = socket.split();
    let Some(user) = user else {
        let _ = tx
            .send(Message::Text(json!({ "type": "error", "msg": "未登录" }).to_string()))
            .await;
        return;
    };

    // 推送当前用户可见的活跃会话快照
    async fn snapshot(state: &SharedState, user: &str) -> String {
        let tasks: Vec<_> = state
            .tasks_for(user)
            .await
            .into_iter()
            .filter(|t| t.status != TaskStatus::Finished)
            .collect();
        serde_json::to_string(&json!({ "type": "tasks", "data": tasks })).unwrap_or_default()
    }

    if tx.send(Message::Text(snapshot(&state, &user).await)).await.is_err() {
        return;
    }
    let mut sub = state.tx.subscribe();
    loop {
        tokio::select! {
            msg = sub.recv() => {
                match msg {
                    Ok(_tick) => {
                        if tx.send(Message::Text(snapshot(&state, &user).await)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
