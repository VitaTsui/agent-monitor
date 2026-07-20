//! am-client —— 终端任务监控桌面客户端（Mac / Windows，一套代码双平台打包）。
//! 本机扫描 AI 代理会话 → 上报 hub；Tauri 窗口 + 托盘（AM_HEADLESS=1 时无界面）。
#![cfg_attr(all(windows, feature = "desktop"), windows_subsystem = "windows")]

mod agent;
mod secrets;
mod openfiles;
mod state;
#[cfg(feature = "desktop")]
mod desktop;

use anyhow::Result;
use state::{AppState, Config};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "agent_monitor=info,info".into()),
        )
        .init();

    // GUI 从 Finder/开机自启启动时没有 shell 环境变量，
    // 这里从配置文件补齐（env 优先，配置文件兜底），使 .app / .exe 免启动器即可运行。
    load_config_file();

    // machine_id：优先 AM_MACHINE_ID，其次数据目录持久化（首次由 hostname 派生），
    // 保证用户改电脑名后设备信任关系不丢；展示名优先用户给电脑设的名称
    let raw_hostname = hostname();
    let hostname = device_name().unwrap_or_else(|| raw_hostname.clone());
    let platform = std::env::consts::OS.to_string();

    // 数据目录用平台规范位置（mac ~/Library/Application Support、
    // Windows %APPDATA%），不放安装目录：安装目录随自更新整体替换，
    // 数据放里面每次更新即丢（设备绑定要重来）；mac 往 .app 包内写文件
    // 还会破坏签名。旧版 ~/.agent-monitor 自动整体迁移。
    let data_dir = std::env::var("AM_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| default_data_dir());
    migrate_legacy_data_dir(&data_dir);
    let _ = std::fs::create_dir_all(&data_dir);

    // Windows GUI 子系统没有控制台：panic 会无声消失，用户只觉得「双击没反应」。
    // 落崩溃日志 + 弹系统对话框；另记启动阶段面包屑，出问题能定位到哪一步。
    #[cfg(all(windows, feature = "desktop"))]
    {
        let crash = data_dir.join("crash.log");
        std::panic::set_hook(Box::new(move |info| {
            let msg = format!("{info}");
            let _ = std::fs::write(&crash, &msg);
            crate::desktop::message_box("终端任务监控 · 崩溃", &format!(
                "程序遇到错误已退出：\n{msg}\n\n日志：{}", crash.display()));
        }));
    }
    let breadcrumb = {
        let path = data_dir.join("startup.log");
        let _ = std::fs::write(&path, "start\n");
        move |step: &str| {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) {
                let _ = writeln!(f, "{step}");
            }
        }
    };
    breadcrumb("data_dir ok");

    // machine_id：AM_MACHINE_ID > 数据目录持久化（首次生成后不再变）
    let machine_id = std::env::var("AM_MACHINE_ID").unwrap_or_else(|_| {
        // machine_id 不是秘密，无需区分是否新生成
        persisted_value(&data_dir.join("machine-id"), || new_machine_id(&raw_hostname))
    });

    let config = Config {
        machine_id,
        hostname,
        platform,
        data_dir,
    };
    let state = AppState::new(config);

    // 已配对过的设备：从系统安全存储加载每设备上报令牌
    // （mac 钥匙串 / Windows DPAPI；旧版明文文件自动迁移进安全存储）
    if let Some(t) = secrets::load(&state.config.data_dir) {
        *state.device_token.blocking_write() = Some(t);
    }

    // hub 地址：AM_HUB_URL > 编译期内置默认（官网分发的安装包开箱即用）
    let hub_url = std::env::var("AM_HUB_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| option_env!("AM_DEFAULT_HUB_URL").map(str::to_string));
    let Some(hub_url) = hub_url else {
        let msg = "未配置 hub 地址：请在 config.txt 或环境变量中设置 AM_HUB_URL，\
                   或使用官网分发的安装包（已内置官方地址）。";
        tracing::error!("{msg}");
        #[cfg(all(windows, feature = "desktop"))]
        desktop::message_box("终端任务监控", msg);
        anyhow::bail!("{msg}");
    };
    let hub_url = hub_url.trim_end_matches('/').to_string();

    // 诊断入口：AM_SELF_UPDATE=1 直接跑一遍自更新流程并打印结果（支持排查用）
    #[cfg(feature = "desktop")]
    if std::env::var("AM_SELF_UPDATE").ok().as_deref() == Some("1") {
        desktop::self_update_probe(&hub_url);
    }

    // 上报服务在后台线程的 tokio runtime 中运行（前台/后台/缩到托盘均持续同步）
    let svc_state = state.clone();
    let svc_hub = hub_url.clone();
    let svc_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
        rt.block_on(async move {
            tracing::info!(
                "客户端运行：machine={} → hub {svc_hub}",
                svc_state.config.machine_id
            );
            agent::report_loop(svc_state, svc_hub).await;
        });
    });
    breadcrumb("service thread spawned");

    // 未绑定：窗口创建前先领一个配对码（快速尝试，失败不阻塞——
    // 服务线程会持续重试，托盘也会给出指引）。这样首窗即可带 ?pair= 引导绑定。
    #[cfg(feature = "desktop")]
    if state.device_token.blocking_read().is_none()
        && std::env::var("AM_AGENT_TOKEN").ok().filter(|s| !s.is_empty()).is_none()
    {
        let st = state.clone();
        let hub = hub_url.clone();
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

    // 主线程：Tauri 桌面窗口 + 托盘（AM_HEADLESS=1 关闭，用于无界面 agent）
    let headless = std::env::var("AM_HEADLESS").map(|v| v == "1").unwrap_or(false)
        || std::env::var("AM_NO_TRAY").map(|v| v == "1").unwrap_or(false);

    #[cfg(feature = "desktop")]
    if !headless {
        breadcrumb("desktop::run");
        desktop::run(
            state.clone(),
            desktop::DesktopConfig { web_base: hub_url, is_agent: true },
        )?;
        return Ok(());
    }

    if headless {
        tracing::info!("无界面模式（AM_HEADLESS）：仅运行后台上报");
    }
    svc_thread.join().ok();
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
        // env 已存在的不覆盖（命令行显式设置优先）
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

/// 读取持久化值；不存在则用 init 生成并写入
fn persisted_value(path: &std::path::Path, init: impl FnOnce() -> String) -> String {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let value = init();
    let _ = std::fs::write(path, &value);
    value
}

/// 平台规范数据目录：mac ~/Library/Application Support/AgentMonitor、
/// Windows %APPDATA%\AgentMonitor、Linux ~/.local/share/AgentMonitor
fn default_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().expect("无法定位用户目录"))
        .join("AgentMonitor")
}

/// 旧版数据目录（~/.agent-monitor）整体迁移到新位置，保住设备绑定等状态
fn migrate_legacy_data_dir(new_dir: &std::path::Path) {
    let Some(home) = dirs::home_dir() else { return };
    let old = home.join(".agent-monitor");
    if !old.is_dir() || new_dir.exists() {
        return;
    }
    if let Some(parent) = new_dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(&old, new_dir) {
        Ok(_) => tracing::info!("数据目录已迁移: {} → {}", old.display(), new_dir.display()),
        Err(e) => tracing::warn!("数据目录迁移失败（继续用旧目录需设 AM_DATA_DIR）: {e}"),
    }
}
