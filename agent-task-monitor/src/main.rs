//! Agent Task Monitor —— 终端 AI 代理任务监控客户端（Tauri 桌面应用）。
//! 服务在后台线程运行；主线程承载 Tauri 窗口 + 托盘（AM_HEADLESS=1 或无 desktop 特性时回退无界面）。
#![cfg_attr(all(windows, feature = "desktop"), windows_subsystem = "windows")]

mod admin;
mod agent;
mod commands;
mod crypto;
mod model;
mod process;
mod registry;
mod scanner;
mod server;
mod state;
#[cfg(feature = "desktop")]
mod desktop;

use anyhow::{Context, Result};
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use scanner::SessionScanner;
use state::{AppState, Config, SharedState};

/// 开发默认私钥（与前端 .env 中 RSA_PUB_KEY 配对）。
/// 生产使用请通过 AM_RSA_KEY_PATH 指定自己的密钥。
const DEV_PRIVATE_KEY: &str = include_str!("../keys/rsa_private.pem");
const DEV_CRYPTO_KEY: &str = "VitaClaudeMonitorAesKey123456789";

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "agent_task_monitor=info,info".into()),
        )
        .init();

    let port: u16 = std::env::var("AM_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8383);
    let username = std::env::var("AM_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("AM_PASSWORD").unwrap_or_else(|_| "admin123".into());
    let crypto_key = std::env::var("AM_CRYPTO_KEY").unwrap_or_else(|_| DEV_CRYPTO_KEY.into());

    let private_key = match std::env::var("AM_RSA_KEY_PATH") {
        Ok(path) => {
            let pem = std::fs::read_to_string(&path).with_context(|| format!("读取私钥 {path}"))?;
            RsaPrivateKey::from_pkcs8_pem(&pem).context("解析 RSA 私钥（须为 PKCS#8 PEM）")?
        }
        Err(_) => RsaPrivateKey::from_pkcs8_pem(DEV_PRIVATE_KEY).context("解析内置私钥")?,
    };

    let projects_dir = std::env::var("AM_CLAUDE_PROJECTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .expect("无法定位用户目录")
                .join(".claude")
                .join("projects")
        });
    if !projects_dir.is_dir() {
        tracing::warn!(
            "Claude Code 会话目录不存在: {}（本机可能尚未使用过 Claude Code）",
            projects_dir.display()
        );
    }

    // machine_id：优先 AM_MACHINE_ID，其次数据目录持久化（首次由 hostname 派生），
    // 保证用户改电脑名后设备信任关系不丢；展示名优先用户给电脑设的名称
    let raw_hostname = hostname();
    let hostname = device_name().unwrap_or_else(|| raw_hostname.clone());
    let platform = std::env::consts::OS.to_string();

    // 用户 + 设备信任注册表（持久化到 ~/.agent-monitor/）
    let data_dir = std::env::var("AM_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .expect("无法定位用户目录")
                .join(".agent-monitor")
        });
    let _ = std::fs::create_dir_all(&data_dir);

    // machine_id：AM_MACHINE_ID > 数据目录持久化（首次由 hostname 派生）
    let machine_id = std::env::var("AM_MACHINE_ID").unwrap_or_else(|_| {
        persisted_value(&data_dir.join("machine-id"), || sanitize_id(&raw_hostname))
    });

    let mut reg = registry::Registry::load(data_dir.clone(), &username, &password);
    // hub 本机默认信任并归属超级管理员（种子用户）
    reg.ensure_device(&machine_id, Some(&username), true);

    // 后管访问令牌：部署（首次启动）时生成并持久化，可用 AM_ADMIN_TOKEN 覆盖
    let admin_token = std::env::var("AM_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| persisted_value(&data_dir.join("admin-token"), random_token));
    tracing::info!(
        "后管访问令牌（X-Admin-Token）: {admin_token}（持久化于 {}/admin-token）",
        data_dir.display()
    );

    // agent 上报令牌：hub 与 agent 共享，防伪造上报/窃取命令队列。
    // hub 侧首启生成；agent 模式必须通过 AM_AGENT_TOKEN 提供与 hub 相同的值。
    let agent_token = std::env::var("AM_AGENT_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| persisted_value(&data_dir.join("agent-token"), random_token));
    tracing::info!(
        "agent 上报令牌（X-Agent-Token）: {agent_token}（持久化于 {}/agent-token，远程 agent 需以 AM_AGENT_TOKEN 配置相同值）",
        data_dir.display()
    );

    let config = Config {
        port,
        username,
        password,
        crypto_key,
        private_key,
        machine_id,
        hostname,
        platform,
        data_dir,
        admin_token,
        agent_token,
    };
    let state = AppState::new(config, SessionScanner::new(projects_dir), reg);

    let hub_url = std::env::var("AM_HUB_URL").ok();
    let is_agent = hub_url.is_some();

    // 服务在后台线程的 tokio runtime 中运行
    let svc_state = state.clone();
    let svc_hub = hub_url.clone();
    let svc_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(async move {
            if let Some(hub) = svc_hub {
                tracing::info!(
                    "以 agent 模式运行：machine={} → hub {hub}",
                    svc_state.config.machine_id
                );
                agent::report_loop(svc_state, hub).await;
            } else if let Err(e) = run_hub(svc_state).await {
                tracing::error!("hub 服务退出: {e}");
                std::process::exit(1);
            }
        });
    });

    let web_base = if let Some(hub) = &hub_url {
        hub.trim_end_matches('/').to_string()
    } else {
        format!("http://localhost:{port}")
    };

    // 主线程：Tauri 桌面窗口 + 托盘（AM_HEADLESS=1 关闭，用于服务器/纯 agent）
    let headless = std::env::var("AM_HEADLESS").map(|v| v == "1").unwrap_or(false)
        || std::env::var("AM_NO_TRAY").map(|v| v == "1").unwrap_or(false);

    #[cfg(feature = "desktop")]
    if !headless {
        // 等待本机 hub 端口就绪，避免窗口先于服务加载失败
        if !is_agent {
            wait_port_ready(port);
        }
        desktop::run(state.clone(), desktop::DesktopConfig { web_base, is_agent })?;
        return Ok(());
    }
    #[cfg(not(feature = "desktop"))]
    let _ = (is_agent, &web_base);

    // 无界面模式：仅跑服务
    if headless {
        tracing::info!("无界面模式（AM_HEADLESS）：仅运行后台服务");
    }
    svc_thread.join().ok();
    Ok(())
}

/// 轮询等待本机端口进入可连接状态（最多 ~5s）
fn wait_port_ready(port: u16) {
    use std::net::TcpStream;
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..100 {
        if TcpStream::connect(&addr).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// hub 模式：本地扫描 + 聚合 + HTTP/WS 服务（含前端静态托管）
async fn run_hub(state: SharedState) -> Result<()> {
    let port = state.config.port;
    tokio::spawn(state::scan_loop(state.clone()));

    let app = server::router(state);
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("agent-task-monitor (hub) 启动: http://{addr}  （WS: /monitor/ws）");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// 用户给电脑设置的设备名（macOS「关于本机」的名称 / Linux PRETTY_HOSTNAME），
/// 取不到时回退 None（调用方回退原始 hostname）。Windows 的 hostname 即用户设的计算机名。
fn device_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("hostnamectl")
            .arg("--pretty")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
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

/// 读取持久化值；不存在则用 init 生成并写入（unix 下文件权限 0600）
fn persisted_value(path: &std::path::Path, init: impl FnOnce() -> String) -> String {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let value = init();
    let _ = std::fs::write(path, &value);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    value
}
