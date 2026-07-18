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
pub async fn register(State(state): State<SharedState>, Json(req): Json<RegisterReq>) -> Json<Value> {
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
    let user = match state.registry.write().await.register(&username, &password, &req.nickname) {
        Ok(u) => u,
        Err(e) => return err(400, &e),
    };
    let token = uuid::Uuid::new_v4().to_string();
    state
        .tokens
        .write()
        .await
        .insert(token.clone(), crate::state::Session::new(user.username.clone()));
    state.sessions_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    ok(json!({
        "token": token,
        "userInfo": { "id": user.id, "username": user.username, "nickname": user.display, "isSuper": false }
    }))
}

/// GET /auth/access/logout
pub async fn logout(State(state): State<SharedState>, headers: axum::http::HeaderMap) -> Json<Value> {
    if let Some(t) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        state.tokens.write().await.remove(t);
        state.sessions_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
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

/// 后管门卫：登录 + 超级管理员 + 部署令牌（X-Admin-Token）三重校验。
/// 通过返回用户名；失败返回统一错误响应。
pub async fn admin_gate(
    state: &SharedState,
    headers: &axum::http::HeaderMap,
) -> Result<String, Json<Value>> {
    let Some(username) = auth_user(state, headers).await else {
        return Err(err(401, "未登录"));
    };
    if !state.registry.read().await.is_super_user(&username) {
        return Err(err(403, "后管仅限管理员使用"));
    }
    let token = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::state::token_eq(token, &state.config.admin_token) {
        // 失败延迟，减缓对任意 /sys/* 接口的令牌爆破
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        return Err(err(4031, "后管访问令牌无效"));
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

/// GET /sys/menu/getMenuATopATopMenu —— 后管仅保留「用户管理」
pub async fn menus(State(state): State<SharedState>, headers: axum::http::HeaderMap) -> Json<Value> {
    if let Err(e) = admin_gate(&state, &headers).await {
        return e;
    }
    let menu_list = json!([
        {
            "id": "1", "nm": "用户管理", "pid": null, "seq": 1, "level": 1, "children": null,
            "path": "permit/user", "url": "permit/User/index", "perm": "permit:user:list",
            "icon": "carbon:user-multiple", "status": null
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
            "permit:user:del"
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
    users.sort_by(|a, b| a.id.parse::<u64>().unwrap_or(0).cmp(&b.id.parse::<u64>().unwrap_or(0)));

    let page_num = parsed.pointer("/p/n").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let page_size = parsed.pointer("/p/s").and_then(Value::as_u64).unwrap_or(20).clamp(1, 200) as usize;
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
    match state.registry.write().await.register(&req.username, &password, &req.nickname) {
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
    match state.registry.write().await.update_display(&req.username, &req.nickname) {
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
    match state.registry.write().await.reset_password(&req.username, &password) {
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
            state.tokens.write().await.retain(|_, s| s.username != username);
            state.sessions_dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            ok(json!(true))
        }
        Err(e) => err(400, &e),
    }
}
