use crate::admin::{self, auth_user, err, ok};
use am_core::model::{ControlCmd, ControlReq, ReportPayload, Task, TaskStatus};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use crate::state::{MachineEntry, SharedState, NEW_SESSION_SETTLE_SECS, OFFLINE_AFTER_SECS};
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

/// 上传接口的请求体上限（12MB）。
///
/// 此前这个路由没设过 limit，吃的是 axum 默认的 **2MB** —— 传张大点的截图都会被拒，
/// 而错误只是一个干巴巴的 413，前端看着像"上传失败"，根本猜不到是体积卡的。
///
/// 定 12MB 是配合前端 5MB 的分片粒度（见 web 的 UPLOAD_CHUNK_SIZE）：单片 5MB 加上
/// multipart 边界与其它字段绰绰有余，也给非分片路径的中等文件留了空间。
/// 不往更大放是因为 hub 会把整片读进内存再 base64（膨胀 1/3），上限就是并发上传时的
/// 内存底数。
const UPLOAD_BODY_LIMIT: usize = 12 * 1024 * 1024;

/// 「会话已结束」推送的新鲜度门槛：会话最后一次有动静距今超过这么久，
/// 它从上报里消失时就静默清理、不再打扰。
///
/// 关掉终端窗口后 claude 进程常会残留，这类僵尸会话可能挂上几小时；
/// 客户端换版本导致扫描口径变化、历史会话滑出窗口，也会让一批陈年会话
/// 同时消失。真正在用的会话刚结束时 mtime 就在几分钟内，1 小时足够宽松。
const STALE_FINISH_SECS: u64 = 3600;

/// 一个从上报里消失的会话，值不值得推「会话已结束」。
///
/// 只看它最后一次有动静距今多久：刚还在用的值得说一声，几小时没动静的
/// 说了也只是打扰。`mtime_ms` 为 0（拿不到时间）时按「很旧」处理 ——
/// 信息不足就别打扰，漏推一条远好过半夜莫名其妙响一下。
fn worth_finish_notice(mtime_ms: u64, now_secs: u64) -> bool {
    let idle_secs = now_secs.saturating_mul(1000).saturating_sub(mtime_ms) / 1000;
    idle_secs <= STALE_FINISH_SECS
}

/// 「进程占位任务」：只扫到 agent 进程、还没配上会话文件时，客户端先造一条空壳任务占位
///（见 core `scanner::build_tasks` 尾部：id 为 `pid-<pid>`，再由 `attach_machine` 加机器前缀）。
///
/// 它消失几乎总是好事而非坏事：一给它下发任务，claude 就落了 jsonl，下一轮扫描把进程配到
/// 真会话上，占位任务功成身退、换成真会话 id 继续。此刻推「会话已结束」纯属噪音 —— 卡片上
/// 只有一个「Claude Code」，既认不出是哪个，也没有任何结果可看（消息都记在新 id 名下）。
///
/// 光靠文本口径挡不住它：占位任务的标题恒为终端名「Claude Code」、提示词恒为
///「（会话尚未产生记录）」，两者都非空。只能认 id。
fn is_proc_placeholder(t: &am_core::model::Task) -> bool {
    is_proc_placeholder_id(&t.id, &t.machine_id)
}

/// [`is_proc_placeholder`] 的纯字符串内核（Task 字段太多，测试直接打这一层）
fn is_proc_placeholder_id(id: &str, machine_id: &str) -> bool {
    // 机器前缀剥不掉时（machine_id 为空 / 老客户端没加前缀）原样回退，兜底认裸形态
    let rest = id.strip_prefix(machine_id).unwrap_or(id);
    rest.starts_with("-pid-") || rest.starts_with("pid-")
}

pub fn router(state: SharedState) -> Router {
    // downloads 目录解析要用 data_dir，router 组装尾部 state 已被 with_state 消费
    let state_dl = state.clone();
    let mut router = Router::new()
        // ---- 版本（供客户端/移动端更新检测）----
        .route("/monitor/version", get(version_info))
        // ---- MCP（Streamable HTTP）：给 AI 客户端做多会话编排用，鉴权同网页 ----
        .route("/mcp", post(crate::mcp::mcp_post).get(crate::mcp::mcp_get))
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
        .route("/sys/dingtalk/app", get(admin::dingtalk_app_admin_get))
        .route("/sys/dingtalk/app", post(admin::dingtalk_app_admin_set))
        // ---- 任务监控 API（前台公开使用）----
        .route("/monitor/me", get(me_info))
        .route("/monitor/tasks", get(list_tasks))
        .route("/monitor/history", get(list_history))
        .route("/monitor/tasks/page", get(page_tasks))
        .route("/monitor/tasks/detail/:id", get(task_detail))
        .route("/monitor/tasks/:id/messages", get(task_messages))
        .route("/monitor/tasks/:id/slash-commands", get(task_slash_commands))
        .route("/monitor/tasks/:id/control", post(control_task))
        .route("/monitor/tasks/:id/input", post(input_task))
        .route("/monitor/tasks/:id/termkey", post(termkey_task))
        .route("/monitor/tasks/:id/queued", get(queued_inputs))
        .route("/monitor/tasks/:id/recall", post(recall_input))
        .route("/monitor/tasks/:id/dirs", get(task_dirs))
        .route("/monitor/tasks/:id/fsop", post(task_fsop))
        .route("/monitor/tasks/:id/fsop/:opid", get(task_fsop_result))
        .route("/monitor/machines", get(machines))
        .route("/monitor/agent", get(agent_status))
        .route("/monitor/ws", get(ws_handler))
        // ---- 设备管理（信任设备）----
        .route("/monitor/devices", get(list_devices))
        .route("/monitor/devices/:id/trust", post(trust_device))
        .route("/monitor/devices/:id/untrust", post(untrust_device))
        .route("/monitor/devices/:id", axum::routing::delete(delete_device))
        .route(
            "/monitor/devices/:id/upload",
            post(upload_file).layer(axum::extract::DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        // ---- 协助共享（跨用户设备接入，类似远程控制）----
        .route("/monitor/share/:id", get(share_info).post(share_create).delete(share_revoke))
        .route("/monitor/share/:id/guests", get(share_guests))
        .route("/monitor/share/:id/kick", post(share_kick))
        .route("/monitor/share/connect", post(share_connect))
        .route("/monitor/share/disconnect", post(share_disconnect))
        // ---- 配置同步（Claude Code / Codex 的 md 类配置跨设备镜像）----
        .route("/monitor/config/sync", get(config_sync_status))
        .route("/monitor/config/source", post(set_config_source))
        // ---- 用户自助机器人集成 ----
        // 配置读写（登录用户，返回各渠道配置 + 专属回调地址）
        .route("/monitor/integrations", get(integrations_get))
        .route("/monitor/integrations/dingtalk-recv-dir", post(set_dingtalk_recv_dir))
        .route("/monitor/integrations/dingtalk-app", post(set_dingtalk_app))
        // 钉钉号绑定（走管理员的全局机器人时才需要）：取码 / 认领链接 / 查看 / 解绑
        .route("/monitor/integrations/dingtalk-bindcode", post(dingtalk_bind_code))
        .route("/monitor/integrations/dingtalk-qr", get(dingtalk_qr))
        // 扫码回调：人在手机钉钉里打开，没有登录态，故不鉴权（凭一次性 state 认人）
        .route("/monitor/integrations/dingtalk-scan", get(dingtalk_scan_cb))
        .route("/monitor/integrations/dingtalk-bind", post(dingtalk_bind))
        .route("/monitor/integrations/dingtalk-ids", get(dingtalk_ids_get))
        .route("/monitor/integrations/dingtalk-unbind", post(dingtalk_unbind))
        // 回调（每用户 channel 路由）
        .route("/monitor/int/dingtalk/:channel", post(crate::bot::dingtalk_message))
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
                        [
                            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                            // HTML 不带 Cache-Control 时 WKWebView 会启发式缓存，
                            // 发版后客户端拿旧页 —— 强制每次向服务器校验
                            (axum::http::header::CACHE_CONTROL, "no-cache"),
                        ],
                        bytes,
                    )
                        .into_response(),
                    Err(_) => (axum::http::StatusCode::NOT_FOUND, "index.html 缺失").into_response(),
                }
            }
        });
        use tower_http::set_header::SetResponseHeaderLayer;
        // /static/ 下是带内容 hash 的文件名（改动即换名），可长期强缓存 ——
        // 第二次及以后启动无需重新下载 JS/CSS，直接命中缓存，秒开。
        let static_dir = dist.join("static");
        let static_assets = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::overriding(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
            ))
            .service(ServeDir::new(&static_dir));
        router = router.nest_service("/static", static_assets);

        // 其余(index.html / build-id.txt 等)统一 no-cache：每次向服务器校验，
        // 配合前端构建号守卫，发版后能拿到指向新 hash 资源的新 index.html。
        let static_srv = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::if_not_present(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache"),
            ))
            .service(ServeDir::new(&dist).fallback(spa_fallback));
        router = router.fallback_service(static_srv);
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

/// 支持分片写入的最低 agent 版本。低于它的客户端不认识 FileTransfer 的 chunk_* 字段，
/// 会把每一片都当整份覆盖写。
const CHUNKED_UPLOAD_MIN_VER: (u32, u32, u32) = (0, 10, 5);

/// 该 agent 版本是否支持分片写入。
///
/// 版本号解析不出来时按「不支持」处理：宁可让大文件走不通、给出明确提示，也不能赌 ——
/// 赌错的代价是文件被静默写坏，而用户还以为传成功了。
fn agent_supports_chunked(ver: &str) -> bool {
    let mut it = ver.trim().trim_start_matches('v').split('.');
    let parse = |x: Option<&str>| -> Option<u32> {
        // 容忍 "0.10.5-beta1" 这类后缀：只取前导数字
        let s = x?.trim();
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    };
    match (parse(it.next()), parse(it.next()), parse(it.next())) {
        (Some(a), Some(b), Some(c)) => (a, b, c) >= CHUNKED_UPLOAD_MIN_VER,
        _ => false,
    }
}

/// 把固定名安装包对齐到最新版本。
///
/// 客户端自更新固定去下 `agent-monitor-setup.exe`（见 client 的 `do_self_update`），而每次
/// 发版上传的是带版本号的 `AgentMonitor-<v>-setup.exe` —— 两者此前靠发布流程手工保持一致。
///
/// 漏更新一次的后果特别隐蔽：客户端把「旧包」完整下载、静默安装，两步都成功，只是版本号
/// 没变，于是自己判定「安装没生效」并停止重试，提示用户手动下载。排查时看到的是安装环节
/// 报错，真正的病灶却在服务端少复制了一个文件。线上就这么发生过（0.10.0 发布后固定名仍
/// 停在 0.9.8，客户端反复把 0.9.8 装了一遍又一遍）。
///
/// 交给 hub 定期核对：发版只要放好版本化的包，固定名自动跟上。
pub(crate) fn sync_fixed_installer(downloads_dir: &std::path::Path) {
    let ver = ready_desktop_version(downloads_dir);
    let src = downloads_dir.join(format!("AgentMonitor-{ver}-setup.exe"));
    let dst = downloads_dir.join("agent-monitor-setup.exe");
    let Ok(meta) = std::fs::metadata(&src) else {
        return; // 版本化的包还没传上来，等下一轮
    };
    // 只比大小：每次发版内容必变，大小相同即认为已是同一个包。比 mtime 稳 ——
    // 复制出来的副本 mtime 天然与源不同，拿它比会每轮都重复复制。
    if std::fs::metadata(&dst).map(|d| d.len()).ok() == Some(meta.len()) {
        return;
    }
    // 先写临时再 rename：直接覆盖的话，正在下载的客户端会拿到写了一半的文件
    let tmp = downloads_dir.join("agent-monitor-setup.exe.part");
    if std::fs::copy(&src, &tmp).is_ok() && std::fs::rename(&tmp, &dst).is_ok() {
        tracing::info!("固定名安装包已对齐到 v{ver}");
    } else {
        let _ = std::fs::remove_file(&tmp);
        tracing::warn!("固定名安装包对齐失败（v{ver}），客户端自更新会下到旧包");
    }
}

/// 已就绪、可对外推送的桌面版本：downloads 里已存在 `AgentMonitor-<v>-setup.exe`
/// 的最高版本（不超过 hub 自身版本）。
///
/// hub 一部署就会按自身版本推送，但对应安装包往往还要几分钟才构建/上传完；这期间
/// 若照 hub 版本推送，客户端会去下还没传好的包、拿到旧包打转。改为只推送「安装包已
/// 上传」的版本，上传完成后自然开始推送，彻底避免「推送早于构建完成」。
fn ready_desktop_version(downloads_dir: &std::path::Path) -> String {
    let hub_ver = env!("CARGO_PKG_VERSION");
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        let mut it = v.split('.');
        Some((
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
            it.next()?.parse().ok()?,
        ))
    };
    // 「最新桌面版」= downloads 里已就绪的最高版本安装包 AgentMonitor-<ver>-setup.exe。
    // 不再拿 hub 自身编译版本（env!CARGO_PKG_VERSION）当上限：客户端自更新下载的是固定名
    // agent-monitor-setup.exe，与 hub 版本无关；而版本化安装包「最后上传」本就是该版发布完成
    // 的信号。若还卡 hub 版本，只要部署的 hub 二进制落后于已发布客户端，检查更新就会报旧版本
    // （= 用户遇到的「检测到的最新版落后于实际最新版」）。hub_ver 仅作 downloads 为空时的兜底。
    let mut best: Option<((u32, u32, u32), String)> = None;
    if let Ok(rd) = std::fs::read_dir(downloads_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(v) = name
                .strip_prefix("AgentMonitor-")
                .and_then(|s| s.strip_suffix("-setup.exe"))
            {
                if let Some(t) = parse(v) {
                    if best.as_ref().map_or(true, |(bt, _)| t > *bt) {
                        best = Some((t, v.to_string()));
                    }
                }
            }
        }
    }
    best.map(|(_, v)| v).unwrap_or_else(|| hub_ver.to_string())
}

/// GET /monitor/version —— 最新版本信息（客户端/移动端更新检测用，公开）。
/// desktop = 已就绪可推送的桌面版本；android 读 downloads/manifest.json（打包时写入）。
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
        "desktop": ready_desktop_version(&downloads_dir),
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
/// GET /monitor/me —— 当前登录用户信息（含实时 isSuper）。前端每次加载调一次刷新本地缓存，
/// 让「改了权限（如设成超级管理员）」无需重新登录即可生效。
async fn me_info(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(username) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let reg = state.registry.read().await;
    let is_super = reg.is_super_user(&username);
    let (id, nickname) = reg
        .user_by_name(&username)
        .map(|u| {
            let nick = if u.display.is_empty() { u.username.clone() } else { u.display.clone() };
            (u.id.clone(), nick)
        })
        .unwrap_or_default();
    ok(json!({ "id": id, "username": username, "nickname": nickname, "isSuper": is_super }))
}

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
    ok(json!({ "list": with_slots(&state, &user, &filtered).await }))
}

/// 给会话补上「号位」（钉钉里 `@N` 的 N），让网页/移动端与钉钉看到同一个编号 ——
/// 否则在网页上看着会话，却不知道该 @ 几号。
///
/// 号位的分配与去重统一由 `bot::sorted_active_tasks` 负责（那里保证了分配顺序稳定），
/// 这里**只按终端锚查、不分配**，避免两处各自分配导致编号不一致。
async fn with_slots(
    state: &SharedState,
    user: &str,
    tasks: &[am_core::model::Task],
) -> Vec<Value> {
    let by_anchor: std::collections::HashMap<String, u32> =
        crate::bot::sorted_active_tasks(state, user)
            .await
            .into_iter()
            .map(|(t, no)| (crate::slots::anchor_of(&t), no))
            .collect();
    tasks
        .iter()
        .map(|t| {
            let mut v = serde_json::to_value(t).unwrap_or_else(|_| json!({}));
            if let Some(obj) = v.as_object_mut() {
                // 查不到 = 该会话还没进过号位表（罕见），给 null 让前端不显示徽标
                obj.insert(
                    "slot".into(),
                    json!(by_anchor.get(&crate::slots::anchor_of(t))),
                );
            }
            v
        })
        .collect()
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
        id: None,
    });
    tracing::info!("已向机器 {} 下发控制命令: {id}", task.machine_id);
    ok(json!({ "pid": pid, "result": "命令已下发，等待执行" }))
}

#[derive(Deserialize)]
struct InputReq {
    text: String,
    pid: Option<u32>,
    /// 下发来源："client"/"web"（网页仍会带上，hub 目前不据此推送，保留兼容不报错）。
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<String>,
    /// 这条是在回答终端弹出的选择卡（选项序号或自定义答案），不是主动发的任务。
    ///
    /// 单看内容说明不了什么 —— 孤零零一个「1」，问题本身又不在流里。所以既不推钉钉
    /// 也不进交互历史，与前端把 fromSelect 排除出对话流的处理保持一致。
    /// 用显式 rename 而非给整个结构挂 rename_all：现有字段都是单词，不必为这一个改口径。
    #[serde(default, rename = "fromSelect")]
    from_select: bool,
}

/// 把 select 消息（AskUserQuestion 的整份 input JSON 文本）转成可读文本。解析失败返回空串。
fn select_options_text(content: &str) -> String {
    match serde_json::from_str::<Value>(content) {
        Ok(v) => select_summary(&v),
        Err(_) => String::new(),
    }
}

/// 把 AskUserQuestion 的 input 渲染成「问题 + 编号选项」。
///
/// 钉钉的「⌨️ 需要你选择」与 MCP 的 session_detail 共用这一份 —— 两处各写一套的话，
/// 哪天选项结构变了（比如加多选标记）只改一边，另一边就悄悄错了。
pub(crate) fn select_summary(v: &Value) -> String {
    let mut out = String::new();
    let Some(qs) = v.get("questions").and_then(|q| q.as_array()) else {
        return out;
    };
    let many = qs.len() > 1;
    for (qi, q) in qs.iter().enumerate() {
        // 多选与单选的作答方式完全不同（单选发一个序号即落定，多选要连写序号再补
        // Submit 的编号），不标出来的话，远端只能靠猜 —— 猜错就卡在选择卡上不动。
        let multi = q.get("multiSelect").and_then(|x| x.as_bool()).unwrap_or(false);
        // 题与题之间空一行。**这一行不能省**：markdown 的 lazy continuation 会把紧跟在
        // 列表项后面的文字当成该项的续行，于是下一题的题干被吞进上一题的最后一个选项，
        // 后面的有序列表还会被视为同一个列表的延续、自动接着编号。
        // 线上就这么翻过车：两道题在钉钉里被渲染成一道、选项连号成 1~6。
        if qi > 0 {
            out.push('\n');
        }
        if many {
            out.push_str(&format!("**第 {} 题**", qi + 1));
            if multi {
                out.push_str("（多选）");
            }
            out.push_str("  \n");
        }
        if let Some(question) = q.get("question").and_then(|x| x.as_str()) {
            out.push_str(question);
            if multi && !many {
                out.push_str("（多选）");
            }
            // 题干与选项列表之间同样要空行，列表才会被当作新列表起头
            out.push_str("\n\n");
        }
        if let Some(opts) = q.get("options").and_then(|o| o.as_array()) {
            for (i, o) in opts.iter().enumerate() {
                let label = o.get("label").and_then(|x| x.as_str()).unwrap_or("");
                out.push_str(&format!("{}. {}\n", i + 1, label));
            }
            if multi {
                // Submit 在终端选择卡里也占编号：N 个选项 + 「其它」占 N+1，Submit 是 N+2
                out.push_str(&format!(
                    "\n（多选：勾选的序号连写，末尾补 {}＝Submit，如 \"1{}\"）\n",
                    opts.len() + 2,
                    opts.len() + 2
                ));
            }
        }
    }
    if many {
        // 每题的选项都从 1 编号，而作答是一次一题 —— 不说明的话，看到两组「1.」很容易
        // 以为可以直接回第二题的序号。
        out.push_str("\n（多题：逐题作答，先回第 1 题的序号）");
    }
    out.trim_end().to_string()
}

/// 把一段 markdown 里的标题行（# ~ ######）改成加粗行：钉钉里 assistant 结果常带
/// `###### 小标题`，heading 会带大字号/上下间距，塞进推送里突兀；转成 **加粗** 更贴合。
fn md_headings_to_bold(s: &str) -> String {
    s.lines()
        .map(|line| {
            let t = line.trim_start();
            let hashes = t.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&hashes) && t.chars().nth(hashes) == Some(' ') {
                let title = t[hashes..].trim();
                if title.is_empty() {
                    line.to_string()
                } else {
                    format!("**{title}**")
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let cmd_id = uuid::Uuid::new_v4().to_string();
    let text_for_notify = text.clone();
    entry.pending.push_back(ControlCmd {
        task_id: id.clone(),
        pid,
        action: am_core::model::ControlAction::Input,
        text: Some(text),
        id: Some(cmd_id.clone()),
    });
    drop(machines); // 释放锁：下面后台任务会再读 machines
    // 记进「远程交互历史」的 user 侧。网页这条路径没走 bot::queue_command（它自己压队列），
    // 所以要单独记一次，否则网页发的任务不会出现在聊天记录里。
    // 选择卡的作答除外（见 InputReq::from_select）。
    if !req.from_select {
        let slot =
            crate::slots::slot_of(&state, &user, &crate::slots::anchor_of(&task)).await;
        crate::history::append(
            &state,
            crate::history::HistoryEntry {
                id: crate::history::new_id(),
                owner: user.clone(),
                session_id: id.clone(),
                role: "user".into(),
                content: text_for_notify.clone(),
                at: crate::state::now_secs(),
                source: "web".into(),
                slot,
                hostname: task.hostname.clone(),
                project: task.project_name.clone(),
                title: task.title.clone(),
                provider: task.provider_dsr.clone(),
            },
        )
        .await;
    }
    // 网页/客户端（非钉钉）下发的任务，主动把「排队中 / 执行中」状态推到钉钉私聊；
    // 排队的还会盯到执行后再推一条。钉钉自己「发 N」走 queue_command 不经这里，不重复。
    // 选择卡的作答不推：钉钉那边本来就收到过「⌨️ 需要你选择」，再补一条「已下发 1」
    // 只是噪音 —— 真正该看的是它选完之后做了什么。
    if !req.from_select {
        let st = state.clone();
        let (owner, tid) = (user.clone(), id.clone());
        tokio::spawn(async move {
            crate::bot::notify_web_dispatch(st, owner, tid, text_for_notify).await
        });
    }
    ok(json!({ "pid": pid, "result": "已下发到目标机器", "cmdId": cmd_id }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TermKeyReq {
    /// "up"（撤回排队，按 count 次上键）| "esc"（插入排队，按一次 Esc）
    key: String,
    #[serde(default)]
    count: u32,
    #[serde(default)]
    pid: Option<u32>,
}

/// POST /monitor/tasks/:id/termkey —— 向终端注入按键：撤回排队(↑) / 插入排队(Esc)。
/// 仅 iTerm2(mac) 与 Windows 控制台可干净注入；Terminal.app 由前端走提示、不会走到这里。
async fn termkey_task(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TermKeyReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let spec = match req.key.as_str() {
        "up" => format!("up:{}", req.count.clamp(1, 50)),
        "esc" => "esc".to_string(),
        _ => return err(400, "未知按键"),
    };
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    let pid = match (req.pid, task.pid) {
        (Some(p), Some(tp)) if p != tp => return err(403, "pid 与任务不符"),
        (Some(_), None) => return err(403, "该任务没有关联进程"),
        _ => task.pid,
    };
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
        action: am_core::model::ControlAction::TermKey,
        text: Some(spec),
        id: None,
    });
    ok(json!({ "result": "已下发按键" }))
}

/// GET /monitor/tasks/:id/queued —— 该会话仍在 hub 队列里、还没被客户端
/// 取走的输入（网页据此显示「排队中」并提供撤回）。
async fn queued_inputs(
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
    let machines = state.machines.read().await;
    let list: Vec<Value> = machines
        .get(&task.machine_id)
        .map(|entry| {
            entry
                .pending
                .iter()
                .filter(|c| {
                    c.task_id == id
                        && matches!(c.action, am_core::model::ControlAction::Input)
                        && c.id.is_some()
                })
                .map(|c| json!({ "cmdId": c.id, "text": c.text }))
                .collect()
        })
        .unwrap_or_default();
    ok(json!({ "list": list }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecallReq {
    cmd_id: String,
}

/// POST /monitor/tasks/:id/recall —— 撤回仍在排队的输入。
/// 只在 hub 队列里有效；已被客户端取走（写进终端）则撤不回。
async fn recall_input(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RecallReq>,
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
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return err(404, "任务所属机器已离线");
    };
    let before = entry.pending.len();
    entry
        .pending
        .retain(|c| c.id.as_deref() != Some(req.cmd_id.as_str()));
    if entry.pending.len() < before {
        ok(json!(true))
    } else {
        err(410, "已被终端接收，无法撤回")
    }
}

#[derive(Deserialize)]
struct DirsQuery {
    #[serde(default)]
    rel: String,
}

/// GET /monitor/tasks/:id/dirs?rel=a/b —— 会话目录下的子目录（异步：
/// 首次返回 pending，agent 下一轮上报带回结果后再查即有缓存）。
async fn task_dirs(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DirsQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // rel 归一化：拒绝越出根的路径（.. 与绝对路径）
    let rel = q.rel.trim().trim_matches('/').to_string();
    if rel.split('/').any(|seg| seg == "..") || rel.starts_with('/') {
        return err(400, "非法目录");
    }
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    let cwd = task.process.as_ref().map(|p| p.cwd.clone()).unwrap_or_default();
    if cwd.is_empty() {
        return err(400, "该会话没有工作目录信息");
    }
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return err(404, "任务所属机器已离线");
    };
    if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
        return err(500, "任务所属机器已离线");
    }
    let key = (id.clone(), rel.clone());
    let cached = entry.dir_cache.get(&key).cloned();
    if cached.is_none()
        && !entry
            .pending_dir
            .iter()
            .any(|x| x.task_id == id && x.rel == rel)
    {
        entry.pending_dir.push_back(am_core::model::DirQuery {
            task_id: id.clone(),
            cwd: cwd.clone(),
            rel: rel.clone(),
        });
    }
    match cached {
        Some((dirs, files)) => {
            ok(json!({ "dirs": dirs, "files": files, "cwd": cwd, "pending": false }))
        }
        None => ok(json!({ "dirs": [], "files": [], "cwd": cwd, "pending": true })),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsOpReq {
    /// mkdir / delete / rename
    op: String,
    #[serde(default)]
    rel: String,
    name: String,
    #[serde(default)]
    new_name: String,
}

/// 单个路径段合法性：非空、无分隔符、非 . / ..
fn valid_seg(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && !s.contains('\\') && s != "." && s != ".."
}

/// POST /monitor/tasks/:id/fsop —— 会话目录内新建/删除/重命名文件夹（异步：下发给
/// agent 执行，返回 opId，网页再轮询 /fsop/:opid 取结果）。
async fn task_fsop(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<FsOpReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let rel = req.rel.trim().trim_matches('/').to_string();
    if rel.split('/').filter(|s| !s.is_empty()).any(|seg| seg == "..") || rel.starts_with('/') {
        return err(400, "非法目录");
    }
    if !matches!(req.op.as_str(), "mkdir" | "delete" | "rename") {
        return err(400, "未知操作");
    }
    if !valid_seg(&req.name) {
        return err(400, "非法名称");
    }
    if req.op == "rename" && !valid_seg(&req.new_name) {
        return err(400, "非法新名称");
    }
    let task = {
        let tasks = state.tasks_for(&user).await;
        tasks.into_iter().find(|t| t.id == id)
    };
    let Some(task) = task else {
        return err(404, "任务不存在");
    };
    let cwd = task.process.as_ref().map(|p| p.cwd.clone()).unwrap_or_default();
    if cwd.is_empty() {
        return err(400, "该会话没有工作目录信息");
    }
    let op_id = format!("{}-{}", task.machine_id, crate::state::now_secs());
    let op_id = format!("{op_id}-{}", rel.len() + req.name.len() + req.op.len());
    let mut machines = state.machines.write().await;
    let Some(entry) = machines.get_mut(&task.machine_id) else {
        return err(404, "任务所属机器已离线");
    };
    if entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS {
        return err(500, "任务所属机器已离线");
    }
    entry.pending_fsop.push_back(am_core::model::FsOp {
        op_id: op_id.clone(),
        task_id: id.clone(),
        cwd,
        rel: rel.clone(),
        op: req.op,
        name: req.name,
        new_name: req.new_name,
    });
    // 该目录的列举缓存作废：操作后网页会重新拉取，须重新向 agent 查询而非返回旧缓存
    entry.dir_cache.remove(&(id, rel));
    ok(json!({ "opId": op_id, "pending": true }))
}

/// GET /monitor/tasks/:id/fsop/:opid —— 取某文件夹操作的执行结果（agent 回传前 pending）
async fn task_fsop_result(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_id, opid)): Path<(String, String)>,
) -> Json<Value> {
    if auth_user(&state, &headers).await.is_none() {
        return err(401, "未登录");
    }
    let mut machines = state.machines.write().await;
    for entry in machines.values_mut() {
        if let Some(r) = entry.fsop_results.remove(&opid) {
            return ok(json!({ "ok": r.ok, "msg": r.msg, "pending": false }));
        }
    }
    ok(json!({ "pending": true }))
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

/// GET /monitor/config/sync —— 配置同步状态：谁是配置源、各设备还差多少份。
///
/// 「差多少」对源机与镜像机含义相反：源机是「基线还没收全的份数」，
/// 镜像机是「本机还缺的份数」，前端按 isSource 分别措辞。
async fn config_sync_status(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let (source, devices) = {
        let reg = state.registry.read().await;
        (reg.config_source_of(&user), reg.devices_of(&user))
    };
    let store = state.configs.read().await;
    let baseline = store.manifest_of(&user);
    let machines = state.machines.read().await;

    let list: Vec<Value> = devices
        .iter()
        .map(|(id, meta)| {
            let entry = machines.get(id);
            let online = entry
                .map(|e| e.last_report.elapsed().as_secs() < crate::state::OFFLINE_AFTER_SECS)
                .unwrap_or(false);
            let is_source = source.as_deref() == Some(id.as_str());
            // 没有清单 = 客户端版本还不支持配置同步，或刚上线还没扫完第一轮
            let (supported, file_count, behind, scanned_at) = match entry
                .and_then(|e| e.config_manifest.as_ref())
            {
                Some(m) => {
                    let behind = if is_source {
                        crate::configsync::diff(m, &baseline).len()
                    } else {
                        crate::configsync::diff(&baseline, m).len()
                    };
                    (true, m.files.len(), behind, m.scanned_at)
                }
                None => (false, 0usize, 0usize, 0u64),
            };
            json!({
                "machineId": id,
                "hostname": meta.hostname,
                "platform": am_core::model::platform_dsr(&meta.platform),
                "trusted": meta.trusted,
                "online": online,
                "isSource": is_source,
                "supported": supported,
                "fileCount": file_count,
                "behind": behind,
                "scannedAt": scanned_at,
            })
        })
        .collect();

    // 字段普查汇总（二期定白名单用）：按 文件+字段路径 归并所有设备的结果。
    // `machineSpecific` 取**或**：只要在任何一台机器上是本机路径，这个字段就不能跨机同步。
    let mut agg: std::collections::BTreeMap<(String, String), (String, bool, usize)> =
        std::collections::BTreeMap::new();
    for (_machine, probes) in store.probes_of(&user) {
        for p in probes {
            for k in p.keys {
                let e = agg
                    .entry((p.file.clone(), k.path.clone()))
                    .or_insert((k.ty.clone(), false, 0));
                e.1 |= k.machine_specific;
                e.2 += 1;
            }
        }
    }
    let probe: Vec<Value> = agg
        .into_iter()
        .map(|((file, path), (ty, machine_specific, seen))| {
            json!({
                "file": file,
                "path": path,
                "type": ty,
                "machineSpecific": machine_specific,
                // 出现在几台设备上：只在一台上出现的字段多半是那台机器的特例
                "seenOn": seen,
            })
        })
        .collect();

    ok(json!({
        "enabled": source.is_some(),
        "source": source,
        "baselineCount": store.file_count(&user),
        "devices": list,
        "probe": probe,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSourceReq {
    /// 作为配置源的设备；空字符串 = 关闭该账号的配置同步
    #[serde(default)]
    machine_id: String,
}

/// POST /monitor/config/source —— 指定配置源设备（空 = 关闭同步）
async fn set_config_source(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ConfigSourceReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // 归属校验在 registry 里做（填别人的 machine_id 就能把对方配置拉进自己的基线）
    match state.registry.write().await.set_config_source(&user, &req.machine_id) {
        Ok(()) if req.machine_id.trim().is_empty() => ok(json!({ "result": "已关闭配置同步" })),
        Ok(()) => ok(json!({ "result": "已设为配置源" })),
        Err(e) => err(400, &e),
    }
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

/// hub 对外公网地址（拼绑定链接 / 回调 URL 用），取自 AM_PUBLIC_URL，默认线上域名
pub(crate) fn public_base() -> String {
    std::env::var("AM_PUBLIC_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://monitor.vita-llm.com".into())
        .trim_end_matches('/')
        .to_string()
}

/// GET /monitor/integrations —— 当前用户的三种渠道配置 + 专属回调地址。
/// 密钥类字段不回传，只回「是否已设置」。
async fn integrations_get(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // 机器人文件接收目录（所有渠道通用，按项目存）：按「设备 → 项目」层级列出，供弹窗配置。
    // 每个项目附一个活跃会话 taskId，网页据此调 /dirs 浏览该项目目录树来选接收目录。
    let recv_dirs = state.registry.read().await.dingtalk_recv_dirs_of(&user);
    // machine_id → (hostname, 项目 key → (代表 cwd, name, taskId))
    // 分组键用 encode_path（项目 key），与侧栏 selectedGroups / 配对 project_key 同规则：
    // 同一目录的不同 cwd 形态（占位任务用进程 cwd vs 真实会话用 jsonl cwd、cursor/非
    // cursor，分隔符/盘符/标点常有细微差异）归并为一项，避免像 sub-centers 那样冒重复行。
    let mut devs: std::collections::BTreeMap<
        String,
        (String, std::collections::BTreeMap<String, (String, String, Option<String>)>),
    > = std::collections::BTreeMap::new();
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in state.tasks_for(&user).await {
        if t.project.is_empty() {
            continue;
        }
        // 与侧栏一致：只列活跃会话的项目，隐藏已结束会话（否则旧会话的项目会多冒出来、
        // 跟左侧列表对不上）。list_tasks 也是这个过滤。
        if t.status == TaskStatus::Finished {
            continue;
        }
        let key = am_core::scanner::encode_path(&t.project);
        seen_keys.insert(key.clone());
        let d = devs
            .entry(t.machine_id.clone())
            .or_insert_with(|| (t.hostname.clone(), std::collections::BTreeMap::new()));
        let p = d
            .1
            .entry(key)
            .or_insert_with(|| (t.project.clone(), t.project_name.clone(), None));
        // 代表 cwd/taskId 优先取带活跃会话 id 的那条（网页据此浏览目录树）
        if p.2.is_none() && !t.id.is_empty() {
            p.0 = t.project.clone();
            p.2 = Some(t.id.clone());
        }
    }
    // 已配置但当前无活跃会话的项目：归到「未在线」分组（无 taskId → 只能手输，不能浏览）
    for k in recv_dirs.keys() {
        let key = am_core::scanner::encode_path(k);
        if !seen_keys.contains(&key) {
            let name =
                k.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(k).to_string();
            devs.entry(String::new())
                .or_insert_with(|| ("（未在线项目）".to_string(), std::collections::BTreeMap::new()))
                .1
                .insert(key, (k.clone(), name, None));
        }
    }
    let recv_dir_devices: Vec<Value> = devs
        .into_iter()
        .map(|(machine_id, (hostname, projs))| {
            json!({
                "machineId": machine_id,
                "hostname": hostname,
                "projects": projs.into_iter().map(|(key, (cwd, name, task_id))| json!({
                    "cwd": cwd,
                    "name": name,
                    // 配置按 encode_path 匹配（存储键可能是同目录的另一种 cwd 形态）
                    "dir": recv_dirs.iter()
                        .find(|(k, _)| am_core::scanner::encode_path(k) == key)
                        .map(|(_, v)| v.clone()),
                    "taskId": task_id,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    // 两种接入方式的现状都要回：用户据此知道自己走的是哪条路
    let reg = state.registry.read().await;
    let is_super = reg.is_global_dingtalk_app(&user);
    let app = reg.dingtalk_app_of(&user);
    // 管理员配了全局机器人 → 没自己配机器人的用户也能用（绑钉钉号即可）
    let has_global = reg.global_dingtalk_app().is_some_and(|a| !a.app_key.is_empty());
    let bound: Vec<Value> = reg
        .dingtalk_ids_of(&user)
        .into_iter()
        .map(|(staff_id, nick)| json!({ "staffId": staff_id, "nick": nick }))
        .collect();
    drop(reg);
    ok(json!({
        "recvDirDevices": recv_dir_devices,
        // 自己的机器人（优先生效）。密钥不回显，只回「配没配」+ 通没通。
        // 超管名下那个是全局机器人，不算他的「个人机器人」，免得界面上两处打架。
        "dingtalk": {
            "appKey": if is_super { String::new() } else { app.as_ref().map(|a| a.app_key.clone()).unwrap_or_default() },
            "hasSecret": !is_super && app.as_ref().is_some_and(|a| !a.app_secret.is_empty()),
            // 已捕获到聊天对象 = 机器人已经能主动给你推消息了
            "linked": !is_super && app.as_ref().is_some_and(|a| !a.staff_id.is_empty()),
        },
        // 没配自己的机器人时可用的公共通道：绑定钉钉号即可
        "globalBot": { "available": has_global, "boundIds": bound },
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DingtalkAppReq {
    #[serde(default)]
    app_key: String,
    #[serde(default)]
    app_secret: String,
}

/// POST /monitor/integrations/dingtalk-app —— 配置**自己的**钉钉机器人。
///
/// 一个账号一个机器人：配好之后，这个机器人收到的消息都归本账号，推送也只发给
/// 跟它说过话的那个钉钉号。appKey 留空 = 解绑（连同已捕获的聊天对象一并清掉）。
async fn set_dingtalk_app(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<DingtalkAppReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let app_key = req.app_key.trim().to_string();
    let mut app_secret = req.app_secret.trim().to_string();
    if app_key.is_empty() {
        state.registry.write().await.set_dingtalk_app(&user, "", "");
        state.dingtalk_reload.notify_one();
        return ok(json!({ "result": "已解绑" }));
    }
    // 密钥留空 = 沿用已存的（界面上不回显密钥，只改 appKey 时不该被清掉）
    if app_secret.is_empty() {
        app_secret = state
            .registry
            .read()
            .await
            .dingtalk_app_of(&user)
            .map(|a| a.app_secret)
            .unwrap_or_default();
    }
    if app_secret.is_empty() {
        return err(400, "请填写 AppSecret");
    }
    state.registry.write().await.set_dingtalk_app(&user, &app_secret, &app_key);
    // 立刻重连 Stream：不重连的话要等下次 hub 重启才生效
    state.dingtalk_reload.notify_one();
    ok(json!({ "result": "已保存，去钉钉给机器人发条消息即可开始使用" }))
}

/// POST /monitor/integrations/dingtalk-bindcode —— 取一个绑定码。
///
/// 拿到码后到钉钉里发「绑定 <码>」即可把那个钉钉号绑到本账号。网页会把
/// 这条指令做成二维码，手机扫一下就能直接发出去。
async fn dingtalk_bind_code(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let now = crate::state::now_secs();
    let mut map = state.dingtalk_bind_codes.write().await;
    map.retain(|_, e| now.saturating_sub(e.at) < crate::bot::BIND_TOKEN_TTL_SECS);
    // 同一个人反复点「取码」不该堆一串等价的码：还没过期就复用手上那个
    if let Some((code, e)) = map.iter().find(|(_, e)| e.user == user) {
        let left = crate::bot::BIND_TOKEN_TTL_SECS.saturating_sub(now.saturating_sub(e.at));
        let code = code.clone();
        return ok(json!({ "code": code, "expiresIn": left, "command": format!("绑定 {code}") }));
    }
    if map.len() >= 5000 {
        return err(429, "绑定请求过多，请稍后再试");
    }
    let code = crate::state::new_bind_code();
    map.insert(code.clone(), crate::state::PendingBindCode { user, at: now });
    ok(json!({
        "code": code,
        "expiresIn": crate::bot::BIND_TOKEN_TTL_SECS,
        // 前端直接把它做成二维码：扫出来就是能发给机器人的那句话
        "command": format!("绑定 {code}"),
    }))
}

/// GET /monitor/integrations/dingtalk-qr —— 扫码绑定用的授权地址。
///
/// 前端把返回的 url 画成二维码，用钉钉扫一下、确认授权，回调就把扫码那个人的
/// 钉钉号绑到本账号 —— 全程不用手输任何东西。
///
/// **只对全局机器人有意义**：自己配了机器人的账号，谁配的归谁，本就不需要绑
/// 钉钉号。而且 OAuth 的 redirect_uri 要在应用后台登记白名单，个人应用没登记
/// 过我们的回调地址，给了也走不通。
///
/// state 直接复用绑定码：一码两用 —— 扫码走它认人，手动发「绑定 <码>」也认它。
async fn dingtalk_qr(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let Some(app) = state.registry.read().await.global_dingtalk_app() else {
        return err(400, "管理员还没配公共机器人，暂不能扫码绑定");
    };
    if app.app_key.is_empty() {
        return err(400, "管理员还没配公共机器人，暂不能扫码绑定");
    }
    let now = crate::state::now_secs();
    let mut map = state.dingtalk_bind_codes.write().await;
    map.retain(|_, e| now.saturating_sub(e.at) < crate::bot::BIND_TOKEN_TTL_SECS);
    // 与取码走同一张表：手上那个码没过期就接着用，别每次刷新都换二维码
    let (code, left) = match map.iter().find(|(_, e)| e.user == user) {
        Some((c, e)) => {
            (c.clone(), crate::bot::BIND_TOKEN_TTL_SECS.saturating_sub(now.saturating_sub(e.at)))
        }
        None => {
            if map.len() >= 5000 {
                return err(429, "绑定请求过多，请稍后再试");
            }
            let c = crate::state::new_bind_code();
            map.insert(c.clone(), crate::state::PendingBindCode { user, at: now });
            (c, crate::bot::BIND_TOKEN_TTL_SECS)
        }
    };
    drop(map);
    let redirect = format!("{}/monitor/integrations/dingtalk-scan", public_base());
    ok(json!({
        "url": crate::dingtalk::qr_auth_url(&app.app_key, &redirect, &code),
        "code": code,
        "expiresIn": left,
        // 扫码不通时的退路：到钉钉里把这句话发给机器人，一样能绑
        "command": format!("绑定 {code}"),
    }))
}

#[derive(Deserialize)]
struct ScanCb {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
}

/// GET /monitor/integrations/dingtalk-scan —— 扫码授权后的回调（钉钉打开，无登录态）。
///
/// 回的是给人看的 HTML：扫码人此刻在手机钉钉的内置浏览器里，看不懂 JSON。
async fn dingtalk_scan_cb(
    State(state): State<SharedState>,
    Query(q): Query<ScanCb>,
) -> axum::response::Html<String> {
    let page = |ok: bool, msg: &str| {
        axum::response::Html(format!(
            "<!doctype html><meta charset=utf-8>\
             <meta name=viewport content=\"width=device-width,initial-scale=1\">\
             <div style=\"font:16px/1.7 -apple-system,system-ui,sans-serif;\
             padding:56px 24px;text-align:center;color:#1f2329\">\
             <div style=\"font-size:44px\">{}</div>\
             <div style=\"margin-top:16px;font-size:18px;font-weight:600\">{}</div>\
             <div style=\"margin-top:10px;color:#8a8f8d;font-size:14px\">{}</div></div>",
            if ok { "✅" } else { "⚠️" },
            msg,
            if ok { "可以关掉这个页面了" } else { "请回到网页重新扫码" },
        ))
    };
    if q.code.is_empty() || q.state.is_empty() {
        return page(false, "授权信息不完整");
    }
    // state = 绑定码 → 认出「是谁在网页上发起的这次绑定」。一次性，用掉即销。
    let now = crate::state::now_secs();
    let pending = state.dingtalk_bind_codes.write().await.remove(q.state.trim());
    let Some(p) = pending else {
        return page(false, "二维码已失效");
    };
    if now.saturating_sub(p.at) >= crate::bot::BIND_TOKEN_TTL_SECS {
        return page(false, "二维码已过期");
    }
    let Some(app) = state.registry.read().await.global_dingtalk_app() else {
        return page(false, "公共机器人未配置");
    };
    let now_ms = now * 1000;
    match crate::dingtalk::resolve_scan_user(&app.app_key, &app.app_secret, &q.code, now_ms).await {
        Ok((staff_id, nick)) => {
            state.registry.write().await.bind_dingtalk_id(&staff_id, &p.user, &nick);
            tracing::info!("钉钉扫码绑定成功 user={} nick={nick}", p.user);
            page(true, &format!("已绑定到 {}", p.user))
        }
        Err(e) => {
            tracing::warn!("钉钉扫码绑定失败: {e}");
            page(false, "绑定失败，请重试")
        }
    }
}

#[derive(Deserialize)]
struct DingtalkBindReq {
    token: String,
}

/// POST /monitor/integrations/dingtalk-bind —— 认领机器人回的登录链接（?dtbind=）。
///
/// 这是绑定的另一头：人在手机上、先跟机器人说了话，机器人回一条链接，
/// 点开登录后带 token 来认领，把那个钉钉号绑到刚登录进的账号。
async fn dingtalk_bind(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<DingtalkBindReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let now = crate::state::now_secs();
    // 一次性：取出即作废，避免链接被转发后重复绑定
    let pending = state.dingtalk_binds.write().await.remove(req.token.trim());
    let Some(p) = pending else {
        return err(400, "绑定链接无效或已过期，请在钉钉里重新给机器人发条消息");
    };
    if now.saturating_sub(p.at) >= crate::bot::BIND_TOKEN_TTL_SECS {
        return err(400, "绑定链接已过期，请在钉钉里重新给机器人发条消息");
    }
    state.registry.write().await.bind_dingtalk_id(&p.staff_id, &user, &p.nick);
    ok(json!({ "result": "已绑定", "staffId": p.staff_id, "nick": p.nick }))
}

/// GET /monitor/integrations/dingtalk-ids —— 本账号已绑定的钉钉号
async fn dingtalk_ids_get(State(state): State<SharedState>, headers: HeaderMap) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let list: Vec<Value> = state
        .registry
        .read()
        .await
        .dingtalk_ids_of(&user)
        .into_iter()
        .map(|(staff_id, nick)| json!({ "staffId": staff_id, "nick": nick }))
        .collect();
    ok(json!({ "list": list }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnbindReq {
    staff_id: String,
}

/// POST /monitor/integrations/dingtalk-unbind —— 解绑自己的某个钉钉号
async fn dingtalk_unbind(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<UnbindReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // 只能解绑自己的：否则拿到别人的 staffId 就能把人踢下线
    if state.registry.read().await.dingtalk_user_of(&req.staff_id).as_deref() != Some(user.as_str())
    {
        return err(403, "该钉钉号不在你名下");
    }
    state.registry.write().await.unbind_dingtalk_id(&req.staff_id);
    ok(json!({ "result": "已解绑" }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecvDirReq {
    /// 项目 cwd（唯一键）
    project: String,
    /// 接收目录：绝对路径或相对项目的子路径；空 = 清除（回落默认 tmp）
    #[serde(default)]
    dir: String,
}

/// POST /monitor/integrations/dingtalk-recv-dir —— 设置某项目的钉钉文件接收目录
async fn set_dingtalk_recv_dir(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<RecvDirReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    if req.project.trim().is_empty() {
        return err(400, "缺少项目");
    }
    state
        .registry
        .write()
        .await
        .set_dingtalk_recv_dir(&user, req.project.trim(), &req.dir);
    ok(json!({ "result": "已保存" }))
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    /// 返回条数上限（默认 50，最多 200）
    #[serde(default)]
    limit: Option<usize>,
    /// 只看某个会话的往来；不给则返回该账号的全部
    #[serde(default)]
    session: Option<String>,
}

/// GET /monitor/history —— 本账号的会话历史（最新在前）。
/// 每个会话结束时留一条最终产出，终端关了、机器关机后仍可回看。
async fn list_history(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let session = q.session.as_deref().filter(|s| !s.trim().is_empty());
    let list = crate::history::list_for(&state, &user, session, limit).await;
    ok(json!({ "list": list, "total": list.len() }))
}

// 钉钉机器人由用户在前台「机器人管理」自助配置（见 set_dingtalk_app）：
// 谁配的机器人就服务谁，不再需要单独把钉钉 id 绑到账号上。
// 钉钉群机器人 / 企业微信 / 企业应用配置均已迁到后管（admin::*，/sys/dingtalk/* · /sys/wecom/*），此处不再有用户端入口。

// ---------- 协助共享 ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareCreateReq {
    /// true=临时密码（系统生成，30 分钟过期）；false=固定密码
    temporary: bool,
    #[serde(default)]
    password: String,
}

/// GET /monitor/share/:id —— 查看本设备当前协助码（主人）
async fn share_info(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    match state.registry.read().await.share_info(&id) {
        Some((code, temporary, expires_at)) => {
            ok(json!({ "code": code, "temporary": temporary, "expiresAt": expires_at }))
        }
        None => ok(json!(null)),
    }
}

/// POST /monitor/share/:id —— 生成/刷新协助码（主人）。返回连接码 + 明文密码。
async fn share_create(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ShareCreateReq>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    let fixed = (!req.temporary).then_some(req.password.as_str());
    // 先 clone 出结果再释放写锁：写锁临时量若活到 match 结束，Ok 分支里
    // 再取读锁 share_info 会自我死锁（同 if-let 锁跨块陷阱）。
    let created = state.registry.write().await.create_share(&id, req.temporary, fixed);
    match created {
        Ok((code, password)) => {
            let expires_at = state.registry.read().await.share_info(&id).map(|(_, _, e)| e).unwrap_or(0);
            ok(json!({
                "code": code,
                "password": password,
                "temporary": req.temporary,
                "expiresAt": expires_at,
            }))
        }
        Err(e) => err(400, &e),
    }
}

/// DELETE /monitor/share/:id —— 撤销协助码 + 踢出所有访客（主人）
async fn share_revoke(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    state.registry.write().await.revoke_share(&id);
    ok(json!({ "result": "已停止共享" }))
}

/// GET /monitor/share/:id/guests —— 当前接入的访客（主人）
async fn share_guests(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    let list = state.registry.read().await.share_guests(&id);
    ok(json!({ "list": list }))
}

#[derive(Deserialize)]
struct KickReq {
    user: String,
}

/// POST /monitor/share/:id/kick —— 踢掉某访客（主人）
async fn share_kick(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<KickReq>,
) -> Json<Value> {
    if let Err(e) = ensure_owner(&state, &headers, &id).await {
        return e;
    }
    state.registry.write().await.kick_share_user(&id, &req.user);
    ok(json!({ "result": "已移除" }))
}

#[derive(Deserialize)]
struct ShareConnectReq {
    code: String,
    password: String,
}

/// POST /monitor/share/connect —— 访客用连接码 + 密码接入他人设备
async fn share_connect(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ShareConnectReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    // 密码爆破节流（复用登录节流器，按连接码计账）
    let delay = state.login_throttle.read().await.delay_for(&req.code);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    // 先释放注册表写锁再动 login_throttle，避免跨锁持有
    let res = state.registry.write().await.connect_share(&req.code, &req.password, &user);
    match res {
        Ok(machine_id) => {
            state.login_throttle.write().await.record_success(&req.code);
            tracing::info!("用户 {user} 通过协助码接入设备 {machine_id}");
            ok(json!({ "machineId": machine_id, "result": "接入成功" }))
        }
        Err(e) => {
            state.login_throttle.write().await.record_fail(&req.code);
            err(400, &e)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareDisconnectReq {
    machine_id: String,
}

/// POST /monitor/share/disconnect —— 访客主动断开自己的接入
async fn share_disconnect(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ShareDisconnectReq>,
) -> Json<Value> {
    let Some(user) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    state.registry.write().await.disconnect_share(&req.machine_id, &user);
    ok(json!({ "result": "已断开" }))
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
    let mut name_field = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    // 分片信息：前端切大文件时带上，缺省即「整份就这一个」
    let mut chunk_index: u32 = 0;
    let mut chunk_total: u32 = 0;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "dir" => dir = field.text().await.unwrap_or_default(),
            // 显式文件名（UTF-8 文本字段）：优先用它，避免 multipart filename 对非 ASCII 解歪
            "name" => name_field = field.text().await.unwrap_or_default(),
            "chunkIndex" => {
                chunk_index = field.text().await.ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            }
            "chunkTotal" => {
                chunk_total = field.text().await.ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            }
            "file" => {
                filename = field.file_name().unwrap_or("file.bin").to_string();
                bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            }
            _ => {}
        }
    }
    // 优先显式 name 字段；缺省（旧前端）退回 multipart filename
    if !name_field.trim().is_empty() {
        filename = name_field.trim().to_string();
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
        // 分片必须先确认对端认得这套协议。旧版 agent 反序列化时 chunk_total 取默认值 0，
        // 会把**每一片**都当成完整文件覆盖写 —— 传完只剩最后一片，文件却看着"成功"了。
        // 与其静默写坏，不如明确拒绝并告诉用户去更新客户端。
        if chunk_total > 1 && !agent_supports_chunked(&entry.version) {
            return err(
                400,
                "该设备的客户端版本过旧，不支持分片传输大文件，请先更新客户端",
            );
        }
        entry.pending_files.push_back(am_core::model::FileTransfer {
            dir,
            filename: safe_name,
            content_b64: B64.encode(&bytes),
            chunk_index,
            chunk_total,
        });
        let done = chunk_total <= 1 || chunk_index + 1 >= chunk_total;
        ok(json!({
            "result": if done { "已下发到目标设备，等待写入" } else { "分片已接收" },
            "size": bytes.len(),
            "chunkIndex": chunk_index,
            "chunkTotal": chunk_total,
        }))
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

/// 配置同步的一轮：算出「要向这台机器索要哪些文件」与「要下发哪些文件给它」。
///
/// 差异每轮现算，**不进 `pending_*` 队列**：队列会在设备离线期间积压出一堆早已过期的内容，
/// 上线后一股脑写下去；而配置差异是幂等的，重算一次比维护队列正确得多，也天然容错——
/// 任何一轮丢了，下一轮照样算得出来。
///
/// 返回 `(要索要的路径, 要下发的内容)`，两者互斥：一台机器要么是配置源（只上传），
/// 要么是镜像（只下载）。
async fn sync_configs(
    state: &SharedState,
    machine_id: &str,
    bodies: &[am_core::model::ConfigFileBody],
    probes: &[am_core::model::ConfigProbe],
    device_manifest: Option<am_core::model::ConfigManifest>,
) -> (Vec<String>, Vec<am_core::model::ConfigPush>) {
    let empty = || (Vec::new(), Vec::new());

    // 归属账号 + 该账号选定的配置源。未信任的设备一概不参与：
    // 它连会话都不许上报，更不该往别人的机器上写文件。
    let (owner, source) = {
        let reg = state.registry.read().await;
        let meta = reg.device_meta(machine_id);
        if !meta.trusted {
            return empty();
        }
        let Some(owner) = meta.owner else { return empty() };
        let source = reg.config_source_of(&owner);
        (owner, source)
    };
    // 没指定配置源 = 该账号没开配置同步。默认关闭：往用户机器上写文件这件事，
    // 必须是他自己点开的。
    let Some(source) = source else { return empty() };
    let is_source = source == machine_id;

    // 字段普查（只有键与类型，没有值）：源机与镜像机都收 —— 同一字段在不同机器上
    // 是否机器相关可能不同，而那正是「它能不能跨机同步」的判据。
    if !probes.is_empty() {
        state.configs.read().await.put_probe(&owner, machine_id, probes);
    }

    // 源机回传的内容入基线。只认源机的上传——否则任何一台被控设备都能往基线里塞东西，
    // 而基线随后会被分发到该账号的全部设备上。
    if is_source && !bodies.is_empty() {
        let mut store = state.configs.write().await;
        for b in bodies {
            store.put(&owner, b);
        }
    }

    // 还没收到过这台机器的清单（旧客户端，或刚上线还没扫完）：这一轮没有可比对的东西
    let Some(device) = device_manifest else { return empty() };

    if is_source {
        // 源机：基线要向它看齐。先摘掉源机已经删掉的条目，否则用户在源机删了一个 agent，
        // 基线还留着，反手又会把它推回给其它机器。
        //
        // 空清单不 prune：客户端扫不到目录（权限、home 取不到）时也会报空，
        // 那不是「用户删光了配置」，照单执行会清空整个基线。
        if !device.files.is_empty() {
            let keep: std::collections::HashSet<&str> =
                device.files.iter().map(|f| f.path.as_str()).collect();
            let dropped = state.configs.write().await.retain(&owner, &keep);
            if dropped > 0 {
                tracing::info!("配置基线移除 {dropped} 份（源机已删除）: {owner}");
            }
        }
        let baseline = state.configs.read().await.manifest_of(&owner);
        let mut pulls = crate::configsync::diff(&device, &baseline);
        pulls.truncate(crate::configsync::MAX_PULLS_PER_ROUND);
        (pulls, Vec::new())
    } else {
        // 镜像机：基线里有而它没有（或内容不同）的，发给它
        let store = state.configs.read().await;
        let want = crate::configsync::diff(&store.manifest_of(&owner), &device);
        let pushes: Vec<_> = want
            .iter()
            .take(crate::configsync::MAX_PUSHES_PER_ROUND)
            .filter_map(|rel| store.get(&owner, rel))
            .collect();
        (Vec::new(), pushes)
    }
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

    // 钉钉推送：本次上报的归属者（用于状态变化推送）
    let notify_owner = state.registry.read().await.device_meta(&payload.machine_id).owner;

    let mut machines = state.machines.write().await;
    let mut was_new = false;
    let entry = machines
        .entry(payload.machine_id.clone())
        .or_insert_with(|| {
            was_new = true;
            MachineEntry {
                hostname: payload.hostname.clone(),
                platform: payload.platform.clone(),
                version: payload.version.clone(),
                is_hub: false,
                tasks: Vec::new(),
                last_report: Instant::now(),
                pending: VecDeque::new(),
                pending_files: VecDeque::new(),
                messages: HashMap::new(),
                pending_dir: VecDeque::new(),
                pending_fsop: VecDeque::new(),
                fsop_results: HashMap::new(),
                dir_cache: HashMap::new(),
                notified_online: false,
                select_notified: std::collections::HashSet::new(),
                online_since: Instant::now(),
                known_sessions: HashMap::new(),
                session_last_seen: HashMap::new(),
                last_select_at: HashMap::new(),
                new_session_pending: HashMap::new(),
                config_manifest: None,
            }
        });
    // 设备上线边沿：新登记 或 之前已判离线（超阈值）
    let was_offline = was_new || entry.last_report.elapsed().as_secs() >= OFFLINE_AFTER_SECS;
    entry.hostname = payload.hostname.clone();
    entry.platform = payload.platform;
    entry.version = payload.version;
    entry.last_report = Instant::now();
    // 配置清单：客户端每 30s 才带一次，其余轮次是 None —— 所以只覆盖、不清空，
    // 中间轮次的差异计算全靠这份缓存才能每轮推进（见 configsync）。
    if let Some(m) = &payload.config_manifest {
        entry.config_manifest = Some(m.clone());
    }
    let device_manifest = entry.config_manifest.clone();
    // 上线边沿：刷新沉降起点。上线后 NEW_SESSION_SETTLE_SECS 内出现的会话一律当「重连扫回的
    // 已有会话」不推，避免客户端更新/重启后分批扫回历史会话时刷屏「会话开始」。
    // 同时清空会话基线：离线期间「消失」的旧会话不该在重连时逐条推「已结束」，重连后重建基线。
    if was_offline {
        entry.online_since = Instant::now();
        entry.known_sessions.clear();
        entry.session_last_seen.clear();
        entry.last_select_at.clear();
    }
    let online_secs = entry.online_since.elapsed().as_secs();
    let now_i = Instant::now();
    let mut tasks = payload.tasks;
    for t in tasks.iter_mut() {
        if !t.recent_messages.is_empty() {
            entry.messages.insert(t.id.clone(), std::mem::take(&mut t.recent_messages));
        }
    }
    // 会话状态变化事件（对比旧快照）
    let mut events: Vec<crate::dingtalk::NotifyEvent> = Vec::new();
    // 本轮要落到 known_sessions 的增改 / 删除（owner 块内只收集，块后统一 apply，避开借用冲突）
    let mut known_updates: Vec<am_core::model::Task> = Vec::new();
    let mut known_removes: Vec<String> = Vec::new();
    // 同理：new_session_pending 的增删也在块内收集、块后 apply
    let mut pending_sets: Vec<(String, String)> = Vec::new(); // (会话 id, 终端锚)
    let mut pending_removes: Vec<String> = Vec::new();
    // 会话结束时要落的历史记录（同样块内收集、块后写，避开借用冲突）
    // (记录, 终端锚) —— 号位要在锁外查，见下方 pending_history 处理
    let mut history_records: Vec<(crate::history::HistoryEntry, String)> = Vec::new();
    if let Some(owner) = &notify_owner {
        use crate::dingtalk::{EventKind, NotifyEvent};
        let old: std::collections::HashMap<&str, TaskStatus> =
            entry.tasks.iter().map(|t| (t.id.as_str(), t.status)).collect();
        // 上一轮还是「进程占位任务」的那些终端锚。
        //
        // 占位任务一收到输入就落盘 jsonl、配上真会话，id 从 `<machine>-pid-<pid>` 换成
        // 会话 uuid —— 对用户而言是同一个终端接着往下用，不是新开了一个会话。此时若照常
        // 推「🆕 会话开始」，配上另一头的「✅ 会话已结束」（占位任务消失），一次下发就
        // 收到两条通知，而实际什么都没开始也没结束。
        //
        // 认锚不认 id：占位任务与转正后的会话共享同一个终端锚（machine|sh:pid@start），
        // 这是唯一能把两者串起来的线索。
        let placeholder_anchors: std::collections::HashSet<String> = entry
            .tasks
            .iter()
            .filter(|t| is_proc_placeholder(t))
            .map(crate::slots::anchor_of)
            .collect();
        // 会话基线是否已建立：空表示刚（重）上线还没建基线，此时出现的会话不算「新」。
        let baseline = !entry.known_sessions.is_empty();
        let current_ids: std::collections::HashSet<&str> =
            tasks.iter().map(|t| t.id.as_str()).collect();
        // 会话「真实年龄」闸门：只有刚开不久（started_at 在近几分钟内）的会话才算真·新。
        // 这比「上线沉降窗口」可靠得多——旧会话 started_at 是很久以前，无论被重连/客户端重启/
        // mac App Nap 拖慢扫描在多久后才扫回，都不会被误推「会话开始」。解析失败按「不新」处理
        // （偏保守：宁可漏推一条真新，也不刷屏旧会话）。
        const NEW_SESSION_MAX_AGE_SECS: i64 = 5 * 60;
        let now_s = crate::state::now_secs() as i64;
        let freshly_started = |t: &am_core::model::Task| -> bool {
            t.started_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| {
                    let age = now_s - dt.timestamp();
                    (0..=NEW_SESSION_MAX_AGE_SECS).contains(&age)
                })
                .unwrap_or(false)
        };
        let dev = &entry.hostname;
        // markdown 正文：设备/终端/项目/会话（两空格软换行，钉钉 markdown 才逐行断开）。
        // `{{NO}}`（format! 编译后为 `{NO}`）占位由 deliver 换成会话编号。
        let body = |t: &am_core::model::Task| -> String {
            let title = if t.title.is_empty() { t.provider_dsr.clone() } else { t.title.clone() };
            let title: String = title.chars().take(40).collect();
            format!(
                "**设备**：{dev}  \n**终端**：{}  \n**项目**：{}  \n**会话**：{{NO}}{title}",
                t.provider_dsr, t.project_name
            )
        };
        // 「最后结果」：取该会话最近一条 assistant 文本（本身是 markdown）。返回
        // (推送里展示的截断版, 若被截断则给出完整原文供 OTO 作为文件补发)。
        let msgs_map = &entry.messages;
        let result = |id: &str| -> (String, Option<String>) {
            // 正文不再硬截断：完整结果整段交给 dingtalk::push_*，那边按钉钉 4000 上限**分片**
            // 逐条发进聊天里（chunk_text 尽量断在换行/空格）。此前这里 take(LIMIT) 把话砍掉、
            // 只在下面附个 .txt——长结果在聊天里看不全，正是要修的「最后结果太长被砍掉」。
            //
            // 只有极长（会被分成很多条、刷屏）才额外附一份完整 .txt 兜底，既不刷屏也留个整档。
            const HUGE: usize = crate::mdfmt::DINGTALK_MAX_LEN * 3;
            msgs_map
                .get(id)
                .and_then(|ms| ms.iter().rev().find(|m| m.role.as_str() == "assistant"))
                .map(|m| {
                    let full = m.content.trim();
                    // 结果正文里的 markdown 标题转成加粗，避免推送里出现大字号 heading
                    let s = md_headings_to_bold(full);
                    if s.is_empty() {
                        (String::new(), None)
                    } else {
                        let file =
                            (full.chars().count() > HUGE).then(|| full.to_string());
                        (format!("\n\n**最后结果**\n\n{s}"), file)
                    }
                })
                .unwrap_or((String::new(), None))
        };
        // 「占位会话」：没有真实内容可看的空壳，结束时推一条「会话已结束」纯属噪音
        //（认不出是哪个、也没有任何结果可看），所以不推。两种形态：
        //   ① 进程占位任务（`is_proc_placeholder`）—— 只有进程还没配上 jsonl；下发任务后
        //      它换成真会话 id 就地消失，这条「消失」本不该报丧。
        //   ② 有会话文件但标题与提示词都空 —— 刚开终端还没输入过。
        // 只挡推送，基线仍要照常清理，否则会被后面的「消失」判定再推一次。
        let is_placeholder = |t: &am_core::model::Task| {
            is_proc_placeholder(t) || (t.title.is_empty() && t.prompt.is_empty())
        };
        // 造一条 assistant 侧的交互记录（任务完成 / 会话结束时的结果）。
        // 正文优先用完整原文（推送里被截断时 full 有值），否则退回展示版并去掉「最后结果」抬头。
        // 号位要 await 才能查，所以这里带回终端锚，等锁释放后再补。
        let make_reply = |t: &am_core::model::Task,
                          owner: &str,
                          full: Option<&str>,
                          shown: &str|
         -> (crate::history::HistoryEntry, String) {
            let content = match full {
                Some(f) => f.to_string(),
                None => shown.trim_start_matches("\n\n**最后结果**\n\n").to_string(),
            };
            (
                crate::history::HistoryEntry {
                    id: crate::history::new_id(),
                    owner: owner.to_string(),
                    session_id: t.id.clone(),
                    role: "assistant".into(),
                    content,
                    at: crate::state::now_secs(),
                    source: String::new(),
                    slot: None, // 锁外补
                    hostname: t.hostname.clone(),
                    project: t.project_name.clone(),
                    title: t.title.clone(),
                    provider: t.provider_dsr.clone(),
                },
                crate::slots::anchor_of(t),
            )
        };
        // 「等待选择」判定：从末尾回看最近一条实质消息 —— 若先遇到 select（其后没有 user/
        // tool_result 应答），说明仍在等你选。比「末条恰好是 select」稳健：AskUserQuestion 记录
        // 常不在绝对末尾（后面可能还跟 assistant 文本），但只要没被应答就仍算等待。
        let is_pending_select = |ms: &[am_core::model::MessageBrief]| -> bool {
            for m in ms.iter().rev() {
                match m.role.as_str() {
                    "todos" | "bgtasks" => continue,
                    "select" => return true,
                    "user" | "tool_result" => return false,
                    _ => continue, // assistant/tool/plan：继续往前看
                }
            }
            false
        };
        let now_selecting: std::collections::HashSet<String> = tasks
            .iter()
            .filter(|t| msgs_map.get(&t.id).map(|ms| is_pending_select(ms)).unwrap_or(false))
            .map(|t| t.id.clone())
            .collect();
        for t in &tasks {
            // 状态跃迁（会话仍在）：任务完成（Running→Idle）/ 结束（→Finished）。
            // 重连/客户端重启那一轮（was_offline）绝不比对：此时 `old` 还是重启【前】的旧快照，
            // 离线期间会话状态早变了，逐条比对会把「离线期间的状态变化」在重连瞬间一次性刷屏
            //（把所有会话都推一遍）。这一轮只重建基线（下面 known_updates），下一轮起 `old`
            // 已是重连后的快照，再正常比对。
            if !was_offline {
                if let Some(&prev) = old.get(t.id.as_str()) {
                    // 等待选择的会话不推「任务完成」——它由下面的「需要你选择」覆盖，避免同时两条
                    if prev == TaskStatus::Running
                        && t.status == TaskStatus::Idle
                        && !now_selecting.contains(&t.id)
                    {
                        let (res, full) = result(&t.id);
                        // 任务完成的结果进历史的 assistant 侧 —— 这是「我发了什么→它回了什么」
                        // 里最有价值的一半，不能只在会话结束时才记。
                        history_records.push(make_reply(t, owner, full.as_deref(), &res));
                        events.push(NotifyEvent {
                            owner: owner.clone(),
                            kind: EventKind::Waiting,
                            task_id: Some(t.id.clone()),
                            text: format!("**🔔 任务完成 · 等待你的操作**\n\n{}{}", body(t), res),
                            full_content: full,
                        });
                    } else if prev != TaskStatus::Finished && t.status == TaskStatus::Finished {
                        // 空壳占位会话结束不推（噪音）；基线照常清理
                        if !is_placeholder(t) {
                            let (res, full) = result(&t.id);
                            // 留一条历史：终端关了、机器关机后仍能回看这个会话最后出了什么
                            history_records.push(make_reply(t, owner, full.as_deref(), &res));
                            events.push(NotifyEvent {
                                owner: owner.clone(),
                                kind: EventKind::Finished,
                                task_id: Some(t.id.clone()),
                                text: format!("**✅ 会话已结束**\n\n{}{}", body(t), res),
                                full_content: full,
                            });
                        }
                        known_removes.push(t.id.clone()); // 已结束：移出基线，别再被「消失」判一次
                    }
                }
            }
            // 会话开始：基线里没有 = 尚未见过；再叠一道「真实年龄」闸门（近几分钟内才开的）
            // 才算真·新——彻底堵住重连/重启把旧会话扫回后误推。基线 + 沉降窗口作为额外去重保留。
            if !entry.known_sessions.contains_key(&t.id)
                && baseline
                && !was_offline
                && online_secs >= NEW_SESSION_SETTLE_SECS
                && freshly_started(t)
            {
                // 再加一道**配对沉降**：新会话刚扫到时客户端的配对多半还没稳定（权威 pin 要等
                // claude 跑起工具才抓得到，在那之前 mtime 启发式可能把它配到隔壁终端），此刻
                // 推出去的 {NO} 会指向别人 —— 实测给 9 号下发后收到的「会话开始」写着 #11。
                // 要求终端锚连续稳定 NEW_SESSION_PAIR_SETTLE_SECS 才推；锚一变就重新计时。
                // 沉降期内**不进基线**（下面 continue 跳过 known_updates），否则下一轮
                // `!known_sessions.contains_key` 不成立，这条会话就永远不会再被判为「新」。
                let anchor = crate::slots::anchor_of(t);
                // 占位任务转正：同一个终端接着用，不是新会话（见 placeholder_anchors）。
                // 仍要落进基线（不 continue），否则下一轮它又会被当成没见过的新会话。
                if placeholder_anchors.contains(&anchor) {
                    known_updates.push(t.clone());
                    continue;
                }
                match entry.new_session_pending.get(&t.id) {
                    Some((since, a))
                        if a == &anchor
                            && since.elapsed().as_secs()
                                >= crate::state::NEW_SESSION_PAIR_SETTLE_SECS =>
                    {
                        pending_removes.push(t.id.clone());
                        events.push(NotifyEvent {
                            owner: owner.clone(),
                            kind: EventKind::NewSession,
                            task_id: Some(t.id.clone()),
                            text: format!("**🆕 会话开始**\n\n{}", body(t)),
                            full_content: None,
                        });
                    }
                    // 锚相同但还没到时间 → 继续等（不重置计时）
                    Some((_, a)) if a == &anchor => continue,
                    // 首次见到，或锚变了（配对还在抖）→ (重新)计时
                    _ => {
                        pending_sets.push((t.id.clone(), anchor));
                        continue;
                    }
                }
            }
            known_updates.push(t.clone());
        }
        // 消失的会话 = 结束（去抖）：只有连续消失超过 FINISH_GRACE_SECS 才判结束，
        // 抹掉配对振荡时一两个周期的抖动（消失即回，last_seen 很近，不会误判结束）。
        for (id, task) in entry.known_sessions.iter() {
            if current_ids.contains(id.as_str()) || known_removes.contains(id) {
                continue;
            }
            // 上次就已是 Finished 的会话「消失」= 滑出 7 天窗口（早就结束了），不是刚结束：
            // 冷启动/重连把既有 Finished 会话登记进基线，一周后它老化滑出窗口时不该再推一次
            //「会话已结束」。真·刚结束的会话走上面的「Running/Idle→Finished 跃迁」推送并移出
            // 基线，不会走到这里。只有上次还活着的会话消失才算真结束。移出基线即可，不推。
            if task.status == TaskStatus::Finished {
                known_removes.push(id.clone());
                continue;
            }
            // 最近在等待选择的会话：仍活着（用户在慢慢选），配对抖动导致的消失不算结束
            let select_protected = entry
                .last_select_at
                .get(id)
                .map(|t| t.elapsed().as_secs() < crate::state::SELECT_PROTECT_SECS)
                .unwrap_or(false);
            if select_protected {
                continue;
            }
            let gone = entry
                .session_last_seen
                .get(id)
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(u64::MAX);
            if gone >= crate::state::FINISH_GRACE_SECS {
                // 早就没动静的会话消失了，不值得打扰：终端可能几小时前就关了
                //（关掉窗口后 claude 进程常残留），此刻收到一条「会话已结束」
                // 只会莫名其妙 —— 用户早不记得有这么个会话。
                //
                // 这类「消失」也不止于此：客户端换版本导致扫描口径变化、历史会话
                // 滑出窗口，都会让一批陈年会话同时消失。
                //
                // 同上：空壳占位会话消失不推，只清基线
                if !is_placeholder(task)
                    && worth_finish_notice(task.mtime_ms, crate::state::now_secs())
                {
                    let (res, full) = result(id);
                    history_records.push(make_reply(task, owner, full.as_deref(), &res));
                    events.push(NotifyEvent {
                        owner: owner.clone(),
                        kind: EventKind::Finished,
                        task_id: Some(id.clone()),
                        text: format!("**✅ 会话已结束**\n\n{}{}", body(task), res),
                        full_content: full,
                    });
                }
                known_removes.push(id.clone());
            }
        }
        // 设备上线边沿
        if was_offline && !entry.notified_online {
            events.push(NotifyEvent {
                owner: owner.clone(),
                kind: EventKind::Device,
                task_id: None,
                text: format!("**🟢 设备上线**\n\n**设备**：{dev}"),
                full_content: None,
            });
        }
        // 交互式选择提醒：会话仍在等待选择（now_selecting，已在上方按「最近实质消息是未应答的
        // select」判定）且尚未提醒过时，推一条。edge 触发靠 select_notified 去重。
        for t in &tasks {
            if now_selecting.contains(&t.id) && !entry.select_notified.contains(&t.id) {
                // 选项文本取自该会话最近一条 select 消息（未必是绝对末条）
                let opts = msgs_map
                    .get(&t.id)
                    .and_then(|ms| ms.iter().rev().find(|m| m.role.as_str() == "select"))
                    .map(|m| select_options_text(&m.content))
                    .unwrap_or_default();
                events.push(NotifyEvent {
                    owner: owner.clone(),
                    kind: EventKind::Select,
                    task_id: Some(t.id.clone()),
                    text: format!(
                        "**⌨️ 需要你选择**\n\n{}\n\n{}\n\n回复「发 {{N}} 序号」作答",
                        body(t),
                        opts
                    ),
                    full_content: None,
                });
            }
        }
        // 记下正在等待选择的会话时刻：其之后若配对抖动消失，disappear 分支据此保护、不误推结束
        for id in &now_selecting {
            entry.last_select_at.insert(id.clone(), now_i);
        }
        entry.select_notified = now_selecting;
    }
    if notify_owner.is_some() {
        entry.notified_online = true;
    }
    // 应用会话基线的增改/删除（owner 块内借用 entry 只读，故延到此处统一 apply）。
    // 先增改后删除：状态跃迁到 Finished 的会话既在 updates 也在 removes，净效果为移除。
    for t in &known_updates {
        entry.known_sessions.insert(t.id.clone(), t.clone());
        entry.session_last_seen.insert(t.id.clone(), now_i);
    }
    for id in &known_removes {
        entry.known_sessions.remove(id);
        entry.session_last_seen.remove(id);
        entry.last_select_at.remove(id);
        entry.new_session_pending.remove(id);
    }
    // 「会话开始」的配对沉降表：登记/重新计时的写在这里落，已推的移除
    for (id, anchor) in pending_sets {
        entry.new_session_pending.insert(id, (now_i, anchor));
    }
    for id in &pending_removes {
        entry.new_session_pending.remove(id);
    }
    // 兜底清理：会话没等到锚稳定就消失、或早已过了「真实年龄」闸门（5 分钟）不会再推的，
    // 留着只会让表无限长。10 分钟一刀切即可。
    entry.new_session_pending.retain(|_, (since, _)| since.elapsed().as_secs() < 10 * 60);
    entry.tasks = tasks;
    // 会话历史（需要 &state，故在释放 machines 锁之后写 —— 见函数末尾）
    let pending_history = history_records;
    // 缓存 agent 回传的 git 对比结果
    for r in payload.dir_results {
        entry.dir_cache.insert((r.task_id.clone(), r.rel.clone()), (r.dirs, r.files));
    }
    // 文件夹操作结果：按 op_id 存起来供网页轮询（上限防止 map 无限涨）
    for r in payload.fs_op_results {
        entry.fsop_results.insert(r.op_id.clone(), r);
    }
    if entry.fsop_results.len() > 256 {
        entry.fsop_results.clear();
    }
    // 清掉已消失会话的缓存：这两张表按会话 ID 累积，不清理的话
    // hub 长期运行会随「历史会话总数」无限增长（而非「当前会话数」）。
    let alive: std::collections::HashSet<&str> =
        entry.tasks.iter().map(|t| t.id.as_str()).collect();
    entry.messages.retain(|k, _| alive.contains(k.as_str()));
    // 输入指令一律即时下发到终端：点了发送就直接键入终端会话，是否「排队」由终端里
    // claude 自己的原生队列决定（会话跑着时新输入排在其后、被接收后才执行），hub 不再
    // 代为扣留。（撤回按「终端队列是否已接收」判定，见前端。）
    let commands: Vec<ControlCmd> = entry.pending.drain(..).collect();
    let files: Vec<am_core::model::FileTransfer> = entry.pending_files.drain(..).collect();
    let dir_queries: Vec<am_core::model::DirQuery> = entry.pending_dir.drain(..).collect();
    let fs_ops: Vec<am_core::model::FsOp> = entry.pending_fsop.drain(..).collect();
    drop(machines);

    // 配置同步：锁已释放再算 —— 里面要拿 registry 与 configs 两把锁，
    // 在 machines 写锁里嵌套取锁是自找死锁。
    let (config_pulls, config_pushes) = sync_configs(
        &state,
        &payload.machine_id,
        &payload.config_bodies,
        &payload.config_probe,
        device_manifest,
    )
    .await;

    // 会话历史：锁已释放，这里统一落（record 内部去重 + 截断 + 标脏，tick 循环负责写盘）
    for (mut rec, anchor) in pending_history {
        // 补号位，让历史里的编号与钉钉的「@N」对得上。会话可能已结束、活跃列表里查不到，
        // 所以按终端锚直接查表（锚要过保留期才回收，多数情况仍在）。
        rec.slot = crate::slots::slot_of(&state, &rec.owner, &anchor).await;
        crate::history::append(&state, rec).await;
    }

    // 钉钉推送：不阻塞上报响应，后台异步发
    if !events.is_empty() {
        let st = state.clone();
        let now_ms = crate::state::now_secs() * 1000;
        tokio::spawn(async move { crate::dingtalk::deliver(&st, events, now_ms).await });
    }

    // 告知 agent 是否已被信任：未信任时 agent 不应再上报任何会话数据
    let trusted = state.registry.read().await.device_meta(&payload.machine_id).trusted;
    // hubVersion：只推「安装包已上传」的版本，避免推送早于构建/上传完成（见 ready_desktop_version）
    let downloads_dir = std::env::var("AM_DOWNLOADS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.config.data_dir.join("downloads"));
    ok(json!({
        "commands": commands,
        "files": files,
        "dirQueries": dir_queries,
        "fsOps": fs_ops,
        // 配置同步：向源机索要的路径 / 向镜像机下发的内容（两者互斥，见 sync_configs）
        "configPulls": config_pulls,
        "configPushes": config_pushes,
        "trusted": trusted,
        "hubVersion": ready_desktop_version(&downloads_dir),
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

    // 推送当前用户可见的活跃会话快照。
    // 必须和 `list_tasks` 走同一个 `with_slots`：前端两条通路共用一份快照，
    // WS 推送若少了 `slot`，一来就把轮询拿到的号位覆盖没了（徽标闪一下就消失）。
    async fn snapshot(state: &SharedState, user: &str) -> String {
        let tasks: Vec<_> = state
            .tasks_for(user)
            .await
            .into_iter()
            .filter(|t| t.status != TaskStatus::Finished)
            .collect();
        let data = with_slots(state, user, &tasks).await;
        serde_json::to_string(&json!({ "type": "tasks", "data": data })).unwrap_or_default()
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

#[cfg(test)]
mod select_summary_tests {
    use super::select_summary;
    use serde_json::json;

    fn two_questions() -> serde_json::Value {
        json!({"questions": [
            {"question": "这两个值从哪来？", "options": [
                {"label": "前端自算"}, {"label": "等后端补字段"}, {"label": "复用总体增长率"}
            ]},
            {"question": "允许选几个？", "options": [
                {"label": "单选，可取消"}, {"label": "多选（最多 2 个）"}
            ]}
        ]})
    }

    /// 多题之间必须有空行分隔。
    ///
    /// 线上翻过车：没有空行时 markdown 的 lazy continuation 把下一题的题干当成上一题
    /// 最后一个选项的续行，两道题被渲染成一道，选项还连号成了 1~6。
    #[test]
    fn questions_separated_by_blank_line() {
        let s = select_summary(&two_questions());
        // 第二题的题干必须自成一段，前面隔着空行 —— 不能紧贴在上一个选项行后面
        assert!(
            s.contains("\n\n") && !s.contains("复用总体增长率\n允许选几个？"),
            "第二题题干被粘到了上一题的选项后面：\n{s}"
        );
        assert!(s.contains("**第 1 题**") && s.contains("**第 2 题**"), "多题要标题号：\n{s}");
    }

    /// 题干与其选项之间也要空行，否则列表不会被当成新列表起头
    #[test]
    fn question_and_options_separated() {
        let s = select_summary(&two_questions());
        assert!(s.contains("这两个值从哪来？\n\n1. 前端自算"), "得到：\n{s}");
    }

    /// 每题各自从 1 开始编号，且要提示逐题作答 —— 看到两组「1.」容易以为能直接回第二题
    #[test]
    fn each_question_numbers_from_one() {
        let s = select_summary(&two_questions());
        assert!(s.contains("1. 前端自算") && s.contains("1. 单选，可取消"), "得到：\n{s}");
        assert!(s.contains("逐题作答"), "多题要提示作答顺序：\n{s}");
    }

    /// 单题不加题号前缀，免得平白多一行
    #[test]
    fn single_question_has_no_index_prefix() {
        let s = select_summary(&json!({"questions": [
            {"question": "继续吗？", "options": [{"label": "继续"}, {"label": "停"}]}
        ]}));
        assert!(!s.contains("第 1 题"), "单题不该有题号：\n{s}");
        assert!(s.starts_with("继续吗？"), "得到：\n{s}");
    }

    /// 多选要标出来并给出作答格式（Submit 占 N+2 号）
    #[test]
    fn multi_select_marked_with_submit_hint() {
        let s = select_summary(&json!({"questions": [
            {"question": "选哪些？", "multiSelect": true,
             "options": [{"label": "A"}, {"label": "B"}]}
        ]}));
        assert!(s.contains("（多选）"), "得到：\n{s}");
        assert!(s.contains("补 4＝Submit"), "2 个选项时 Submit 应是 4 号：\n{s}");
    }
}

#[cfg(test)]
mod chunked_support_tests {
    use super::agent_supports_chunked;

    /// 达标与超出都放行
    #[test]
    fn new_enough_versions_pass() {
        assert!(agent_supports_chunked("0.10.5"));
        assert!(agent_supports_chunked("0.10.6"));
        assert!(agent_supports_chunked("0.11.0"));
        assert!(agent_supports_chunked("1.0.0"));
        assert!(agent_supports_chunked("v0.10.5"), "带 v 前缀也要认");
    }

    /// 差一个补丁号都不行 —— 旧 agent 会把每片当整份写坏
    #[test]
    fn older_versions_rejected() {
        assert!(!agent_supports_chunked("0.10.4"));
        assert!(!agent_supports_chunked("0.9.9"));
        assert!(!agent_supports_chunked("0.1.0"));
    }

    /// 解析不出来一律按「不支持」：宁可大文件走不通并给出提示，
    /// 也不能赌 —— 赌错就是文件被静默写坏，而用户以为传成功了
    #[test]
    fn unparsable_is_treated_as_unsupported() {
        assert!(!agent_supports_chunked(""));
        assert!(!agent_supports_chunked("unknown"));
        assert!(!agent_supports_chunked("0.10"), "位数不足不能当成 0.10.0 放行");
        assert!(!agent_supports_chunked("a.b.c"));
    }

    /// 预发布后缀要能容忍：0.10.5-beta 的能力与 0.10.5 相同
    #[test]
    fn prerelease_suffix_tolerated() {
        assert!(agent_supports_chunked("0.10.5-beta1"));
        assert!(!agent_supports_chunked("0.10.4-rc1"));
    }
}

#[cfg(test)]
mod proc_placeholder_tests {
    use super::is_proc_placeholder_id;

    /// 正常形态：客户端 attach_machine 给 `pid-<pid>` 加了机器前缀
    #[test]
    fn prefixed_placeholder_is_detected() {
        assert!(is_proc_placeholder_id("mach01-pid-1234", "mach01"));
    }

    /// machine_id 为空（老客户端 / 还没归属）时的裸形态也要认出来
    #[test]
    fn bare_placeholder_is_detected() {
        assert!(is_proc_placeholder_id("pid-1234", ""));
        assert!(is_proc_placeholder_id("pid-1234", "mach01"), "前缀剥不掉时按裸形态兜底");
    }

    /// 真会话（jsonl 的 uuid 文件名）绝不能被误判成占位 —— 误判就是漏推「会话已结束」
    #[test]
    fn real_session_is_not_placeholder() {
        assert!(!is_proc_placeholder_id("mach01-9f3c-4a1e-bb02-77d1", "mach01"));
        assert!(!is_proc_placeholder_id("9f3c4a1e-bb02-77d1", ""));
    }

    /// 会话 uuid 里恰好出现 `pid-` 字样不算数：只认剥掉机器前缀后的开头
    #[test]
    fn pid_substring_elsewhere_is_not_placeholder() {
        assert!(!is_proc_placeholder_id("mach01-abc-pid-1234", "mach01"));
        assert!(!is_proc_placeholder_id("rapid-7788", ""));
    }
}

#[cfg(test)]
mod finish_notice_tests {
    use super::{worth_finish_notice, STALE_FINISH_SECS};

    const NOW: u64 = 1_800_000_000;

    /// 刚还在用的会话结束了，该说一声
    #[test]
    fn fresh_session_is_worth_notifying() {
        let two_min_ago = (NOW - 120) * 1000;
        assert!(worth_finish_notice(two_min_ago, NOW));
    }

    /// 几小时没动静的会话消失，说了只是打扰 ——
    /// 关掉终端后 claude 进程常残留，这类僵尸会话可能挂很久才被清理掉，
    /// 用户早不记得有这么个会话（线上就因此在凌晨收到过莫名其妙的结束提醒）。
    #[test]
    fn stale_session_is_silently_dropped() {
        let three_hours_ago = (NOW - 3 * 3600) * 1000;
        assert!(!worth_finish_notice(three_hours_ago, NOW));
    }

    /// 边界两侧各验一次，防阈值写反
    #[test]
    fn boundary_is_inclusive() {
        assert!(worth_finish_notice((NOW - STALE_FINISH_SECS) * 1000, NOW), "刚好卡线仍推");
        assert!(!worth_finish_notice((NOW - STALE_FINISH_SECS - 1) * 1000, NOW), "过线即不推");
    }

    /// 拿不到 mtime（0）时按「很旧」处理：信息不足就别打扰
    #[test]
    fn unknown_mtime_is_treated_as_stale() {
        assert!(!worth_finish_notice(0, NOW));
    }

    /// mtime 比当前时间还新（客户端时钟快）不能算成「极旧」而漏推
    #[test]
    fn future_mtime_does_not_underflow() {
        assert!(worth_finish_notice((NOW + 60) * 1000, NOW), "时钟偏差不该吞掉通知");
    }
}
