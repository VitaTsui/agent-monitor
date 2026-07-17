//! Tauri v2 桌面外壳：ToDesk 式设备管理器窗口 + 系统托盘。
//! 窗口加载本地/远端 hub 托管的前台页面（设备树 → 选中设备看终端会话 + 设备管理）。
//! 服务在后台线程运行；本模块在主线程跑 Tauri 事件循环。
#![cfg(feature = "desktop")]

use crate::state::SharedState;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub struct DesktopConfig {
    /// 前台基础地址（hub 本机 http://localhost:port；agent 模式为远端 hub 地址）
    pub web_base: String,
    pub is_agent: bool,
}

/// 运行 Tauri 桌面应用（阻塞，不返回）。
pub fn run(state: SharedState, cfg: DesktopConfig) -> anyhow::Result<()> {
    let portal_url = format!("{}/portal", cfg.web_base);
    let web_base = cfg.web_base.clone();
    let is_agent = cfg.is_agent;
    let state_setup = state.clone();

    tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();

            // macOS：设为 Accessory —— 只驻留菜单栏托盘，不在 Dock 显示。
            // （LSUIElement 会被 Tauri 创建窗口时覆盖，必须代码里显式设置）
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 主窗口：加载前台（ToDesk 式设备/终端管理）
            let url: tauri::Url = portal_url.parse().expect("非法前台地址");
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("终端任务监控 · 设备与终端")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                .build()?;

            // 关闭窗口 = 隐藏到托盘（不退出），保持后台上报；退出走托盘「退出」菜单
            let win_close = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = win_close.hide();
                }
            });

            // agent 模式：启动即隐藏窗口，纯后台上报 + 托盘常驻（点托盘图标再打开窗口）
            if is_agent {
                let _ = win.hide();
            }

            // 托盘菜单
            let menu = build_tray_menu(&handle, &state_setup, is_agent)?;

            let web_base_menu = web_base.clone();
            let state_evt = state_setup.clone();
            let tray = TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip(if is_agent {
                    "终端任务监控（Agent）"
                } else {
                    "终端任务监控（Hub）"
                })
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let id = event.id.as_ref();
                    match id {
                        "show" => show_main(app),
                        "browser" => open_external(&format!("{web_base_menu}/portal")),
                        "autostart" => set_autostart(!autostart_enabled()),
                        "quit" => app.exit(0),
                        other => {
                            // 监控范围勾选项：id=excl::<tty>
                            if let Some(tty) = other.strip_prefix("excl::") {
                                let tty = tty.to_string();
                                let st = state_evt.clone();
                                tauri::async_runtime::spawn(async move {
                                    // 必须读真实排除状态再取反，不能依赖菜单项的勾选态：
                                    // 菜单是每 3 秒重建的快照，点击时那份可能已经过期。
                                    let now = st.excludes.read().await.is_excluded(&tty);
                                    st.excludes.write().await.set(&tty, !now);
                                });
                            }
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            // 后台线程定期重建托盘菜单，反映实时连接状态、终端列表与自启状态
            let handle_bg = handle.clone();
            let state_bg = state_setup.clone();
            std::thread::spawn(move || {
                let mut last_sig = String::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let (terminals, excluded, hub_err) = tauri::async_runtime::block_on(async {
                        let t = state_bg.terminals.read().await.clone();
                        let e = state_bg.excludes.read().await.list();
                        let err = state_bg.hub_error.read().await.clone();
                        (t, e, err)
                    });
                    let connected = state_bg
                        .hub_connected
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let dev_trusted = state_bg
                        .hub_trusted
                        .load(std::sync::atomic::Ordering::Relaxed);
                    // 状态变化才重建菜单，避免高频刷新。
                    // hub_err 必须进签名：否则上报被拒时状态行不会刷新出错误原因。
                    let sig = format!(
                        "{:?}|{:?}|{connected}|{dev_trusted}|{hub_err:?}|{}",
                        terminals,
                        excluded,
                        autostart_enabled()
                    );
                    if sig == last_sig {
                        continue;
                    }
                    last_sig = sig;
                    if let Ok(menu) = build_tray_menu(&handle_bg, &state_bg, is_agent) {
                        if let Some(tray) = handle_bg.tray_by_id("main") {
                            let _ = tray.set_menu(Some(menu));
                            let tip = if is_agent {
                                if connected {
                                    if dev_trusted {
                                        "终端任务监控 · 已连接（已信任）".to_string()
                                    } else {
                                        "终端任务监控 · 已连接（待信任）".to_string()
                                    }
                                } else {
                                    // 同状态行：配置错误要说清楚，别和断网混为一谈
                                    match &hub_err {
                                        Some(msg) => format!("终端任务监控 · {msg}"),
                                        None => "终端任务监控 · 连接中…".to_string(),
                                    }
                                }
                            } else {
                                "终端任务监控（Hub）".to_string()
                            };
                            let _ = tray.set_tooltip(Some(&tip));
                        }
                    }
                }
            });

            let _ = tray;
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri 运行失败: {e}"))?;
    Ok(())
}

/// 构建完整托盘菜单：连接状态 → 显示窗口/浏览器打开 → 开机自启 → 监控范围 → 退出
fn build_tray_menu<R: tauri::Runtime>(
    manager: &impl Manager<R>,
    state: &SharedState,
    is_agent: bool,
) -> tauri::Result<Menu<R>> {
    let sep = || PredefinedMenuItem::separator(manager);
    let show = MenuItem::with_id(manager, "show", "显示窗口", true, None::<&str>)?;
    let browser = MenuItem::with_id(manager, "browser", "在浏览器打开", true, None::<&str>)?;
    let scope = build_scope_submenu(manager, state)?;
    let autostart = CheckMenuItem::with_id(
        manager,
        "autostart",
        "开机自启",
        true,
        autostart_enabled(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(manager, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::new(manager)?;
    // agent 模式顶部显示一行连接状态（禁用项，仅展示）
    if is_agent {
        let connected = state
            .hub_connected
            .load(std::sync::atomic::Ordering::Relaxed);
        let trusted = state.hub_trusted.load(std::sync::atomic::Ordering::Relaxed);
        let err = tauri::async_runtime::block_on(async { state.hub_error.read().await.clone() });
        let label = if connected {
            if trusted {
                "● 已连接 · 已信任".to_string()
            } else {
                "● 已连接 · 待信任（去网页信任本设备）".to_string()
            }
        } else {
            // 被 hub 拒绝（令牌错、machineId 冲突…）与网络不通要分开说：
            // 前者重试再多次也不会自愈，必须让用户看到该改哪里。
            match err {
                Some(msg) => format!("✕ {msg}"),
                None => "○ 连接中…".to_string(),
            }
        };
        let status = MenuItem::new(manager, &label, false, None::<&str>)?;
        menu.append(&status)?;
        menu.append(&sep()?)?;
    }
    menu.append(&show)?;
    menu.append(&browser)?;
    menu.append(&sep()?)?;
    menu.append(&autostart)?;
    menu.append(&sep()?)?;
    menu.append(&scope)?;
    menu.append(&sep()?)?;
    menu.append(&quit)?;
    Ok(menu)
}

/// 构建「监控范围（勾选=不监控该终端）」子菜单
fn build_scope_submenu<R: tauri::Runtime>(
    manager: &impl Manager<R>,
    state: &SharedState,
) -> tauri::Result<Submenu<R>> {
    let (terminals, excluded) = tauri::async_runtime::block_on(async {
        let t = state.terminals.read().await.clone();
        let e: std::collections::HashSet<String> =
            state.excludes.read().await.list().into_iter().collect();
        (t, e)
    });

    let submenu = Submenu::new(manager, "监控范围（勾选=不监控）", true)?;
    if terminals.is_empty() {
        let empty = MenuItem::new(manager, "（暂无检测到的终端）", false, None::<&str>)?;
        submenu.append(&empty)?;
    } else {
        for (tty, label) in &terminals {
            let id = format!("excl::{tty}");
            let checked = excluded.contains(tty);
            let item = CheckMenuItem::with_id(
                manager,
                &id,
                format!("{label}  [{tty}]"),
                true,
                checked,
                None::<&str>,
            )?;
            submenu.append(&item)?;
        }
    }
    Ok(submenu)
}

// ---------- 开机自启（跨平台） ----------

/// 当前应用的启动目标：macOS 用 .app 包路径（`open -a`），其它用可执行文件路径。
/// 开机自启要拉起的目标：一律用当前可执行文件的绝对路径。
///
/// macOS 上不取 .app 目录：那需要配合 `open -a`，而 `open -a` 只认应用包，
/// 裸二进制形态会静默失败。直接指向可执行文件对两种形态都成立。
fn autostart_target() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Option<std::path::PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/LaunchAgents/com.vitahsu.agentmonitor.plist"),
    )
}

/// Windows 上起 `reg` 这类控制台程序的统一入口。
///
/// 本程序是 GUI 子系统（见 main.rs 的 windows_subsystem="windows"），自身没有控制台，
/// 拉起控制台程序时系统会新分配一个 → 黑窗一闪。托盘每 3 秒就要查一次自启状态，
/// 不加 CREATE_NO_WINDOW 就是一直闪。
#[cfg(target_os = "windows")]
fn reg_command(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

fn autostart_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        // 光看 plist 在不在是不够的：里面记的是「当初启用时」那份程序的路径。
        // 用户从下载目录直接双击运行、启用自启，之后把 .app 挪进「应用程序」
        // （或清理了下载目录），plist 就指向一个不存在的路径 —— 自启早已失效，
        // 菜单却还打着勾。这里核对记录的路径是否仍是当前这份程序。
        let Some(plist) = launch_agent_plist_path() else {
            return false;
        };
        let Ok(txt) = std::fs::read_to_string(&plist) else {
            return false;
        };
        match autostart_target() {
            Some(exe) => txt.contains(&exe.display().to_string()),
            None => false,
        }
    }
    #[cfg(target_os = "windows")]
    {
        reg_command(&[
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "AgentMonitor",
        ])
        .map(|o| o.status.success())
        .unwrap_or(false)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        false
    }
}

fn set_autostart(enable: bool) {
    let Some(target) = autostart_target() else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        let Some(plist) = launch_agent_plist_path() else {
            return;
        };
        if enable {
            // 直接执行二进制，不走 `open -a`：
            // `open -a` 只认应用包/应用名，裸二进制形态（cargo run、未打包分发）
            // 必然失败且无任何提示。直接执行对两种形态都成立 —— 位于 .app 内的
            // 可执行文件被直接拉起时，macOS 仍会从所在包读取 Info.plist（LSUIElement 照常生效）。
            let content = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.vitahsu.agentmonitor</string>
  <key>ProgramArguments</key>
  <array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>"#,
                target.display()
            );
            if let Some(dir) = plist.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&plist, content);
            let _ = std::process::Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&plist)
                .output();
        } else {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(&plist)
                .output();
            let _ = std::fs::remove_file(&plist);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if enable {
            // 路径必须带引号写入：Run 项的值不加引号时，含空格的路径
            // （C:\Users\John Smith\…）会被 Windows 从空格处截断，开机启动失败，
            // 而 autostart_enabled 只看键存不存在 → 菜单照样打勾，静默失效。
            let quoted = format!("\"{}\"", target.display());
            let _ = reg_command(&[
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "AgentMonitor",
                "/t",
                "REG_SZ",
                "/d",
                &quoted,
                "/f",
            ]);
        } else {
            let _ = reg_command(&[
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "AgentMonitor",
                "/f",
            ]);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = (enable, target);
    }
}

fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn open_external(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        tracing::warn!("打开浏览器失败: {e}");
    }
}
