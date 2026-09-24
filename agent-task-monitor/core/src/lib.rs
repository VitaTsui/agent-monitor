//! am-core —— hub 与桌面客户端共享的纯逻辑：
//! 数据模型、进程扫描/控制、多模型会话解析、git 对比。
//! 这里不允许出现任何服务端（HTTP/注册表）或客户端（Tauri/上报）代码。
/// 配置同步的路径白名单（客户端与 hub 共用同一份规则）
pub mod configpath;

/// 客户端 → hub 的上报节奏，以及由它推出的离线门槛。两边共用这一份：
/// 客户端按它发，hub 按它判，改节奏时离线门槛自动跟着变。
pub mod heartbeat {
    /// 两轮上报之间的等待
    pub const INTERVAL_MS: u64 = 1500;
    /// 单次上报请求的总超时
    pub const REQUEST_TIMEOUT_SECS: u64 = 5;
    /// 一轮里本地扫描的耗时预算（实测 0.5–1s）
    const SCAN_BUDGET_MS: u64 = 1000;
    /// 一轮上报失败的最坏耗时：扫描 + 请求卡满超时 + 等下一轮
    const FAILED_ROUND_MS: u64 = SCAN_BUDGET_MS + REQUEST_TIMEOUT_SECS * 1000 + INTERVAL_MS;
    /// 连续失败几轮才算离线。
    ///
    /// 实测（2026-09-25，国内经 Cloudflare 到 hub）：平均 2s，5%–8% 的请求超过 5s，
    /// 最慢一次 15s 没回来；绕开 Cloudflare 直连源站则 0/25 超时。单次卡住在这条链路上
    /// 是常态 —— 原先 10s 门槛只够容忍 1 轮，网络一抖就判掉线。
    pub const TOLERATED_FAILED_ROUNDS: u64 = 4;
    /// 连续这么久没有一次成功上报 = 离线（当前 4 × 7.5s = 30s）
    pub const OFFLINE_AFTER_SECS: u64 = (TOLERATED_FAILED_ROUNDS * FAILED_ROUND_MS).div_ceil(1000);
    const _: () = assert!(OFFLINE_AFTER_SECS == 30);
}

pub mod model;
pub mod process;
pub mod scanner;
