fn main() {
    // 仅在启用 desktop（Tauri）特性时生成 Tauri 上下文；无界面/服务器构建跳过。
    if std::env::var("CARGO_FEATURE_DESKTOP").is_err() {
        return;
    }
    // 应用自定义命令必须生成 ACL 权限（allow-<command>），否则远程页
    // invoke 一律被拦：「Command X not allowed by ACL」——设置里的
    // 自启/监控范围/版本区块与静默续登全都依赖这些命令。
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "autostart_get",
            "autostart_set",
            "client_auth",
            "local_machine_id",
            "terminals_get",
            "terminal_set_excluded",
            "update_status",
            "update_start",
        ])),
    )
    .expect("tauri_build 失败");
}
