use crate::admin::{self, auth_user, err, ok};
use am_core::model::{ControlCmd, ControlReq, ReportPayload, Task, TaskStatus};
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
use tower_http::services::ServeDir;

/// agent 上报的请求体上限（32MB）。仍保留上限：该接口虽有令牌校验，
/// 但不设限等于给任何持令牌方一个无界内存分配入口。
const REPORT_BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn router(state: SharedState) -> Router {
    // downloads 目录解析要用 data_dir，router 组装尾部 state 已被 with_state 消费
    let state_dl = state.clone();
    let mut router = Router::new()
        // ---- 版本（供客户端/移动端更新检测）----
        .route("/monitor/version", get(version_info))
        // ---- vita-admin 契约 ----
        .route("/auth/access/getCryptoKey", get(admin::get_crypto_key))
        .route("/auth/access/isNeedLoginCaptcha", get(admin::is_need_captcha))
        .route("/auth/access/dingtalk/url", get(admin::dingtalk_url))
        .route("/auth/access/login", post(admin::login))
        // 客户端静默续登：设备令牌换登录会话（设备已绑定账号 = 该机即该用户）
        .route("/monitor/client/session", post(client_session))
        .route("/auth/access/register", post(admin::register))
        .route("/auth/access/logout", get(admin::logout))
        .route("/sys/menu/getMenuATopATopMenu", get(admin::menus))
        .route("/sys/menu/getStringPermissions", get(admin::permissions))
        // ---- 第三方登录（Google / Apple，按环境变量启用）----
        .route("/auth/access/oauth/providers", get(crate::oauth::oauth_providers))
        .route("/auth/access/oauth/:provider/url", get(crate::oauth::oauth_url))
        .route("/auth/access/oauth/:provider/login", post(crate::oauth::oauth_login))
        // Apple form_post 回调（POST）→ 转跳前端登录页
        .route("/auth/access/oauth/apple/callback", post(crate::oauth::apple_callback))
        // ---- 后管（admin token 锁 + 仅用户管理）----
        .route("/sys/admin/verify", post(admin::verify_admin_token))
        .route("/sys/user/page", get(admin::user_page))
        .route("/sys/user/add", post(admin::user_add))
        .route("/sys/user/upd", post(admin::user_upd))
        .route("/sys/user/resetPwd", post(admin::user_reset_pwd))
        .route("/sys/user/del", get(admin::user_del))
        // ---- 版本管理 / 更新日志（后管）----
        .route("/sys/version/info", get(admin::version_admin_info))
        .route("/sys/version/minimum", post(admin::version_set_minimum))
        .route("/sys/version/changelog", post(admin::changelog_add))
        .route("/sys/version/changelog/del", post(admin::changelog_del))
        // ---- 任务监控 API（前台公开使用）----
        .route("/monitor/tasks", get(list_tasks))
        .route("/monitor/tasks/page", get(page_tasks))
        .route("/monitor/tasks/detail/:id", get(task_detail))
        .route("/monitor/tasks/:id/messages", get(task_messages))
        .route("/monitor/tasks/:id/git-diff", get(task_git_diff))
        .route("/monitor/tasks/:id/slash-commands", get(task_slash_commands))
        .route("/monitor/tasks/:id/control", post(control_task))
        .route("/monitor/tasks/:id/input", post(input_task))
        .route("/monitor/machines", get(machines))
        .route("/monitor/agent", get(agent_status))
        .route("/monitor/ws", get(ws_handler))
        // ---- 设备管理（信任设备）----
        .route("/monitor/devices", get(list_devices))
        .route("/monitor/devices/:id/trust", post(trust_device))
        .route("/monitor/devices/:id/untrust", post(untrust_device))
        .route("/monitor/devices/:id", axum::routing::delete(delete_device))
        .route("/monitor/devices/:id/upload", post(upload_file))
        // ---- 设备配对（注册+安装即可用，无需管理员发令牌）----
        .route("/monitor/pair/start", post(pair_start))
        .route("/monitor/pair/claim", post(pair_claim))
        .route("/monitor/pair/status", get(pair_status))
        // ---- agent → hub 上报 ----
        // 单独放宽体积上限：axum 默认 2MB，一台机器会话多、消息长时很容易顶到，
        // 一旦 413 该设备就再也同步不上来了。
        .route(
            "/monitor/report",
            post(report).layer(axum::extract::DefaultBodyLimit::max(REPORT_BODY_LIMIT)),
        )
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

    // 客户端安装包下载（官网「客户端」区直链）。
    // 目录：AM_DOWNLOADS_DIR > 数据目录/downloads；不存在则不挂载（官网点击 404）。
    // 包内 config.txt 的上报令牌是占位符（公开可下载，真实令牌由管理员单发），
    // 故无需鉴权。
    let downloads_dir = std::env::var("AM_DOWNLOADS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state_dl.config.data_dir.join("downloads"));
    if downloads_dir.is_dir() {
        tracing::info!("托管客户端下载: {}", downloads_dir.display());
        router = router.nest_service("/downloads", ServeDir::new(&downloads_dir));
    }

    // 静态托管前端构建产物（存在时）：先按真实文件命中，未命中的路径
    // （SPA 前端路由，如 /portal、/admin）回退到 index.html 且以 200 返回。
    // 注意：ServeDir 的 not_found_service 会沿用 404 状态，导致深链/刷新报 404，
    // 这里改用显式 fallback handler 保证返回 200。
    if let Some(dist) = web_dist_dir() {
        tracing::info!("托管前端静态资源: {}", dist.display());
        let index = std::sync::Arc::new(dist.join("index.html"));
        let spa_index = index.clone();
        let spa_fallback = axum::routing::get(move || {
            let index = spa_index.clone();
            async move {
                match tokio::fs::read(index.as_ref()).await {
                    Ok(bytes) => (
                        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        bytes,
                    )
                        .into_response(),
                    Err(_) => (axum::http::StatusCode::NOT_FOUND, "index.html 缺失").into_response(),
                }
            }
        });
        router = router.fallback_service(ServeDir::new(&dist).fallback(spa_fallback));
    }
    router
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairStartReq {
    machine_id: String,
    hostname: String,
    platform: String,
}

/// POST /monitor/pair/start —— 客户端领配对码（公开）。
/// 返回 code（给用户/网页认领用）与 pairToken（客户端轮询凭证）。
async fn pair_start(State(state): State<SharedState>, Json(req): Json<PairStartReq>) -> Json<Value> {
    if req.machine_id.trim().is_empty() {
        return err(400, "缺少 machineId");
    }
    let mut map = state.pair_codes.write().await;
    map.retain(|_, e| !e.expired());
    // 容量兜底：防被刷爆内存
    if map.len() >= 5000 {
        return err(429, "配对请求过多，请稍后再试");
    }
    // 8 位大写码，避开易混淆字符
    const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let code: String = (0..8).map(|_| ALPHA[rng.gen_range(0..ALPHA.len())] as char).collect();
    let pair_token = uuid::Uuid::new_v4().simple().to_string();
    map.insert(
        code.clone(),
        crate::state::PairEntry {
            machine_id: req.machine_id.trim().to_string(),
            hostname: req.hostname,
            platform: req.platform,
            pair_token: pair_token.clone(),
            created: Instant::now(),
            device_token: None,
        },
    );
    ok(json!({ "code": code, "pairToken": pair_token }))
}

#[derive(Deserialize)]
struct PairClaimReq {
    code: String,
}

/// POST /monitor/pair/claim —— 已登录用户认领设备：绑定到自己名下并签发设备令牌。
async fn pair_claim(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<PairClaimReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let code = req.code.trim().to_uppercase();
    let mut map = state.pair_codes.write().await;
    let Some(entry) = map.get_mut(&code) else {
        return err(404, "配对码不存在或已过期，请在客户端重新发起");
    };
    if entry.expired() {
        map.remove(&code);
        return err(404, "配对码已过期，请在客户端重新发起");
    }
    if entry.device_token.is_some() {
        return err(400, "该配对码已被认领");
    }
    let token = state.registry.write().await.bind_device(&entry.machine_id, &user);
    entry.device_token = Some(token);
    tracing::info!("设备配对成功: {} → 用户 {user}", entry.machine_id);
    ok(json!({ "machineId": entry.machine_id, "hostname": entry.hostname, "platform": entry.platform }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairStatusQuery {
    code: String,
    pair_token: String,
}

/// GET /monitor/pair/status —— 客户端轮询：认领完成即领走设备令牌（一次性）。
async fn pair_status(
    State(state): State<SharedState>,
    Query(q): Query<PairStatusQuery>,
) -> Json<Value> {
    let mut map = state.pair_codes.write().await;
    let code = q.code.trim().to_uppercase();
    let Some(entry) = map.get(&code) else {
        return ok(json!({ "claimed": false, "expired": true }));
    };
    // pairToken 不匹配按不存在处理：防他人凭 code 轮询窃取设备令牌
    if !crate::state::token_eq(&entry.pair_token, &q.pair_token) {
        return ok(json!({ "claimed": false, "expired": true }));
    }
    if entry.expired() {
        map.remove(&code);
        return ok(json!({ "claimed": false, "expired": true }));
    }
    if let Some(token) = entry.device_token.clone() {
        map.remove(&code); // 一次性：令牌交付即销毁配对条目
        return ok(json!({ "claimed": true, "deviceToken": token }));
    }
    ok(json!({ "claimed": false, "expired": false }))
}

/// GET /monitor/version —— 最新版本信息（客户端/移动端更新检测用，公开）。
/// desktop = hub 自身版本（同一代码库）；android 读 downloads/manifest.json（打包时写入）。
async fn version_info(State(state): State<SharedState>) -> Json<Value> {
    let downloads_dir = std::env::var("AM_DOWNLOADS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.config.data_dir.join("downloads"));
    let manifest = tokio::fs::read_to_string(downloads_dir.join("manifest.json"))
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null);
    let pick = |ptr: &str| manifest.pointer(ptr).and_then(Value::as_str).map(String::from);
    // minVersion = 强制更新下限：低于它的客户端必须更新才能继续使用
    // （有根本性协议/安全变更时在 manifest.json 里抬高对应字段）
    ok(json!({
        "desktop": env!("CARGO_PKG_VERSION"),
        "desktopMin": pick("/desktop/minVersion"),
        "android": pick("/android/version"),
        "androidMin": pick("/android/minVersion"),
    }))
}

/// 强制更新下限（桌面端，随上报响应下发）；manifest 缺失时无强制
async fn desktop_min_version(state: &SharedState) -> Option<String> {
    let downloads_dir = std::env::var("AM_DOWNLOADS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.config.data_dir.join("downloads"));
    tokio::fs::read_to_string(downloads_dir.join("manifest.json"))
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|m| m.pointer("/desktop/minVersion").and_then(Value::as_str).map(String::from))
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
    // 页码由请求方给定且无上界，(n-1)*s 直接算会溢出（release 下回绕成任意
    // skip 返回错误页，debug 下直接 panic）。饱和运算下超大页码只会得到空列表。
    let items: Vec<_> = filtered
        .into_iter()
        .skip(page_num.saturating_sub(1).saturating_mul(page_size))
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

/// GET /monitor/tasks/:id/messages —— 读所属机器上报的消息缓存
async fn task_messages(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<MsgQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let _limit = q.limit.unwrap_or(120).clamp(1, 500);
    let machine_id = {
        let tasks = state.tasks_for(&user).await;
        tasks.iter().find(|t| t.id == id).map(|t| t.machine_id.clone())
    };
    let Some(machine_id) = machine_id else {
        return err(404, "任务不存在");
    };

    let machines = state.machines.read().await;
    let list = machines
        .get(&machine_id)
        .and_then(|m| m.messages.get(&id))
        .cloned()
        .unwrap_or_default();
    ok(json!({ "list": list }))
}

/// GET /monitor/tasks/:id/git-diff —— 会话项目目录的 git 改动概览（原文件 vs 修改后）。
/// 本机会话直接计算；远程会话下发请求给 agent，返回缓存结果（首次可能 pending，前端轮询）。
async fn task_git_diff(
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
    let cwd = task.process.as_ref().map(|p| p.cwd.clone()).unwrap_or_default();

    // 读缓存；同时下发一个请求让 agent 刷新（去重：同 task 已在队列则不重复入队）
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return err(404, "任务所属机器已离线");
    };
    if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
        return err(500, "任务所属机器已离线");
    }
    let cached = entry.git_cache.get(&id).cloned();
    if !entry.pending_git.iter().any(|q| q.task_id == id) {
        entry.pending_git.push_back(am_core::model::GitQuery {
            task_id: id.clone(),
            cwd,
        });
    }
    match cached {
        Some(overview) => ok(json!({ "overview": overview, "pending": false })),
        None => ok(json!({ "overview": null, "pending": true })),
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
    // hub 无法扫描远端机器的自定义命令目录，统一给该模型的内置命令集
    let list = crate::commands::collect(&task.provider, "");
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

    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return err(404, "任务所属机器已离线");
    };
    if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
        return err(500, "任务所属机器已离线，无法下发");
    }
    tracing::info!("已向机器 {} 下发输入: {}", task.machine_id, truncate_log(&text));
    entry.pending.push_back(ControlCmd {
        task_id: id.clone(),
        pid,
        action: am_core::model::ControlAction::Input,
        text: Some(text),
    });
    ok(json!({ "pid": pid, "result": "已下发到目标机器" }))
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

    {
        // 目标目录合法性由目标机权威校验：hub 是 Linux、目标机是 Mac 时，
        // /Users/xxx 这种目标机上完全合法的路径在 hub 侧无从判断。
        // 进文件队列由 agent 拉取，agent 侧用自己的 upload_root 复验后写入。
        let mut machines = state.machines.write().await;
        let Some(entry) = machines.get_mut(&id) else {
            return err(404, "设备不存在或已离线");
        };
        if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
            return err(500, "设备已离线，无法传输");
        }
        entry.pending_files.push_back(am_core::model::FileTransfer {
            dir,
            filename: safe_name,
            content_b64: B64.encode(&bytes),
        });
        ok(json!({ "result": "已下发到目标设备，等待写入", "size": bytes.len() }))
    }
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
    ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": state.started_at.to_rfc3339(),
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
    // 鉴权（两通道）：
    // 1) 每设备令牌（x-device-token）——配对绑定时签发，普通用户唯一路径；
    // 2) 全局 agent 令牌（x-agent-token）——内部部署/兼容旧客户端。
    let device_token = headers
        .get("x-device-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let global_token = headers
        .get("x-agent-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let dev_ok = !device_token.is_empty()
        && state
            .registry
            .read()
            .await
            .verify_device_token(&payload.machine_id, device_token);
    let global_ok = crate::state::token_eq(global_token, &state.config.agent_token);
    if !dev_ok && !global_ok {
        return err(401, "设备未绑定账号：打开客户端窗口登录一次即可自动绑定");
    }
    // AM_USER 填了就必须是真实存在的账号。
    // 不校验的话，拼错一个字母就会登记成一台「谁都看不到、也无法信任」的孤儿设备：
    // owned_by 是严格相等，devices_for 也没有超管兜底，用户却只会看到托盘上
    // 一句「已连接 · 待信任」，然后在网页上永远找不到这台机器。
    // 这里直接拒绝，agent 会把原因显示到托盘上（见 agent::describe_reject）。
    let claim_owner = if dev_ok { None } else { payload.owner.as_deref() };
    if let Some(owner) = claim_owner.filter(|o| !o.is_empty()) {
        if !state.registry.read().await.user_exists(owner) {
            return err(
                400,
                &format!("AM_USER 指定的账号「{owner}」不存在，请核对客户端配置"),
            );
        }
    }
    // 登记设备：新设备默认信任（信任开关是「断开链接」的手段，撤销有粘性，
    // 不会被后续上报重新打开）；顺带刷新展示信息（内部有变更/节流判断，
    // 不会把注册表写穿），离线后设备管理里仍能看到这台机器
    {
        let mut reg = state.registry.write().await;
        reg.ensure_device(&payload.machine_id, claim_owner, true);
        reg.update_device_info(
            &payload.machine_id,
            &payload.hostname,
            &payload.platform,
            &payload.version,
        );
    }

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
            pending_git: VecDeque::new(),
            git_cache: HashMap::new(),
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
    // 缓存 agent 回传的 git 对比结果
    for r in payload.git_results {
        entry.git_cache.insert(r.task_id, r.overview);
    }
    // 清掉已消失会话的缓存：这两张表按会话 ID 累积，不清理的话
    // hub 长期运行会随「历史会话总数」无限增长（而非「当前会话数」）。
    let alive: std::collections::HashSet<&str> =
        entry.tasks.iter().map(|t| t.id.as_str()).collect();
    entry.messages.retain(|k, _| alive.contains(k.as_str()));
    entry.git_cache.retain(|k, _| alive.contains(k.as_str()));
    let commands: Vec<ControlCmd> = entry.pending.drain(..).collect();
    let files: Vec<am_core::model::FileTransfer> = entry.pending_files.drain(..).collect();
    let git_queries: Vec<am_core::model::GitQuery> = entry.pending_git.drain(..).collect();
    // 告知 agent 是否已被信任：未信任时 agent 不应再上报任何会话数据
    let trusted = state.registry.read().await.device_meta(&payload.machine_id).trusted;
    // hubVersion：hub 与桌面客户端同一工作区发版，hub 的版本即最新客户端版本，
    // agent 用它做更新提示（托盘「新版本可用」）
    ok(json!({
        "commands": commands,
        "files": files,
        "gitQueries": git_queries,
        "trusted": trusted,
        "hubVersion": env!("CARGO_PKG_VERSION"),
        // 强制更新下限：客户端低于它必须更新才能继续使用
        "minVersion": desktop_min_version(&state).await,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientSessionReq {
    machine_id: String,
    device_token: String,
}

/// POST /monitor/client/session —— 桌面客户端静默续登。
/// 设备令牌是绑定时签发、持久化在客户端本机的长期凭证；持有它即等于
/// 「这台已绑定的机器」，据此给归属用户签发一个网页会话 —— 客户端登录
/// 一次后，之后会话过期/服务重启都由客户端自动换新，用户无感知。
async fn client_session(
    State(state): State<SharedState>,
    Json(req): Json<ClientSessionReq>,
) -> Json<Value> {
    let (is_super, user) = {
        let reg = state.registry.read().await;
        if !reg.verify_device_token(&req.machine_id, &req.device_token) {
            return err(401, "设备未绑定或令牌无效，请重新登录绑定");
        }
        let Some(owner) = reg.device_meta(&req.machine_id).owner else {
            return err(401, "设备无归属账号，请重新登录绑定");
        };
        let Some(user) = reg.user_by_name(&owner).cloned() else {
            return err(401, "归属账号已不存在，请重新登录");
        };
        (reg.is_super_user(&owner), user)
    };
    let token = uuid::Uuid::new_v4().to_string();
    state
        .tokens
        .write()
        .await
        .insert(token.clone(), crate::state::Session::new(user.username.clone()));
    state.sessions_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    let nickname = if user.display.is_empty() {
        user.username.clone()
    } else {
        user.display.clone()
    };
    tracing::info!("客户端静默续登: {}（machine={}）", user.username, req.machine_id);
    ok(json!({
        "token": token,
        "userInfo": {
            "id": user.id,
            "username": user.username,
            "nickname": nickname,
            "isSuper": is_super,
        }
    }))
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
    // 同 auth_user：过期会话一律当未登录
    let user = match &q.token {
        Some(t) => state
            .tokens
            .read()
            .await
            .get(t)
            .filter(|s| !s.expired())
            .map(|s| s.username.clone()),
        None => None,
    };
    let token = q.token.clone().unwrap_or_default();
    ws.on_upgrade(move |socket| ws_loop(socket, state, user, token))
}

async fn ws_loop(socket: WebSocket, state: SharedState, user: Option<String>, token: String) {
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
    // 登录态是否仍然有效。握手时校验过一次是不够的：这条流会持续推送该用户的
    // 全部会话快照（含 prompt 与 cwd），若不复验，退出登录 / 会话过期 / 管理员
    // 删号都切不断它 —— 被窃取的 token 一旦升级成 WS 就是永久且不可撤销的读权限。
    async fn still_valid(state: &SharedState, token: &str, user: &str) -> bool {
        match state.tokens.read().await.get(token) {
            Some(s) => !s.expired() && s.username == user,
            None => false,
        }
    }

    let mut sub = state.tx.subscribe();
    loop {
        tokio::select! {
            msg = sub.recv() => {
                match msg {
                    Ok(_tick) => {
                        if !still_valid(&state, &token, &user).await {
                            let _ = tx
                                .send(Message::Text(
                                    json!({ "type": "error", "msg": "登录态已失效" }).to_string(),
                                ))
                                .await;
                            break;
                        }
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
