fn main() {
    // 仅在启用 desktop（Tauri）特性时生成 Tauri 上下文；无界面/服务器构建跳过。
    if std::env::var("CARGO_FEATURE_DESKTOP").is_ok() {
        tauri_build::build();
    }
}
