//! am-hub —— agent-monitor 服务端。
//! 聚合各机上报、托管网页与下载、用户/设备/配对管理。纯服务端：不含扫描/托盘/上报。

mod admin;
mod bot;
mod commands;
/// 配置同步：各账号的配置基线与差异计算
mod configsync;
mod crypto;
mod dingtalk;
mod dingtalk_stream;
mod history;
mod mcp;
mod mdfmt;
mod oauth;
mod registry;
mod server;
mod slots;
mod state;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};
use state::{AppState, Config, SharedState};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "agent_task_monitor=info,info".into()),
        )
        .init();

    load_config_file();

    let port: u16 = std::env::var("AM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8383);
    let username = std::env::var("AM_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("AM_PASSWORD").unwrap_or_else(|_| "admin123".into());

    let data_dir = std::env::var("AM_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .expect("无法定位用户目录")
                .join(".agent-monitor")
        });
    let _ = std::fs::create_dir_all(&data_dir);

    // 登录加密的 AES 密钥：与前端 .env 的 CRYPTO_KEY 必须逐字节相同，
    // 不同则登录在浏览器里就失败、连请求都发不出去。不设则首启随机生成并持久化。
    let crypto_key = match std::env::var("AM_CRYPTO_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    {
        Some(k) => k,
        None => {
            let (k, fresh) = persisted_value(&data_dir.join("crypto-key"), random_token);
            if fresh {
                tracing::info!(
                    "已生成登录加密 AES 密钥: {k}\n请填入前端 .env 的 CRYPTO_KEY 后重新构建前端，否则登录会「无请求即失败」。（持久化于 {}/crypto-key）",
                    data_dir.display()
                );
            } else {
                tracing::info!(
                    "登录加密 AES 密钥已就绪（读取自 {}/crypto-key）",
                    data_dir.display()
                );
            }
            k
        }
    };

    // 登录用的 RSA 私钥：**运行时**读文件，绝不编译期嵌入 ——
    // 嵌进二进制的私钥会随任何一份产物外泄（构建产物误提交即等于私钥公开）。
    // 未指定 AM_RSA_KEY_PATH 时落在数据目录；文件不存在就现生成一对并以 0600 落盘。
    let key_path = std::env::var("AM_RSA_KEY_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("rsa_private.pem"));
    let private_key = load_or_create_rsa_key(&key_path)?;

    let reg = registry::Registry::load(data_dir.clone(), &username, &password);

    // 后管访问令牌：部署（首次启动）时生成并持久化，可用 AM_ADMIN_TOKEN 覆盖
    let admin_token = match std::env::var("AM_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        Some(t) => t,
        None => {
            let (t, fresh) = persisted_value(&data_dir.join("admin-token"), random_token);
            if fresh {
                // 只在首次部署生成时打印一次；后续启动只提示存放位置。
                tracing::info!(
                    "已生成后管访问令牌（X-Admin-Token）: {t}\n请立即保存，此令牌只打印这一次。（持久化于 {}/admin-token）",
                    data_dir.display()
                );
            } else {
                tracing::info!(
                    "后管访问令牌已就绪（读取自 {}/admin-token）",
                    data_dir.display()
                );
            }
            t
        }
    };

    // 全局 agent 上报令牌：内部部署/无界面 agent 的兼容通道；普通用户走每设备令牌。
    let agent_token = match std::env::var("AM_AGENT_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        Some(t) => t,
        None => {
            let (t, fresh) = persisted_value(&data_dir.join("agent-token"), random_token);
            if fresh {
                tracing::info!(
                    "已生成 agent 上报令牌（X-Agent-Token）: {t}\n请立即保存，此令牌只打印这一次；内部 agent 需以 AM_AGENT_TOKEN 配置相同值。（持久化于 {}/agent-token）",
                    data_dir.display()
                );
            } else {
                tracing::info!(
                    "agent 上报令牌已就绪（读取自 {}/agent-token）",
                    data_dir.display()
                );
            }
            t
        }
    };

    let config = Config {
        port,
        crypto_key,
        private_key,
        data_dir,
        admin_token,
        agent_token,
    };
    let state = AppState::new(config, reg);

    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    rt.block_on(async move {
        if let Err(e) = run_hub(state).await {
            tracing::error!("hub 服务退出: {e}");
            std::process::exit(1);
        }
    });
    Ok(())
}

/// hub 模式：聚合 + HTTP/WS 服务（含前端静态托管与安装包下载）
async fn run_hub(state: SharedState) -> Result<()> {
    let port = state.config.port;
    tokio::spawn(state::tick_loop(state.clone()));
    // 钉钉 Stream 长连接管理器：为配了 AppKey+AppSecret 的用户维持收消息长连接
    tokio::spawn(dingtalk_stream::run(state.clone()));
    // 机器人「监控 N」推送循环：把被监控会话的新内容推到钉钉会话
    tokio::spawn(bot::monitor_loop(state.clone()));

    let app = server::router(state);
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("agent-task-monitor (hub) 启动: http://{addr}  （WS: /monitor/ws）");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// 从配置文件补齐缺失的 AM_* 环境变量（env 已设的优先，不覆盖）。
/// 查找顺序：AM_CONFIG 指定 > 可执行文件同级 config.txt > ~/.agent-monitor/config.txt。
/// 文件格式：每行 `KEY=VALUE`，# 开头为注释。
fn load_config_file() {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("AM_CONFIG") {
        candidates.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("config.txt"));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".agent-monitor").join("config.txt"));
    }

    let Some(text) = candidates
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok())
    else {
        return;
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim().trim_matches('"');
        // env 已存在的不覆盖（命令行/systemd 显式设置优先）
        if !key.is_empty() && std::env::var_os(key).is_none() {
            std::env::set_var(key, val);
        }
    }
}

/// 读取 PKCS#8 PEM 私钥；文件不存在则现生成一对 2048 位密钥并落盘（unix 下 0600），
/// 同时打印配对公钥的 base64 —— 那正是前端 `.env` 里 `RSA_PUB_KEY` 要填的值。
fn load_or_create_rsa_key(path: &std::path::Path) -> Result<RsaPrivateKey> {
    if path.exists() {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("读取 RSA 私钥 {}", path.display()))?;
        return RsaPrivateKey::from_pkcs8_pem(&pem)
            .with_context(|| format!("解析 RSA 私钥 {}（须为 PKCS#8 PEM）", path.display()));
    }

    let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).context("生成 RSA 私钥")?;
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .context("序列化 RSA 私钥为 PKCS#8 PEM")?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, pem.as_bytes())
        .with_context(|| format!("写入 RSA 私钥 {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    let pub_b64 = B64.encode(
        RsaPublicKey::from(&key)
            .to_public_key_der()
            .context("序列化 RSA 公钥")?
            .as_bytes(),
    );
    tracing::info!(
        "已生成 RSA 密钥对（{}）。配对公钥（填入前端 .env 的 RSA_PUB_KEY 后重新构建前端）：\n{pub_b64}",
        path.display()
    );
    Ok(key)
}

/// 生成 32 位随机令牌
fn random_token() -> String {
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// 读取持久化的秘密值；不存在则用 init 生成并写入（unix 下文件权限 0600）。
/// 返回 (值, 是否本次新生成) —— 秘密只在首次生成时打印一次。
fn persisted_value(path: &std::path::Path, init: impl FnOnce() -> String) -> (String, bool) {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return (t, false);
        }
    }
    let value = init();
    let _ = std::fs::write(path, &value);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    (value, true)
}
