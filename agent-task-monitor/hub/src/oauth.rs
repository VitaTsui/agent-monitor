//! 第三方登录（Google / Apple）。凭据来自环境变量，未配置则渠道 enabled=false，
//! 前端据此提示「管理员尚未配置」。回调地址统一为 <AM_PUBLIC_URL>/login。
use crate::admin::{err, ok};
use crate::state::SharedState;
use axum::extract::{Form, Path, Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 单个 OAuth 渠道的运行配置
struct ProviderCfg {
    client_id: String,
    client_secret: String,
    authorize_endpoint: &'static str,
    token_endpoint: &'static str,
    scope: &'static str,
    /// apple 需要 response_mode=form_post；google 不需要
    extra_authorize: &'static str,
    redirect_uri: String,
}

fn public_base() -> Option<String> {
    std::env::var("AM_PUBLIC_URL")
        .ok()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn provider_cfg(provider: &str) -> Option<ProviderCfg> {
    let base = public_base()?;
    match provider {
        "google" => {
            let client_id = std::env::var("AM_GOOGLE_CLIENT_ID").ok().filter(|s| !s.is_empty())?;
            let client_secret =
                std::env::var("AM_GOOGLE_CLIENT_SECRET").ok().filter(|s| !s.is_empty())?;
            Some(ProviderCfg {
                client_id,
                client_secret,
                authorize_endpoint: "https://accounts.google.com/o/oauth2/v2/auth",
                token_endpoint: "https://oauth2.googleapis.com/token",
                scope: "openid email profile",
                extra_authorize: "",
                // Google 支持 query 模式：直接回跳前端登录页，SPA 读 ?code=&state=
                redirect_uri: format!("{base}/login"),
            })
        }
        "apple" => {
            let client_id = std::env::var("AM_APPLE_CLIENT_ID").ok().filter(|s| !s.is_empty())?;
            // Apple 的 client_secret 是一段 ES256 JWT（有效期最长 6 个月），
            // 由管理员离线用 .p8 私钥生成后放入环境变量，避免在服务内内置 ES256 签名。
            let client_secret =
                std::env::var("AM_APPLE_CLIENT_SECRET").ok().filter(|s| !s.is_empty())?;
            Some(ProviderCfg {
                client_id,
                client_secret,
                authorize_endpoint: "https://appleid.apple.com/auth/authorize",
                token_endpoint: "https://appleid.apple.com/auth/token",
                scope: "name email",
                extra_authorize: "&response_mode=form_post",
                // Apple 用 form_post 回调（POST），由后端 callback 转成前端跳转
                redirect_uri: format!("{base}/auth/access/oauth/apple/callback"),
            })
        }
        _ => None,
    }
}

/// GET /auth/access/oauth/providers —— 一次性返回各渠道开关，
/// 免去登录页首屏为每个渠道各打一次探测请求。
pub async fn oauth_providers() -> Json<Value> {
    ok(json!({
        "google": provider_cfg("google").is_some(),
        "apple": provider_cfg("apple").is_some(),
    }))
}

/// 已签发 state 的有效期
const STATE_TTL_SECS: u64 = 600;

/// 单机在飞 state 的上限
const STATE_MAX: usize = 10_000;

/// 记录一个已签发的 state（并顺带清理过期项）
async fn issue_state(state: &SharedState, value: &str) {
    if value.is_empty() {
        return;
    }
    let mut map = state.oauth_states.write().await;
    map.retain(|_, t| t.elapsed().as_secs() < STATE_TTL_SECS);
    // 到上限时淘汰最旧的，而不是拒绝新签发——
    // 否则任何人都能无鉴权刷满这张表，让所有真实用户的 state 都存不进去，
    // 从而把全站第三方登录锁死。
    while map.len() >= STATE_MAX {
        // elapsed 最大 = 签发最早 = 最该淘汰的
        let oldest = map
            .iter()
            .max_by_key(|(_, t)| t.elapsed())
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
    map.insert(value.to_string(), std::time::Instant::now());
}

/// 校验并一次性消费 state；未签发过/已过期/已用过 → false
async fn consume_state(state: &SharedState, value: &Option<String>) -> bool {
    let Some(v) = value.as_deref().filter(|s| !s.is_empty()) else {
        return false;
    };
    let mut map = state.oauth_states.write().await;
    match map.remove(v) {
        Some(t) => t.elapsed().as_secs() < STATE_TTL_SECS,
        None => false,
    }
}

/// GET /auth/access/oauth/:provider/url?state= —— 授权地址（未配置 enabled=false）
pub async fn oauth_url(
    State(app): State<SharedState>,
    Path(provider): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let Some(cfg) = provider_cfg(&provider) else {
        return ok(json!({ "enabled": false, "url": "" }));
    };
    let state = q.get("state").cloned().unwrap_or_default();
    // 服务端登记该 state，回调时必须原样带回（防 OAuth CSRF / 登录劫持）
    issue_state(&app, &state).await;
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}{}",
        cfg.authorize_endpoint,
        urlencoding_encode(&cfg.client_id),
        urlencoding_encode(&cfg.redirect_uri),
        urlencoding_encode(cfg.scope),
        urlencoding_encode(&state),
        cfg.extra_authorize,
    );
    ok(json!({ "enabled": true, "url": url }))
}

/// POST /auth/access/oauth/apple/callback —— 接住 Apple 的 form_post 回调，
/// 302 跳转到前端登录页并把授权码带在 query 上，交给 SPA 走 oauth_login。
pub async fn apple_callback(Form(form): Form<HashMap<String, String>>) -> impl IntoResponse {
    let base = public_base().unwrap_or_default();
    let code = form.get("code").cloned().unwrap_or_default();
    let state = form.get("state").cloned().unwrap_or_default();
    let target = format!(
        "{base}/login?code={}&state={}",
        urlencoding_encode(&code),
        urlencoding_encode(&state),
    );
    Redirect::to(&target)
}

#[derive(Deserialize)]
pub struct OAuthLoginReq {
    pub code: String,
    #[serde(default)]
    pub state: Option<String>,
}

/// POST /auth/access/oauth/:provider/login —— 授权码换登录态（不存在则自动注册）
pub async fn oauth_login(
    State(state): State<SharedState>,
    Path(provider): Path<String>,
    Json(req): Json<OAuthLoginReq>,
) -> Json<Value> {
    let Some(cfg) = provider_cfg(&provider) else {
        return err(400, "该第三方登录未配置");
    };
    // 防 CSRF：state 必须是本服务签发且未使用过的
    if !consume_state(&state, &req.state).await {
        return err(400, "登录校验失败（state 无效或已过期），请重新登录");
    }

    // 1. 授权码换 token
    let client = reqwest::Client::new();
    let token_resp = client
        .post(cfg.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &req.code),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("redirect_uri", &cfg.redirect_uri),
        ])
        .send()
        .await;
    let token_json: Value = match token_resp {
        Ok(r) => r.json().await.unwrap_or(Value::Null),
        Err(e) => return err(500, &format!("换取令牌失败: {e}")),
    };

    // 2. 取邮箱：google 用 userinfo；apple 从 id_token 解出
    let (email, display) = match provider.as_str() {
        "google" => {
            let access = token_json
                .get("access_token")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if access.is_empty() {
                return err(400, "Google 未返回 access_token（检查授权码/配置）");
            }
            let info: Value = match client
                .get("https://openidconnect.googleapis.com/v1/userinfo")
                .bearer_auth(access)
                .send()
                .await
            {
                Ok(r) => r.json().await.unwrap_or(Value::Null),
                Err(e) => return err(500, &format!("获取 Google 用户信息失败: {e}")),
            };
            // 必须是已验证邮箱：否则拿到未验证的同名邮箱即可登入他人账号
            if !claim_email_verified(&info) {
                return err(400, "该 Google 账号的邮箱未验证，无法登录");
            }
            let email = info.get("email").and_then(Value::as_str).unwrap_or("").to_string();
            let name = info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (email, name)
        }
        "apple" => {
            let id_token = token_json
                .get("id_token")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let claims = decode_jwt_payload(id_token).unwrap_or(Value::Null);
            if !claim_email_verified(&claims) {
                return err(400, "该 Apple 账号的邮箱未验证，无法登录");
            }
            let email = claims
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (email, String::new())
        }
        _ => return err(400, "不支持的渠道"),
    };

    if email.is_empty() {
        return err(400, "第三方账号未返回邮箱，无法登录");
    }

    // 3. 找或建用户（用户名 = 邮箱），签发登录态
    let user = {
        let mut reg = state.registry.write().await;
        match reg.find_or_create_oauth(&email, &display, &provider) {
            Ok(u) => u,
            Err(e) => return err(400, &e),
        }
    };
    let is_super = state.registry.read().await.is_super_user(&user.username);
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

/// 简易 application/x-www-form-urlencoded 编码（仅转义 OAuth 场景常见字符）
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 邮箱是否已验证：Google/Apple 的 email_verified 可能是布尔或 "true" 字符串
fn claim_email_verified(v: &Value) -> bool {
    match v.get("email_verified") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// 解出 JWT 的 payload（不校验签名，仅取声明；base64url 中段）
fn decode_jwt_payload(jwt: &str) -> Option<Value> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}
