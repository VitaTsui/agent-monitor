//! am-core —— hub 与桌面客户端共享的纯逻辑：
//! 数据模型、进程扫描/控制、多模型会话解析、git 对比。
//! 这里不允许出现任何服务端（HTTP/注册表）或客户端（Tauri/上报）代码。
pub mod model;
pub mod process;
pub mod scanner;
