fn main() {
    // 仅在启用 desktop（Tauri）特性时生成 Tauri 上下文；无界面/服务器构建跳过。
    if std::env::var("CARGO_FEATURE_DESKTOP").is_err() {
        return;
    }
    // 应用自定义命令必须生成 ACL 权限（allow-<command>）、并在 capabilities 里放行，否则远程页
    // invoke 一律被拦：「Command X not allowed by ACL」。
    //
    // 命令清单**只在 desktop.rs 的 generate_handler! 里写一份**，这里读出来用，不再手抄：
    // 此前三处（generate_handler! / 这里 / capabilities）各自手工维护，新命令漏登记了没有任何
    // 信号 —— 网页那头的调用把失败吞掉，read_session_image、clear_device_token、plugin_status、
    // plugin_update 就这样一直被拦着没人发现（设置页插件版本从来不显示、失效令牌从来没被清）。
    println!("cargo:rerun-if-changed=src/desktop.rs");
    println!("cargo:rerun-if-changed=capabilities/default.json");
    let commands = handler_commands();
    check_capabilities(&commands);
    let commands: &'static [&'static str] = Box::leak(
        commands
            .into_iter()
            .map(|c| &*Box::leak(c.into_boxed_str()))
            .collect::<Vec<&'static str>>()
            .into_boxed_slice(),
    );
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("tauri_build 失败");
}

/// desktop.rs 里 `tauri::generate_handler![...]` 列的命令名
fn handler_commands() -> Vec<String> {
    let src = std::fs::read_to_string("src/desktop.rs").expect("读 src/desktop.rs 失败");
    let start = src
        .find("generate_handler![")
        .expect("desktop.rs 里找不到 generate_handler![")
        + "generate_handler![".len();
    let end = start + src[start..].find(']').expect("generate_handler! 没有闭合");
    let cmds: Vec<String> = src[start..end]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        cmds.iter().all(|c| c
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')),
        "generate_handler! 里出现了解析不了的内容（只支持逗号分隔的命令名）：{cmds:?}"
    );
    cmds
}

/// 每个命令都得在 capabilities/default.json 里有 `allow-<命令名，下划线换连字符>`
fn check_capabilities(commands: &[String]) {
    let caps = std::fs::read_to_string("capabilities/default.json")
        .expect("读 capabilities/default.json 失败");
    let missing: Vec<String> = commands
        .iter()
        .map(|c| format!("allow-{}", c.replace('_', "-")))
        .filter(|perm| !caps.contains(&format!("\"{perm}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "capabilities/default.json 没有放行这些命令，网页调用会被 ACL 拦下：{missing:?}"
    );
}
