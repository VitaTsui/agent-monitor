//! vita-admin 前端所需的最小后端契约：登录（RSA+AES）、菜单、权限。
use crate::crypto;
use crate::state::SharedState;
use axum::extract::State;
use axum::Json;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};

/// 统一响应信封：成功 code=0
pub fn ok(data: Value) -> Json<Value> {
    Json(json!({ "code": 0, "msg": "ok", "data": data }))
}

pub fn err(code: i64, msg: &str) -> Json<Value> {
    Json(json!({ "code": code, "msg": msg, "data": null }))
}

/// GET /auth/access/getCryptoKey
/// 生成随机会话密钥（32 字符），用共享 CRYPTO_KEY 加密后下发
pub async fn get_crypto_key(State(state): State<SharedState>) -> Json<Value> {
    let session_key: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    match crypto::aes_gcm_encrypt(&session_key, &state.config.crypto_key) {
        Ok(blob) => ok(json!(blob)),
        Err(e) => err(500, &format!("生成密钥失败: {e}")),
    }
}

/// GET /auth/access/isNeedLoginCaptcha
pub async fn is_need_captcha() -> Json<Value> {
    ok(json!(false))
}

/// GET /auth/access/dingtalk/url
pub async fn dingtalk_url() -> Json<Value> {
    ok(json!({ "enabled": false, "url": "" }))
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
    #[serde(rename = "cryptoKey")]
    pub crypto_key: String,
    #[allow(dead_code)]
    #[serde(rename = "codeKey", default)]
    pub code_key: String,
    #[allow(dead_code)]
    #[serde(rename = "codeVal", default)]
    pub code_val: String,
}

/// POST /auth/access/login
pub async fn login(State(state): State<SharedState>, Json(req): Json<LoginReq>) -> Json<Value> {
    // 1. 用共享密钥恢复会话密钥
    let session_key = match crypto::aes_gcm_decrypt(&req.crypto_key, &state.config.crypto_key) {
        Ok(k) => k,
        Err(_) => return err(400, "cryptoKey 无效"),
    };
    // 2. RSA + AES 双层解密用户名密码
    let username =
        match crypto::decrypt_login_field(&req.username, &session_key, &state.config.private_key) {
            Ok(u) => u,
            Err(_) => return err(400, "用户名解密失败"),
        };
    let password =
        match crypto::decrypt_login_field(&req.password, &session_key, &state.config.private_key) {
            Ok(p) => p,
            Err(_) => return err(400, "密码解密失败"),
        };
    // 3. 爆破节流：该账号连续失败越多，本次应答越慢
    let delay = state.login_throttle.read().await.delay_for(&username);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    // 4. 校验（走用户注册表）
    let (user, is_super) = {
        let reg = state.registry.read().await;
        match reg.authenticate(&username, &password) {
            Some(u) => {
                let is_super = reg.is_super_user(&u.username);
                (u, is_super)
            }
            None => {
                state.login_throttle.write().await.record_fail(&username);
                return err(400, "用户名或密码错误");
            }
        }
    };
    state.login_throttle.write().await.record_success(&username);
    let token = uuid::Uuid::new_v4().to_string();
    state.tokens.write().await.insert(
        token.clone(),
        crate::state::Session::new(user.username.clone()),
    );
    state
        .sessions_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let nickname = if user.display.is_empty() {
        user.username.clone()
    } else {
        user.display.clone()
    };
    ok(json!({
        "token": token,
        "userInfo": {
            "id": user.id,
            "username": user.username,
            "nickname": nickname,
            "isSuper": is_super
        }
    }))
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub username: String,
    pub password: String,
    #[serde(rename = "cryptoKey")]
    pub crypto_key: String,
    #[serde(default)]
    pub nickname: String,
}

/// POST /auth/access/register —— 自助注册用户
pub async fn register(
    State(state): State<SharedState>,
    Json(req): Json<RegisterReq>,
) -> Json<Value> {
    let session_key = match crypto::aes_gcm_decrypt(&req.crypto_key, &state.config.crypto_key) {
        Ok(k) => k,
        Err(_) => return err(400, "cryptoKey 无效"),
    };
    let username =
        match crypto::decrypt_login_field(&req.username, &session_key, &state.config.private_key) {
            Ok(u) => u,
            Err(_) => return err(400, "用户名解密失败"),
        };
    let password =
        match crypto::decrypt_login_field(&req.password, &session_key, &state.config.private_key) {
            Ok(p) => p,
            Err(_) => return err(400, "密码解密失败"),
        };
    let user = match state
        .registry
        .write()
        .await
        .register(&username, &password, &req.nickname)
    {
        Ok(u) => u,
        Err(e) => return err(400, &e),
    };
    let token = uuid::Uuid::new_v4().to_string();
    state.tokens.write().await.insert(
        token.clone(),
        crate::state::Session::new(user.username.clone()),
    );
    state
        .sessions_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    ok(json!({
        "token": token,
        "userInfo": { "id": user.id, "username": user.username, "nickname": user.display, "isSuper": false }
    }))
}

/// GET /auth/access/logout
pub async fn logout(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if let Some(t) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        state.tokens.write().await.remove(t);
        state
            .sessions_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    ok(json!(true))
}

/// 从 Authorization 头解析出用户名（无效或已过期返回 None）。
/// 滑动续期：有活动就顺延 30 天窗口（距上次续期超 1 小时才写，避免锁churn）。
pub async fn auth_user(state: &SharedState, headers: &axum::http::HeaderMap) -> Option<String> {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok())?;
    // 快路径只拿读锁；命中过期项时再拿写锁把它摘掉，顺带清理其它过期会话。
    let hit = state.tokens.read().await.get(token).cloned();
    match hit {
        Some(s) if !s.expired() => {
            if crate::state::now_secs().saturating_sub(s.last_seen)
                >= crate::state::SESSION_TOUCH_SECS
            {
                if let Some(entry) = state.tokens.write().await.get_mut(token) {
                    entry.touch();
                }
                state
                    .sessions_dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Some(s.username)
        }
        Some(_) => {
            let mut map = state.tokens.write().await;
            map.retain(|_, s| !s.expired());
            state
                .sessions_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
            None
        }
        None => None,
    }
}

/// 后管门卫：登录 + 超级管理员两重校验（部署令牌 X-Admin-Token 已废弃）。
/// 通过返回用户名；失败返回统一错误响应。
pub async fn admin_gate(
    state: &SharedState,
    headers: &axum::http::HeaderMap,
) -> Result<String, Json<Value>> {
    // 后管准入 = 已登录 + 是超级管理员即可（不再要求部署令牌 X-Admin-Token）。
    let Some(username) = auth_user(state, headers).await else {
        return Err(err(401, "未登录"));
    };
    if !state.registry.read().await.is_super_user(&username) {
        return Err(err(403, "后管仅限管理员使用"));
    }
    Ok(username)
}

#[derive(Deserialize)]
pub struct VerifyTokenReq {
    pub token: String,
}

/// POST /sys/admin/verify —— 后管锁屏校验部署令牌
pub async fn verify_admin_token(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<VerifyTokenReq>,
) -> Json<Value> {
    let Some(username) = auth_user(&state, &headers).await else {
        return err(401, "未登录");
    };
    if !state.registry.read().await.is_super_user(&username) {
        return err(403, "后管仅限管理员使用");
    }
    if !crate::state::token_eq(&req.token, &state.config.admin_token) {
        // 失败延迟，减缓暴力尝试
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        tracing::warn!("后管令牌校验失败（用户: {username}）");
        return err(400, "令牌不正确");
    }
    ok(json!(true))
}

/// GET /sys/menu/getMenuATopATopMenu —— 后管只有「用户管理」「版本管理」两页
pub async fn menus(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let menu_list = json!([
        {
            "id": "1", "nm": "用户管理", "pid": null, "seq": 1, "level": 1, "children": null,
            "path": "permit/user", "url": "permit/User/index", "perm": "permit:user:list",
            "icon": "carbon:user-multiple", "status": null
        },
        {
            "id": "2", "nm": "版本管理", "pid": null, "seq": 2, "level": 1, "children": null,
            "path": "sysmgmt/version", "url": "sysmgmt/Version/index", "perm": "sysmgmt:version:list",
            "icon": "carbon:upgrade", "status": null
        }
    ]);
    ok(json!({ "topMenuList": [], "menuList": menu_list, "topId": null, "topList": null }))
}

/// GET /sys/menu/getStringPermissions
pub async fn permissions(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    ok(json!({
        "stringPermissions": [
            "permit:user:list",
            "permit:user:add",
            "permit:user:upd",
            "permit:user:resetPwd",
            "permit:user:del",
            "sysmgmt:version:list",
            "sysmgmt:version:upd"
        ]
    }))
}

// ---------- 用户管理（后管唯一功能，全部走 admin_gate） ----------

/// GET /sys/user/page?query= —— vita-admin Query 分页
pub async fn user_page(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }

    let parsed: Value = q
        .get("query")
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);

    // 关键字过滤：取 r[].w[] 中 username / nickname 的 LK 条件
    let mut keyword = String::new();
    if let Some(rules) = parsed.get("r").and_then(Value::as_array) {
        for rule in rules {
            if let Some(conds) = rule.get("w").and_then(Value::as_array) {
                for cond in conds {
                    let k = cond.get("k").and_then(Value::as_str).unwrap_or("");
                    if k == "username" || k == "nickname" {
                        if let Some(v) = cond.get("v").and_then(Value::as_str) {
                            keyword = v.trim().to_lowercase();
                        }
                    }
                }
            }
        }
    }

    let reg = state.registry.read().await;
    let mut users: Vec<_> = reg
        .list_users()
        .into_iter()
        .filter(|u| {
            keyword.is_empty()
                || u.username.to_lowercase().contains(&keyword)
                || u.display.to_lowercase().contains(&keyword)
        })
        .collect();
    users.sort_by(|a, b| {
        a.id.parse::<u64>()
            .unwrap_or(0)
            .cmp(&b.id.parse::<u64>().unwrap_or(0))
    });

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
    let total = users.len();

    // 饱和运算：页码无上界，(n-1)*s 溢出会回绕（release）或 panic（debug）
    let items: Vec<Value> = users
        .into_iter()
        .skip(page_num.saturating_sub(1).saturating_mul(page_size))
        .take(page_size)
        .map(|u| {
            let is_super = reg.is_super_user(&u.username);
            json!({
                "id": u.id,
                "username": u.username,
                "nickname": u.display,
                "isSuper": is_super,
                "roleDsr": if is_super { "超级管理员" } else { "普通用户" },
                "deviceCount": reg.device_count_of(&u.username),
            })
        })
        .collect();

    ok(json!({
        "list": items,
        "page": { "pageNum": page_num, "pageSize": page_size, "total": total }
    }))
}

#[derive(Deserialize)]
pub struct UserAddReq {
    pub username: String,
    /// RSA(AES(密码, 会话密钥))，与登录口令同一加密方案
    pub password: String,
    #[serde(rename = "cryptoKey")]
    pub crypto_key: String,
    #[serde(default)]
    pub nickname: String,
}

/// POST /sys/user/add —— 新增用户（明文字段，仅后管内部使用）
pub async fn user_add(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<UserAddReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let session_key = match crypto::aes_gcm_decrypt(&req.crypto_key, &state.config.crypto_key) {
        Ok(k) => k,
        Err(_) => return err(400, "cryptoKey 无效"),
    };
    let password =
        match crypto::decrypt_login_field(&req.password, &session_key, &state.config.private_key) {
            Ok(p) => p,
            Err(_) => return err(400, "密码解密失败"),
        };
    match state
        .registry
        .write()
        .await
        .register(&req.username, &password, &req.nickname)
    {
        Ok(_) => ok(json!(true)),
        Err(e) => err(400, &e),
    }
}

#[derive(Deserialize)]
pub struct UserUpdReq {
    pub username: String,
    #[serde(default)]
    pub nickname: String,
}

/// POST /sys/user/upd —— 修改昵称
pub async fn user_upd(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<UserUpdReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    match state
        .registry
        .write()
        .await
        .update_display(&req.username, &req.nickname)
    {
        Ok(_) => ok(json!(true)),
        Err(e) => err(400, &e),
    }
}

#[derive(Deserialize)]
pub struct ResetPwdReq {
    pub username: String,
    /// RSA(AES(密码, 会话密钥))，与登录口令同一加密方案
    pub password: String,
    #[serde(rename = "cryptoKey")]
    pub crypto_key: String,
}

/// POST /sys/user/resetPwd —— 重置密码
pub async fn user_reset_pwd(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ResetPwdReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let session_key = match crypto::aes_gcm_decrypt(&req.crypto_key, &state.config.crypto_key) {
        Ok(k) => k,
        Err(_) => return err(400, "cryptoKey 无效"),
    };
    let password =
        match crypto::decrypt_login_field(&req.password, &session_key, &state.config.private_key) {
            Ok(p) => p,
            Err(_) => return err(400, "密码解密失败"),
        };
    match state
        .registry
        .write()
        .await
        .reset_password(&req.username, &password)
    {
        Ok(_) => ok(json!(true)),
        Err(e) => err(400, &e),
    }
}

/// GET /sys/user/del?ids=<username> —— 删除用户（项目删除约定：GET + ids 参数）
pub async fn user_del(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let username = q.get("ids").cloned().unwrap_or_default();
    if username.is_empty() {
        return err(400, "缺少 ids 参数");
    }
    // 删除用户时同步失效其登录态
    match state.registry.write().await.delete_user(&username) {
        Ok(_) => {
            // 用户已删除：踢掉其所有在线会话
            state
                .tokens
                .write()
                .await
                .retain(|_, s| s.username != username);
            state
                .sessions_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
            ok(json!(true))
        }
        Err(e) => err(400, &e),
    }
}

// ---------- 版本管理 / 更新日志（后管） ----------

fn downloads_dir(state: &SharedState) -> std::path::PathBuf {
    std::env::var("AM_DOWNLOADS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| state.config.data_dir.join("downloads"))
}

fn changelog_path(state: &SharedState) -> std::path::PathBuf {
    state.config.data_dir.join("changelog.json")
}

fn read_changelog(state: &SharedState) -> Vec<Value> {
    std::fs::read_to_string(changelog_path(state))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_changelog(state: &SharedState, list: &[Value]) -> Result<(), String> {
    let txt = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(changelog_path(state), txt).map_err(|e| e.to_string())
}

/// GET /sys/version/info —— 当前版本、强制更新下限与更新日志
pub async fn version_admin_info(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let manifest = std::fs::read_to_string(downloads_dir(&state).join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null);
    let pick = |ptr: &str| {
        manifest
            .pointer(ptr)
            .and_then(Value::as_str)
            .map(String::from)
    };
    ok(json!({
        "desktop": env!("CARGO_PKG_VERSION"),
        "desktopMin": pick("/desktop/minVersion"),
        "android": pick("/android/version"),
        "androidMin": pick("/android/minVersion"),
        "changelog": read_changelog(&state),
    }))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DingtalkAppAdminReq {
    #[serde(default)]
    pub app_key: String,
    #[serde(default)]
    pub app_secret: String,
}

/// GET /sys/dingtalk/app —— 后管查看全局钉钉机器人（密钥不回显）。
///
/// 全局机器人服务所有没自己配机器人的用户，他们各自绑一个钉钉号即可使用。
pub async fn dingtalk_app_admin_get(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let app = state.registry.read().await.global_dingtalk_app();
    ok(json!({
        "appKey": app.as_ref().map(|a| a.app_key.clone()).unwrap_or_default(),
        "hasSecret": app.as_ref().is_some_and(|a| !a.app_secret.is_empty()),
        // 已跟机器人说过话的人数（即已绑定的钉钉号数），给管理员一个「用起来没有」的感知
        "boundCount": state.registry.read().await.dingtalk_bound_count(),
    }))
}

/// POST /sys/dingtalk/app —— 后管保存全局钉钉机器人；appKey 传空 = 停用。
pub async fn dingtalk_app_admin_set(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<DingtalkAppAdminReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let app_key = req.app_key.trim().to_string();
    if app_key.is_empty() {
        state.registry.write().await.set_global_dingtalk_app("", "");
        state.dingtalk_reload.notify_one();
        return ok(json!({ "result": "已停用" }));
    }
    // 密钥留空 = 沿用已存的（界面不回显密钥，只改 appKey 时不该被清掉）
    let mut secret = req.app_secret.trim().to_string();
    if secret.is_empty() {
        secret = state
            .registry
            .read()
            .await
            .global_dingtalk_app()
            .map(|a| a.app_secret)
            .unwrap_or_default();
    }
    if secret.is_empty() {
        return err(400, "请填写 AppSecret");
    }
    state
        .registry
        .write()
        .await
        .set_global_dingtalk_app(&secret, &app_key);
    // 立刻重连 Stream，免得管理员配完等半分钟没反应
    state.dingtalk_reload.notify_one();
    ok(json!({ "result": "已保存" }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMinReq {
    pub desktop_min: Option<String>,
    pub android_min: Option<String>,
}

/// POST /sys/version/minimum —— 设置强制更新下限（写 manifest.json，全端即刻生效）
pub async fn version_set_minimum(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SetMinReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let path = downloads_dir(&state).join("manifest.json");
    let mut manifest: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(v) = req.desktop_min.as_deref().map(str::trim) {
        manifest["desktop"]["minVersion"] = json!(v);
    }
    if let Some(v) = req.android_min.as_deref().map(str::trim) {
        // 保留 android.version（APK 最新版号）不被覆盖
        if manifest
            .get("android")
            .map(|a| !a.is_object())
            .unwrap_or(true)
        {
            manifest["android"] = json!({});
        }
        manifest["android"]["minVersion"] = json!(v);
    }
    let Ok(txt) = serde_json::to_string_pretty(&manifest) else {
        return err(500, "序列化失败");
    };
    if let Err(e) = std::fs::write(&path, txt) {
        return err(500, &format!("写入 manifest 失败: {e}"));
    }
    tracing::info!(
        "后管更新强制更新下限: desktop={:?} android={:?}",
        req.desktop_min,
        req.android_min
    );
    ok(json!(true))
}

#[derive(Deserialize)]
pub struct ChangelogAddReq {
    pub version: String,
    #[serde(default)]
    pub date: String,
    pub notes: String,
}

/// POST /sys/version/changelog —— 新增一条更新日志（同版本号覆盖）
pub async fn changelog_add(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangelogAddReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let version = req.version.trim().to_string();
    if version.is_empty() || req.notes.trim().is_empty() {
        return err(400, "版本号与更新说明不能为空");
    }
    let date = if req.date.trim().is_empty() {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    } else {
        req.date.trim().to_string()
    };
    let mut list = read_changelog(&state);
    list.retain(|e| e.pointer("/version").and_then(Value::as_str) != Some(version.as_str()));
    list.insert(
        0,
        json!({ "version": version, "date": date, "notes": req.notes.trim() }),
    );
    if let Err(e) = write_changelog(&state, &list) {
        return err(500, &format!("写入失败: {e}"));
    }
    ok(json!(true))
}

#[derive(Deserialize)]
pub struct ChangelogDelReq {
    pub version: String,
}

/// POST /sys/version/changelog/del —— 删除一条更新日志
pub async fn changelog_del(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangelogDelReq>,
) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let mut list = read_changelog(&state);
    let before = list.len();
    list.retain(|e| e.pointer("/version").and_then(Value::as_str) != Some(req.version.trim()));
    if list.len() == before {
        return err(404, "该版本的日志不存在");
    }
    if let Err(e) = write_changelog(&state, &list) {
        return err(500, &format!("写入失败: {e}"));
    }
    ok(json!(true))
}
