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

// ---------- 关闭行为（退出 or 最小化到托盘）持久化 ----------

/// 关闭窗口时的行为。持久化在数据目录，托盘菜单里可切换。
#[derive(Clone, Copy, PartialEq)]
enum CloseBehavior {
    /// 缩小到系统托盘，程序留在后台继续运行
    Tray,
    /// 直接退出整个应用
    Quit,
}

fn close_pref_path(state: &SharedState) -> std::path::PathBuf {
    state.config.data_dir.join("close-behavior")
}

/// 读取关闭行为；未设置过时默认「最小化到托盘」（后台继续运行，最安全）。
fn read_close_behavior(state: &SharedState) -> CloseBehavior {
    match std::fs::read_to_string(close_pref_path(state)) {
        Ok(s) if s.trim() == "quit" => CloseBehavior::Quit,
        _ => CloseBehavior::Tray,
    }
}

fn write_close_behavior(state: &SharedState, b: CloseBehavior) {
    let v = match b {
        CloseBehavior::Tray => "tray",
        CloseBehavior::Quit => "quit",
    };
    // 写失败不能静默：菜单勾选态是 muda 自翻的（视觉已变），实际行为却由本文件
    // 决定（每次关闭都重读）。吞掉错误的话，用户会看到勾选 3 秒后「自己弹回去」
    // 且关闭行为与勾选不符，完全无从排查。
    if let Err(e) = std::fs::write(close_pref_path(state), v) {
        tracing::warn!("关闭行为设置写入失败（勾选将不生效）: {e}");
    }
}

/// 窗口显示时：作为一般应用（macOS 显示 Dock 图标）。
#[cfg(target_os = "macos")]
fn set_app_visible_in_dock<R: tauri::Runtime>(app: &tauri::AppHandle<R>, visible: bool) {
    // Regular = 正常应用（Dock 有图标、可 Cmd-Tab）；Accessory = 只驻留菜单栏托盘、不占 Dock。
    // 「最小化到托盘」时切到 Accessory，让它从 Dock 消失、只留托盘；显示窗口时切回 Regular。
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
}
#[cfg(not(target_os = "macos"))]
fn set_app_visible_in_dock<R: tauri::Runtime>(_app: &tauri::AppHandle<R>, _visible: bool) {}

/// 把主窗口最小化到托盘：隐藏窗口 + macOS 退出 Dock（程序仍在后台跑）。
fn minimize_to_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        // macOS 原生全屏的窗口直接 hide 通常不生效，还会留下一个空 Space；
        // 而策略随后切到 Accessory（Dock 无图标、Cmd-Tab 不可见），窗口就卡在
        // 全屏里很难救回。先退出全屏再隐藏。
        if w.is_fullscreen().unwrap_or(false) {
            let _ = w.set_fullscreen(false);
        }
        let _ = w.hide();
    }
    set_app_visible_in_dock(app, false);
}

/// Windows：系统消息框（GUI 子系统没有控制台，出错必须可见）
#[cfg(windows)]
pub fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    let wide = |x: &str| x.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (t, m) = (wide(title), wide(text));
    unsafe { MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// Windows：检测 WebView2 运行时。缺失时 Tauri 建不出窗口、进程会静默退出，
/// 用户只会觉得「双击没反应」。这里预检并引导安装。
#[cfg(windows)]
fn ensure_webview2() -> bool {
    const CLIENT: &str = r"\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    for root in [
        format!(r"HKLM\SOFTWARE{CLIENT}"),
        format!(r"HKLM\SOFTWARE\WOW6432Node{CLIENT}"),
        format!(r"HKCU\Software{CLIENT}"),
    ] {
        if reg_command(&["query", &root, "/v", "pv"])
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    message_box(
        "终端任务监控",
        "缺少 Microsoft Edge WebView2 运行时，无法显示界面。

         点击确定后将打开官方下载页，安装（常青版引导程序）后重新运行本程序即可。",
    );
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let wide = |x: &str| x.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (op, url) = (wide("open"), wide("https://go.microsoft.com/fwlink/p/?LinkId=2124703"));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), url.as_ptr(),
            std::ptr::null(), std::ptr::null(), 1)
    };
    false
}

/// 运行 Tauri 桌面应用（阻塞，不返回）。
pub fn run(state: SharedState, cfg: DesktopConfig) -> anyhow::Result<()> {
    let crumb = std::sync::Arc::new({
        let path = state.config.data_dir.join("startup.log");
        move |step: &str| {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{step}");
            }
        }
    });
    let crumb_setup = crumb.clone();
    // Windows：先确认 WebView2 存在，否则 Tauri 静默失败，用户以为程序坏了
    #[cfg(windows)]
    if !ensure_webview2() {
        crumb("webview2 missing");
        return Ok(());
    }
    crumb("webview2 ok");

    // 未绑定账号的 agent：窗口地址带 ?pair=配对码 —— 用户在窗口里登录后，
    // 网页会自动把本机绑定到该账号（无需任何手工令牌）。
    let (pair_q, unpaired) = tauri::async_runtime::block_on(async {
        let paired = state.device_token.read().await.is_some();
        let legacy = std::env::var("AM_AGENT_TOKEN").ok().filter(|s| !s.is_empty()).is_some();
        let code = if paired {
            None
        } else {
            state.pair_info.read().await.as_ref().map(|(c, _)| c.clone())
        };
        (code, !paired && !legacy)
    });
    let portal_url = match &pair_q {
        Some(code) => format!("{}/portal?pair={code}", cfg.web_base),
        None => format!("{}/portal", cfg.web_base),
    };
    // 只要未绑定就弹窗引导登录（即便离线没领到配对码——联网后 show_main 会补带码地址）
    let need_onboard = unpaired;
    // 后台启动标记：开机自启带 --background（静默进托盘）；
    // 用户手动打开（双击/安装完立即运行）没有该标记，直接显示窗口
    let background_launch = std::env::args().any(|a| a == "--background");
    let web_base = cfg.web_base.clone();
    let is_agent = cfg.is_agent;
    let state_setup = state.clone();

    let ipc_ctx = std::sync::Arc::new(IpcCtx {
        state: state.clone(),
        web_base: web_base.clone(),
    });
    tauri::Builder::default()
        .manage(ipc_ctx)
        .invoke_handler(tauri::generate_handler![
            autostart_get,
            autostart_set,
            client_auth,
            local_machine_id,
            terminals_get,
            terminal_set_excluded,
            update_status,
            update_start
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // 作为一般桌面应用运行：macOS 显示 Dock 图标（Regular）。
            // agent 模式启动即后台，初始就用 Accessory —— 若先 Regular 再切，
            // set_activation_policy 走事件循环代理，Dock 图标会闪现一下才消失。
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(if background_launch && !need_onboard {
                tauri::ActivationPolicy::Accessory
            } else {
                tauri::ActivationPolicy::Regular
            });

            crumb_setup("setup enter");
            // 主窗口：加载完整前台页面（设备树 / 会话 / 对话 / 设置 / git diff 等全部功能）。
            // agent 模式直接以隐藏态创建 —— 先可见再 hide 会闪一下窗口。
            let url: tauri::Url = portal_url.parse().expect("非法前台地址");
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("终端任务监控")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                // 首次打开居中显示（不设的话 Windows 上位置有偏移）
                .center()
                .visible(!background_launch || need_onboard)
                .build()?;

            // 点右上角关闭按钮：按用户设置的关闭行为处理。
            // - 最小化到托盘（默认）：隐藏窗口，程序留后台继续运行；
            // - 直接退出：关掉整个应用。
            // 用户可在托盘菜单「关闭时最小化到托盘」里切换。
            let close_handle = app.handle().clone();
            let close_state = state_setup.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    match read_close_behavior(&close_state) {
                        CloseBehavior::Tray => {
                            api.prevent_close();
                            minimize_to_tray(&close_handle);
                        }
                        CloseBehavior::Quit => {
                            close_handle.exit(0);
                        }
                    }
                }
            });

            // agent 模式：窗口已以隐藏态创建、策略已是 Accessory（见上），
            // 即「启动即最小化到托盘」，纯后台上报，点托盘图标再打开窗口。

            crumb_setup("window built");
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
                        "show" => {
                            let pair = tauri::async_runtime::block_on(async {
                                if state_evt.device_token.read().await.is_some() {
                                    None
                                } else {
                                    state_evt
                                        .pair_info
                                        .read()
                                        .await
                                        .as_ref()
                                        .map(|(c, _)| format!("{web_base_menu}/portal?pair={c}"))
                                }
                            });
                            show_main_with_pair(app, pair);
                        }
                        "browser" => open_external(&format!("{web_base_menu}/portal")),
                        // 更新推送入口：打开官网「客户端」下载区
                        "update" => spawn_self_update(app.clone(), web_base_menu.clone()),
                        "autostart" => set_autostart(!autostart_enabled()),
                        "close_to_tray" => {
                            // 切换关闭行为并落盘；下次点菜单会重建反映新勾选态
                            let next = match read_close_behavior(&state_evt) {
                                CloseBehavior::Tray => CloseBehavior::Quit,
                                CloseBehavior::Quit => CloseBehavior::Tray,
                            };
                            write_close_behavior(&state_evt, next);
                        }
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
                    let (terminals, excluded, hub_err, upd) = tauri::async_runtime::block_on(async {
                        let t = state_bg.terminals.read().await.clone();
                        let e = state_bg.excludes.read().await.list();
                        let err = state_bg.hub_error.read().await.clone();
                        let u = state_bg.hub_latest_version.read().await.clone();
                        (t, e, err, u)
                    });
                    let connected = state_bg
                        .hub_connected
                        .load(std::sync::atomic::Ordering::Relaxed);
                    let dev_trusted = state_bg
                        .hub_trusted
                        .load(std::sync::atomic::Ordering::Relaxed);
                    // 状态变化才重建菜单，避免高频刷新。
                    // hub_err 必须进签名：否则上报被拒时状态行不会刷新出错误原因。
                    // close_to_tray 勾选态也进签名：托盘里切换后菜单要跟着刷新。
                    let close_tray = read_close_behavior(&state_bg) == CloseBehavior::Tray;
                    let sig = format!(
                        "{:?}|{:?}|{connected}|{dev_trusted}|{hub_err:?}|{upd:?}|{}|{close_tray}",
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
                                        "终端任务监控 · 已连接（同步已断开）".to_string()
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

            crumb_setup("tray built");
            let _ = tray;
            // [diag] 远程页 IPC 桥自检：把探测结果写进页面标题再读回来
            if std::env::var("AM_IPC_DIAG").ok().as_deref() == Some("1") {
                if let Some(dw) = app.get_webview_window("main") {
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(8));
                        let _ = dw.eval(
                            "location.hash='diag-'+(window.__TAURI__?'T1':'T0')+'-'+(window.__TAURI__&&window.__TAURI__.core&&window.__TAURI__.core.invoke?'I1':'I0')",
                        );
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        if let Ok(u) = dw.url() {
                            ulog(&format!("[diag] inject: {u}"));
                        }
                        let _ = dw.eval(
                            "window.__TAURI__&&window.__TAURI__.core&&window.__TAURI__.core.invoke?window.__TAURI__.core.invoke('local_machine_id').then(function(v){location.hash='diag-ok-'+v}).catch(function(e){location.hash='diag-err-'+String(e).replace(/[^a-zA-Z0-9]/g,'_').slice(0,80)}):(location.hash='diag-nobridge')",
                        );
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        if let Ok(u) = dw.url() {
                            ulog(&format!("[diag] invoke: {u}"));
                        }
                    });
                }
            }
            // 更新监视：新版本弹确认框；低于强制下限必须更新否则退出
            spawn_update_watcher(handle.clone(), state_setup.clone(), web_base.clone());
            // [test] 模拟点击更新：与托盘/设置里的真实点击走同一路径
            if std::env::var("AM_TEST_UPDATE_CLICK").ok().as_deref() == Some("1") {
                let th = handle.clone();
                let tw = web_base.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    ulog("[test] 模拟点击更新");
                    spawn_self_update_inner(th, tw, false);
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| {
            // GUI 子系统没有控制台：失败必须让用户看见，否则就是「双击没反应」
            #[cfg(windows)]
            message_box("终端任务监控", &format!("启动失败：{e}"));
            anyhow::anyhow!("Tauri 运行失败: {e}")
        })?;
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
    // 关闭窗口的行为：勾选=最小化到托盘（后台继续跑），不勾=直接退出
    let close_to_tray = CheckMenuItem::with_id(
        manager,
        "close_to_tray",
        "关闭时最小化到托盘",
        true,
        read_close_behavior(state) == CloseBehavior::Tray,
        None::<&str>,
    )?;
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
    // 更新推送：hub 端有更新版本时置顶提示，点击应用内直接更新
    // （mac 自动换包重启；Windows 拉起安装向导），无需去官网手动下载
    if let Some(v) =
        tauri::async_runtime::block_on(async { state.hub_latest_version.read().await.clone() })
    {
        let upd = MenuItem::with_id(
            manager,
            "update",
            format!("⬆ 新版本 v{v} 可用 · 点击更新"),
            true,
            None::<&str>,
        )?;
        menu.append(&upd)?;
        menu.append(&sep()?)?;
    }
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
                "● 已连接 · 同步已断开（在设备管理里重新信任）".to_string()
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
    menu.append(&close_to_tray)?;
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

/// IPC 用共享上下文：客户端状态 + 允许取凭证的 hub 源
pub struct IpcCtx {
    pub state: SharedState,
    /// 窗口应加载的 hub 地址（origin 校验用）
    pub web_base: String,
}

/// 网页端 IPC：本机设备凭证（机器号 + 设备令牌），供页面静默续登 ——
/// 客户端登录一次绑定后，之后网页会话过期由页面拿它换新会话，登录态永不失效。
/// 仅当窗口当前加载的就是配置的 hub 源时才返回，防止窗口被导航到
/// 其它站点后经 IPC 摸走设备令牌。
#[tauri::command]
fn client_auth(
    window: tauri::WebviewWindow,
    ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>,
) -> Option<serde_json::Value> {
    let cur = window.url().ok()?;
    let allowed: tauri::Url = ctx.web_base.parse().ok()?;
    if cur.origin() != allowed.origin() {
        tracing::warn!("client_auth 拒绝非 hub 源: {cur}");
        return None;
    }
    // try_read 而非 blocking_read：Tauri 命令可能跑在异步运行时线程上，
    // blocking_* 在那里会 panic（配对引导曾因此崩过）。此锁竞争极短，
    // 偶发拿不到就让页面下次重试。
    let token = ctx.state.device_token.try_read().ok()?.clone()?;
    Some(serde_json::json!({
        "machineId": ctx.state.config.machine_id,
        "deviceToken": token,
    }))
}

/// 网页端 IPC：本机 machine_id（非敏感），页面用它在设备列表里标出「本机」
#[tauri::command]
fn local_machine_id(ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>) -> String {
    ctx.state.config.machine_id.clone()
}

/// 网页端 IPC：本机探测到的终端列表（含排除状态），设置页「监控范围」用
#[tauri::command]
async fn terminals_get(
    ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>,
) -> Result<Vec<serde_json::Value>, String> {
    let terminals = ctx.state.terminals.read().await.clone();
    let excludes = ctx.state.excludes.read().await;
    Ok(terminals
        .into_iter()
        .map(|(key, name)| {
            serde_json::json!({
                "key": key,
                "name": name,
                "excluded": excludes.is_excluded(&key),
            })
        })
        .collect())
}

/// 网页端 IPC：设置某终端是否排除监控（与托盘「监控范围」同一份配置）
#[tauri::command]
async fn terminal_set_excluded(
    key: String,
    excluded: bool,
    ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>,
) -> Result<(), String> {
    ctx.state.excludes.write().await.set(&key, excluded);
    Ok(())
}

/// 网页端 IPC：更新状态（当前版本 + 可用新版本；latest 为空即已是最新）。
/// 版本信息由上报心跳每 1.5s 与 hub 比对，这里直接读取即等效「检查更新」。
#[tauri::command]
async fn update_status(
    ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>,
) -> Result<serde_json::Value, String> {
    let latest = ctx.state.hub_latest_version.read().await.clone();
    let progress = UPDATE_PROGRESS.lock().unwrap().clone();
    Ok(serde_json::json!({
        "current": env!("CARGO_PKG_VERSION"),
        "latest": latest,
        "progress": progress,
    }))
}

/// 网页端 IPC：立即执行应用内更新（设置页「检查更新 → 立即更新」）
#[tauri::command]
fn update_start(
    app: tauri::AppHandle,
    ctx: tauri::State<'_, std::sync::Arc<IpcCtx>>,
) {
    spawn_self_update_inner(app, ctx.web_base.clone(), false);
}

/// 网页端 IPC：查询开机自启状态。
/// 客户端窗口加载的是远端前台页；页面里的「开机自启」开关经这两个命令
/// 操作本机（浏览器里打开同一页面时没有 __TAURI__，开关不渲染）。
#[tauri::command]
fn autostart_get() -> bool {
    autostart_enabled()
}

/// 网页端 IPC：设置开机自启，返回设置后的真实状态（以系统为准，而非请求值）
#[tauri::command]
fn autostart_set(enable: bool) -> bool {
    set_autostart(enable);
    autostart_enabled()
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
  <array><string>{}</string><string>--background</string></array>
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
            let quoted = format!("\"{}\" --background", target.display());
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
    show_main_with_pair(app, None)
}

/// 打开主窗口；未绑定且有配对码时，先把窗口导航到带 ?pair= 的地址再显示，
/// 保证用户任何时候从托盘打开都能走通「登录即绑定」。
fn show_main_with_pair<R: tauri::Runtime>(app: &tauri::AppHandle<R>, pair_url: Option<String>) {
    if let (Some(url), Some(w)) = (pair_url, app.get_webview_window("main")) {
        if let Ok(u) = url.parse() {
            let _ = w.navigate(u);
        }
    }
    // 从托盘重新打开：恢复 Dock 图标（macOS），再显示并聚焦窗口
    set_app_visible_in_dock(app, true);
    // 非强制更新的确认弹窗只在重新打开 GUI 时出现（每个版本一次）
    maybe_prompt_update(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn open_external(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    // 不能走 `cmd /C start`：GUI 子系统下会先闪出一个控制台窗口再打开浏览器。
    // ShellExecuteW 直接按 URL 协议关联打开默认浏览器，无任何中间窗口。
    #[cfg(target_os = "windows")]
    let r: std::io::Result<()> = {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let wide = |x: &str| x.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (op, u) = (wide("open"), wide(url));
        let h = unsafe {
            ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(),
                std::ptr::null(), std::ptr::null(), 1)
        };
        // 按 Win32 约定，返回值 > 32 表示成功
        if h as usize > 32 {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("ShellExecuteW 返回 {}", h as usize)))
        }
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        tracing::warn!("打开浏览器失败: {e}");
    }
}

// ---------- 应用内自更新 ----------

/// 更新进行中的进度（设置页「客户端版本」一栏展示）
#[derive(Clone, serde::Serialize)]
struct UpdateProgress {
    /// downloading / installing / restarting
    phase: String,
    received: u64,
    total: u64,
}

static UPDATE_PROGRESS: std::sync::Mutex<Option<UpdateProgress>> = std::sync::Mutex::new(None);

fn set_update_progress(phase: &str, received: u64, total: u64) {
    *UPDATE_PROGRESS.lock().unwrap() = Some(UpdateProgress {
        phase: phase.into(),
        received,
        total,
    });
}

fn clear_update_progress() {
    *UPDATE_PROGRESS.lock().unwrap() = None;
}

/// 更新流程日志：GUI 应用没有可见 stderr，必须落盘才能排查
/// （~/.agent-monitor/client.log）
fn ulog(msg: &str) {
    crate::state::client_log(msg);
}

/// 托盘「点击更新」：后台线程执行自更新。
/// - macOS：下载 zip → 原地替换 .app → 重启（全自动，无需用户操作）
/// - Windows：下载安装器静默安装并自动重启
fn spawn_self_update<R: tauri::Runtime>(app: tauri::AppHandle<R>, hub: String) {
    spawn_self_update_inner(app, hub, false);
}

/// 新版本系统通知（不打断使用）：mac 用系统通知，Windows 用 PowerShell 气泡
fn notify_new_version(v: &str) {
    ulog(&format!("[update] 通知新版本 v{v}"));
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"新版本 v{v} 可用，重新打开窗口或在托盘中即可更新\" with title \"终端任务监控\""
        );
        let _ = std::process::Command::new("osascript").args(["-e", &script]).output();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // 托盘气泡：无需任何依赖/权限，右下角弹出
        let ps = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(8000, '终端任务监控', '新版本 v{v} 可用，打开窗口或在设置中更新', 'Info'); Start-Sleep 9; $n.Dispose()"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = v;
}

/// 窗口（重新）显示时：有待更新版本且尚未弹过窗 → 弹确认框。
/// 非强制更新只在这里弹，平时不打断使用。
static UPDATE_DIALOG_SHOWN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn maybe_prompt_update<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(ctx) = app.try_state::<std::sync::Arc<IpcCtx>>() else {
        return;
    };
    let ctx = ctx.inner().clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let latest = tauri::async_runtime::block_on(async {
            ctx.state.hub_latest_version.read().await.clone()
        });
        let Some(v) = latest else { return };
        {
            let mut shown = UPDATE_DIALOG_SHOWN.lock().unwrap();
            if shown.as_deref() == Some(v.as_str()) {
                return;
            }
            *shown = Some(v.clone());
        }
        let local = env!("CARGO_PKG_VERSION");
        let ok = confirm_box(
            "终端任务监控 · 发现新版本",
            &format!("新版本 v{v} 可用（当前 v{local}）。\n更新将自动完成并重启，是否立即更新？"),
            "立即更新",
            "稍后",
        );
        if ok {
            spawn_self_update_inner(app, ctx.web_base.clone(), false);
        }
    });
}

/// 更新进行中的轻量提示（不打断）：mac 系统通知 / Windows 右下角气泡
fn notify_progress(msg: &str) {
    ulog(&format!("[update] {msg}"));
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"终端任务监控\"",
            msg.replace('"', "'")
        );
        let _ = std::process::Command::new("osascript").args(["-e", &script]).output();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let ps = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(6000, '终端任务监控', '{}', 'Info'); Start-Sleep 7; $n.Dispose()",
            msg.replace('\'', " ")
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = msg;
}

/// 阻塞式提示框（更新结果必须让用户看见；通知对未签名应用常被系统吞掉）
fn alert_box(title: &str, text: &str) {
    #[cfg(windows)]
    message_box(title, text);
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('"', "'");
        let script = format!(
            "display dialog \"{}\" with title \"{}\" buttons {{\"好\"}} default button \"好\"",
            esc(text),
            esc(title)
        );
        let _ = std::process::Command::new("osascript").args(["-e", &script]).output();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = (title, text);
    }
}

/// 下载 hub 上的文件到本地路径（自更新专用；产物文件名均为 ASCII）。
/// 流式下载 + 进度日志 + 30s 无数据即报错：跨境网络常见「连上了但一直
/// 不来数据」，整体超时要干等 5 分钟且全程无反馈（实际用户日志：三次
/// 「开始自更新」后连下载完成都没有）——停滞必须快速可见地失败。
fn download_to(url: &str, dest: &std::path::Path) -> anyhow::Result<()> {
    // 弱网环境（跨境链路）单次失败很常见：自动重试一次，两次都挂才报错
    match download_to_once(url, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            ulog(&format!("[update] 首次下载失败（{e}），3s 后重试一次"));
            std::thread::sleep(std::time::Duration::from_secs(3));
            download_to_once(url, dest)
        }
    }
}

fn download_to_once(url: &str, dest: &std::path::Path) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let bytes = rt.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()?;
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.get(url).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("连接更新服务器超时（30s）"))??;
        if !resp.status().is_success() {
            anyhow::bail!("下载失败 HTTP {}", resp.status());
        }
        let total = resp.content_length().unwrap_or(0);
        ulog(&format!("[update] 开始下载 {} 字节", total));
        let mut resp = resp;
        let mut out: Vec<u8> = Vec::with_capacity(total as usize);
        let mut last_mark = 0usize;
        loop {
            let chunk = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                resp.chunk(),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "下载停滞（30s 无数据，已收 {}/{} 字节），请稍后重试或到官网手动下载",
                    out.len(),
                    total
                )
            })??;
            let Some(chunk) = chunk else { break };
            out.extend_from_slice(&chunk);
            set_update_progress("downloading", out.len() as u64, total);
            // 每 2MB 记一次进度，网络问题可从日志直接定位
            if out.len() - last_mark >= 2 * 1024 * 1024 {
                last_mark = out.len();
                ulog(&format!("[update] 已下载 {}/{} 字节", out.len(), total));
            }
        }
        Ok::<_, anyhow::Error>(out)
    })?;
    if bytes.len() < 1024 * 1024 {
        anyhow::bail!("更新包异常（{} 字节），已取消", bytes.len());
    }
    std::fs::write(dest, &bytes)?;
    Ok(())
}

/// 诊断入口：AM_SELF_UPDATE=1 时由 main 调用，前台跑一遍更新流程并打印结果
pub fn self_update_probe(hub: &str) {
    ulog(&format!("[probe] 手动触发自更新，hub={hub}"));
    match do_self_update(hub) {
        Ok(()) => {
            ulog("[probe] 更新成功，退出旧实例");
            std::process::exit(0);
        }
        Err(e) => {
            ulog(&format!("[probe] 更新失败: {e:#}"));
            eprintln!("self-update failed: {e:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "macos")]
fn do_self_update(hub: &str) -> anyhow::Result<()> {
    // 定位自身 .app：exe 位于 <bundle>.app/Contents/MacOS/ 下
    let exe = std::env::current_exe()?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        .ok_or_else(|| anyhow::anyhow!("当前不是 .app 形态，无法自更新"))?
        .to_path_buf();
    ulog(&format!("[update] bundle={}", bundle.display()));

    let tmp = std::env::temp_dir().join(format!("am-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let zip = tmp.join("update.zip");
    download_to(&format!("{hub}/downloads/agent-monitor-mac.zip"), &zip)?;
    ulog("[update] 下载完成");
    set_update_progress("installing", 0, 0);

    // ditto 解包（保留签名/资源叉）
    let ok = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&tmp)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("解包失败");
    }
    let new_app = tmp.join("终端任务监控.app");
    if !new_app.join("Contents/MacOS/agent-monitor").exists() {
        anyhow::bail!("更新包内容异常");
    }
    ulog("[update] 解包完成");

    // 原地替换：旧 .app 改名挪到同一父目录（同目录 rename 不跨卷、不受
    // 临时目录权限影响），放入新包后延迟重启，最后清掉旧包。
    let parent = bundle.parent().ok_or_else(|| anyhow::anyhow!("bundle 无父目录"))?;
    let old = parent.join(".终端任务监控.old.app");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&bundle, &old).map_err(|e| anyhow::anyhow!("移出旧版本失败: {e}"))?;
    let ok = std::process::Command::new("ditto")
        .arg(&new_app)
        .arg(&bundle)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::rename(&old, &bundle);
        anyhow::bail!("写入新版本失败（已回滚）");
    }
    ulog("[update] 新版本已就位，准备重启");
    set_update_progress("restarting", 0, 0);
    let bundle_str = bundle.to_string_lossy().to_string();
    let old_str = old.to_string_lossy().to_string();
    std::process::Command::new("sh")
        .args([
            "-c",
            &format!("sleep 1; AM_SELF_UPDATE=0 AM_TEST_UPDATE_CLICK=0 open \"{bundle_str}\"; sleep 3; rm -rf \"{old_str}\""),
        ])
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
fn do_self_update(hub: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let tmp = std::env::temp_dir();
    let installer = tmp.join("agent-monitor-setup.exe");
    download_to(&format!("{hub}/downloads/agent-monitor-setup.exe"), &installer)?;
    ulog("[update] 安装器下载完成，静默安装");
    set_update_progress("installing", 0, 0);
    // 全静默更新，不出安装向导：NSIS /S 静默安装（沿用上次安装目录与组件选择，
    // 安装器内部会先结束本进程再覆盖），装完从注册表定位新程序并自动重启。
    // 整个流程放在独立的 bat 里执行 —— 本进程会被安装器 taskkill，
    // cmd 宿主不受影响，能等安装结束再拉起新版本。
    let bat = tmp.join("agent-monitor-update.bat");
    let script = format!(
        "@echo off\r\nchcp 65001 >nul\r\n\"{}\" /S\r\nset \"DIR=\"\r\nfor /f \"skip=2 tokens=2,*\" %%a in ('reg query \"HKCU\\Software\\AgentMonitor\" /v \"InstallDir\" 2^>nul') do set \"DIR=%%b\"\r\nif not defined DIR set \"DIR=%LOCALAPPDATA%\\AgentMonitor\"\r\nif exist \"%DIR%\\AgentMonitor.exe\" (start \"\" \"%DIR%\\AgentMonitor.exe\") else (start \"\" \"%DIR%\\终端任务监控.exe\")\r\ndel \"%~f0\"\r\n",
        installer.display()
    );
    std::fs::write(&bat, script.as_bytes())?;
    std::process::Command::new("cmd")
        .arg("/C")
        .arg(&bat)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn do_self_update(_hub: &str) -> anyhow::Result<()> {
    anyhow::bail!("当前平台暂不支持应用内更新")
}

// ---------- 更新提示（确认后更新 / 强制更新） ----------

/// 跨平台确认框：返回用户是否点了「确认」侧按钮。
/// Windows 用系统 MessageBox（是/否）；macOS 用 osascript 对话框（自定义按钮文案）。
fn confirm_box(title: &str, text: &str, ok_label: &str, cancel_label: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO,
        };
        let _ = (ok_label, cancel_label); // 系统按钮固定「是/否」
        let wide = |x: &str| x.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (t, m) = (wide(title), wide(text));
        let r = unsafe {
            MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_YESNO | MB_ICONQUESTION)
        };
        r == IDYES
    }
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('"', "'");
        let script = format!(
            "display dialog \"{}\" with title \"{}\" buttons {{\"{}\", \"{}\"}} default button \"{}\"",
            esc(text),
            esc(title),
            esc(cancel_label),
            esc(ok_label),
            esc(ok_label),
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(ok_label))
            .unwrap_or(false)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = (title, text, ok_label, cancel_label);
        false
    }
}

/// 更新监视线程：发现新版本弹确认框（同一版本每次运行只问一次）；
/// 低于强制更新下限时必须更新 —— 拒绝或更新失败都会退出程序。
pub(crate) fn spawn_update_watcher<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: SharedState,
    hub: String,
) {
    std::thread::spawn(move || {
        let local = env!("CARGO_PKG_VERSION");
        let mut prompted: Option<String> = None;
        let mut forced_prompted = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let (latest, min) = tauri::async_runtime::block_on(async {
                (
                    state.hub_latest_version.read().await.clone(),
                    state.hub_min_version.read().await.clone(),
                )
            });

            // 强制更新：本机低于下限 → 不更新就不能继续使用
            if !forced_prompted {
                if let Some(min) = min.as_deref().filter(|m| crate::agent::version_newer(m, local)) {
                    forced_prompted = true;
                    let ok = confirm_box(
                        "终端任务监控 · 需要更新",
                        &format!(
                            "当前版本 v{local} 已停止支持（最低要求 v{min}）。\n必须更新后才能继续使用；选择退出将关闭程序。"
                        ),
                        "立即更新",
                        "退出程序",
                    );
                    if ok {
                        spawn_self_update_inner(app.clone(), hub.clone(), true);
                    } else {
                        tracing::warn!("用户拒绝强制更新，退出");
                        app.exit(0);
                    }
                    continue;
                }
            }

            // 常规更新：不打断使用 —— 每个新版本只发一次系统通知（右下角/右上角），
            // 确认弹窗延后到用户重新打开 GUI 窗口时（见 maybe_prompt_update）
            if let Some(v) = latest {
                if prompted.as_deref() != Some(v.as_str()) {
                    prompted = Some(v.clone());
                    notify_new_version(&v);
                }
            }
        }
    });
}

/// 自更新执行（forced=true 时失败即退出：强制更新不允许带病运行）
fn spawn_self_update_inner<R: tauri::Runtime>(app: tauri::AppHandle<R>, hub: String, forced: bool) {
    std::thread::spawn(move || {
        ulog(&format!("[update] 开始自更新 forced={forced} hub={hub}"));
        set_update_progress("downloading", 0, 0);
        notify_progress("正在下载更新，完成后将自动重启…");
        match do_self_update(&hub) {
            Ok(()) => {
                ulog("[update] 自更新就绪，退出旧实例");
                app.exit(0);
                // app.exit 走事件循环代理，个别路径（窗口全隐藏时）可能不生效；
                // 稍候仍未退出就硬退，保证新旧实例交接
                std::thread::sleep(std::time::Duration::from_secs(3));
                ulog("[update] app.exit 未生效，强制退出");
                std::process::exit(0);
            }
            Err(e) => {
                clear_update_progress();
                ulog(&format!("[update] 自更新失败: {e:#}"));
                alert_box(
                    "终端任务监控 · 更新失败",
                    &format!(
                        "{e}\n\n{}",
                        if forced {
                            "程序将退出，请到官网手动下载安装。"
                        } else {
                            "可稍后重试，或到官网手动下载安装包。"
                        }
                    ),
                );
                if forced {
                    app.exit(1);
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    std::process::exit(1);
                }
            }
        }
    });
}
