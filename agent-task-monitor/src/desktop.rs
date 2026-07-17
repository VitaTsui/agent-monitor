//! Tauri v2 桌面外壳：ToDesk 式设备管理器窗口 + 系统托盘。
//! 窗口加载本地/远端 hub 托管的前台页面（设备树 → 选中设备看终端会话 + 设备管理）。
//! 服务在后台线程运行；本模块在主线程跑 Tauri 事件循环。
#![cfg(feature = "desktop")]

use crate::state::SharedState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
    // 菜单项 id → tty（监控范围勾选），跨线程共享
    let excl_map: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    let state_setup = state.clone();
    let excl_setup = excl_map.clone();

    tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();

            // 主窗口：加载前台（ToDesk 式设备/终端管理）
            let url: tauri::Url = portal_url.parse().expect("非法前台地址");
            let _win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("终端任务监控 · 设备与终端")
                .inner_size(1280.0, 820.0)
                .min_inner_size(960.0, 640.0)
                .build()?;

            // 托盘菜单
            let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let browser = MenuItem::with_id(app, "browser", "在浏览器打开", true, None::<&str>)?;
            let scope = build_scope_submenu(&handle, &state_setup, &excl_setup)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show,
                    &browser,
                    &PredefinedMenuItem::separator(app)?,
                    &scope,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            let web_base_menu = web_base.clone();
            let excl_evt = excl_map.clone();
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
                        "quit" => app.exit(0),
                        other => {
                            // 监控范围勾选项：id=excl::<tty>
                            if let Some(tty) = other.strip_prefix("excl::") {
                                let tty = tty.to_string();
                                let map = excl_evt.lock().unwrap();
                                // 当前是否已排除 → 取反
                                let now_excluded = map.values().any(|t| t == &tty);
                                let want = !now_excluded;
                                let st = state_evt.clone();
                                tauri::async_runtime::spawn(async move {
                                    st.excludes.write().await.set(&tty, want);
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

            // 后台线程定期重建「监控范围」子菜单，反映实时终端与勾选状态
            let handle_bg = handle.clone();
            let state_bg = state_setup.clone();
            let excl_bg = excl_map.clone();
            std::thread::spawn(move || {
                let mut last_sig = String::new();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let (terminals, excluded) = tauri::async_runtime::block_on(async {
                        let t = state_bg.terminals.read().await.clone();
                        let e = state_bg.excludes.read().await.list();
                        (t, e)
                    });
                    let sig = format!("{:?}|{:?}", terminals, excluded);
                    if sig == last_sig {
                        continue;
                    }
                    last_sig = sig;
                    if let Ok(scope) = build_scope_submenu(&handle_bg, &state_bg, &excl_bg) {
                        let show = MenuItem::with_id(&handle_bg, "show", "显示窗口", true, None::<&str>);
                        let browser =
                            MenuItem::with_id(&handle_bg, "browser", "在浏览器打开", true, None::<&str>);
                        let quit = MenuItem::with_id(&handle_bg, "quit", "退出", true, None::<&str>);
                        if let (Ok(show), Ok(browser), Ok(quit), Ok(sep1), Ok(sep2)) = (
                            show,
                            browser,
                            quit,
                            PredefinedMenuItem::separator(&handle_bg),
                            PredefinedMenuItem::separator(&handle_bg),
                        ) {
                            if let Ok(menu) = Menu::with_items(
                                &handle_bg,
                                &[&show, &browser, &sep1, &scope, &sep2, &quit],
                            ) {
                                if let Some(tray) = handle_bg.tray_by_id("main") {
                                    let _ = tray.set_menu(Some(menu));
                                }
                            }
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

/// 构建「监控范围（勾选=不监控该终端）」子菜单
fn build_scope_submenu<R: tauri::Runtime>(
    manager: &impl Manager<R>,
    state: &SharedState,
    excl_map: &Arc<Mutex<HashMap<String, String>>>,
) -> tauri::Result<Submenu<R>> {
    let (terminals, excluded) = tauri::async_runtime::block_on(async {
        let t = state.terminals.read().await.clone();
        let e: std::collections::HashSet<String> =
            state.excludes.read().await.list().into_iter().collect();
        (t, e)
    });

    let submenu = Submenu::new(manager, "监控范围（勾选=不监控）", true)?;
    let mut map = excl_map.lock().unwrap();
    map.clear();
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
            map.insert(id, tty.clone());
        }
    }
    Ok(submenu)
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
