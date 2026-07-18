//! Agent Task Monitor —— 终端 AI 代理任务监控客户端（Tauri 桌面应用）。
//! 服务在后台线程运行；主线程承载 Tauri 窗口 + 托盘（AM_HEADLESS=1 或无 desktop 特性时回退无界面）。
#![cfg_attr(all(windows, feature = "desktop"), windows_subsystem = "windows")]

mod admin;
mod agent;
mod commands;
mod crypto;
mod gitdiff;
mod model;
mod oauth;
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

    // GUI 从 Finder/开机自启启动时没有 shell 环境变量，
    // 这里从配置文件补齐（env 优先，配置文件兜底），使 .app / .exe 免启动器即可运行。
    load_config_file();

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

    // machine_id：AM_MACHINE_ID > 数据目录持久化（首次生成后不再变）
    let machine_id = std::env::var("AM_MACHINE_ID").unwrap_or_else(|_| {
        // machine_id 不是秘密，无需区分是否新生成
        persisted_value(&data_dir.join("machine-id"), || new_machine_id(&raw_hostname)).0
    });

    let mut reg = registry::Registry::load(data_dir.clone(), &username, &password);
    // hub 本机默认信任并归属超级管理员（种子用户）
    reg.ensure_device(&machine_id, Some(&username), true);

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

    // agent 上报令牌：hub 与 agent 共享，防伪造上报/窃取命令队列。
    // hub 侧首启生成；agent 模式必须通过 AM_AGENT_TOKEN 提供与 hub 相同的值。
    let agent_token = match std::env::var("AM_AGENT_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        Some(t) => t,
        None => {
            let (t, fresh) = persisted_value(&data_dir.join("agent-token"), random_token);
            if fresh {
                tracing::info!(
                    "已生成 agent 上报令牌（X-Agent-Token）: {t}\n请立即保存，此令牌只打印这一次；远程 agent 需以 AM_AGENT_TOKEN 配置相同值。（持久化于 {}/agent-token）",
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
        machine_id,
        hostname,
        platform,
        data_dir,
        admin_token,
        agent_token,
    };
    let state = AppState::new(config, SessionScanner::new(projects_dir), reg);

    // 已配对过的设备：加载持久化的每设备上报令牌（agent 模式凭它上报，无需全局令牌）
    if let Ok(t) = std::fs::read_to_string(state.config.data_dir.join("device-token")) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            *state.device_token.blocking_write() = Some(t);
        }
    }

    let hub_url = std::env::var("AM_HUB_URL").ok().filter(|s| !s.trim().is_empty())
        // 单文件分发的客户端开箱即用：无任何配置时连编译期内置的默认 hub
        .or_else(|| option_env!("AM_DEFAULT_HUB_URL").map(str::to_string));
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

    // agent 模式且未绑定：窗口创建前先领一个配对码（快速尝试，失败不阻塞——
    // 服务线程会持续重试，托盘也会给出指引）。这样首窗即可带 ?pair= 引导绑定。
    #[cfg(feature = "desktop")]
    if is_agent
        && state.device_token.blocking_read().is_none()
        && std::env::var("AM_AGENT_TOKEN").ok().filter(|s| !s.is_empty()).is_none()
    {
        if let Some(hub) = &hub_url {
            let hub = hub.trim_end_matches('/').to_string();
            let st = state.clone();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
            if let Ok(rt) = rt {
                rt.block_on(async {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(3))
                        .build();
                    if let Ok(client) = client {
                        let body = serde_json::json!({
                            "machineId": st.config.machine_id,
                            "hostname": st.config.hostname,
                            "platform": st.config.platform,
                        });
                        if let Ok(resp) =
                            client.post(format!("{hub}/monitor/pair/start")).json(&body).send().await
                        {
                            if let Ok(v) = resp.json::<serde_json::Value>().await {
                                if let (Some(code), Some(pt)) = (
                                    v.pointer("/data/code").and_then(serde_json::Value::as_str),
                                    v.pointer("/data/pairToken").and_then(serde_json::Value::as_str),
                                ) {
                                    *st.pair_info.write().await =
                                        Some((code.to_string(), pt.to_string()));
                                }
                            }
                        }
                    }
                });
            }
        }
    }

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
    // Windows：GUI 子系统程序拉起控制台命令会闪黑窗，直接读环境变量即可
    #[cfg(windows)]
    {
        if let Ok(n) = std::env::var("COMPUTERNAME") {
            let n = n.trim().to_string();
            if !n.is_empty() {
                return n;
            }
        }
        return "unknown".into();
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    }
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

/// 从配置文件补齐缺失的 AM_* 环境变量（env 已设的优先，不覆盖）。
/// 查找顺序：AM_CONFIG 指定 > 可执行文件同级 config.txt > macOS .app 的
/// Contents/Resources/config.txt > ~/.agent-monitor/config.txt。
/// 文件格式：每行 `KEY=VALUE`，# 开头为注释。
fn load_config_file() {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("AM_CONFIG") {
        candidates.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("config.txt"));
            // macOS .app：MacOS/ 同级找不到时去 ../Resources/
            candidates.push(dir.join("../Resources/config.txt"));
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

fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
}

/// 首次生成本机的 machine_id：可读的主机名 + 随机后缀。
///
/// 不能只用主机名：sanitize 会把非 ASCII 全换成 '-'，中文机器名（"小明的MacBook"）
/// 会被压成一串横线；macOS 的 LocalHostName 本就常是 `MacBook-Pro`，DHCP 下还可能
/// 都叫 `bogon`。两台机器撞到同一个 id 时，hub 以 machine_id 为 key 存机器，
/// 后者会覆盖前者的会话快照，而设备归属只在 owner 为空时认领 —— 结果就是
/// 别人的会话挂到你名下，还能被你暂停 / 注入输入。加随机后缀即可根治。
/// （生成后写入数据目录，之后不再变；已有安装读到旧值，不受影响。）
fn new_machine_id(hostname: &str) -> String {
    let base = sanitize_id(hostname);
    let base = base.trim_matches('-');
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    if base.is_empty() {
        format!("host-{suffix}")
    } else {
        format!("{base}-{suffix}")
    }
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
/// 读取持久化的秘密值；不存在则生成。
/// 返回 (值, 是否本次新生成) —— 调用方据此决定是否打日志：
/// 秘密只在首次生成时打印一次，之后每次启动都打会把它长期留在
/// journald/日志文件里，任何能读日志的人都能拿到后管访问权。
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
