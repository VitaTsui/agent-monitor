//! agent 模式：扫描本机，把任务快照上报给 hub，并执行 hub 下发的控制命令。
use crate::model::{ControlCmd, ReportPayload, Task};
use crate::state::SharedState;
use serde_json::Value;
use std::collections::HashMap;

/// 活跃任务才携带消息缓存，且仅在会话文件变化时重读
struct MsgCache {
    /// session_id → (mtime_ms, messages)
    inner: HashMap<String, (u64, Vec<crate::model::MessageBrief>)>,
}

pub async fn report_loop(state: SharedState, hub_url: String) {
    let hub = hub_url.trim_end_matches('/').to_string();
    let owner = std::env::var("AM_USER").ok().filter(|s| !s.is_empty());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("构建 HTTP 客户端失败");
    let mut msg_cache = MsgCache { inner: HashMap::new() };
    let mut hub_ok = false;
    // 未被 hub 信任前，只发送心跳（设备登记），绝不上报任何会话/终端数据
    let mut trusted = false;

    loop {
        // 始终本地扫描（仅用于本机托盘展示终端列表）；但未信任前不外发任何会话
        let mut scanned = crate::state::local_scan(&state).await;
        let tasks = if trusted {
            attach_messages(&state, &mut scanned, &mut msg_cache).await;
            scanned
        } else {
            Vec::new()
        };

        let payload = ReportPayload {
            machine_id: state.config.machine_id.clone(),
            hostname: state.config.hostname.clone(),
            platform: state.config.platform.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            owner: owner.clone(),
            tasks,
        };

        match client
            .post(format!("{hub}/monitor/report"))
            .header("x-agent-token", &state.config.agent_token)
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => {
                if !hub_ok {
                    tracing::info!("已连上 hub: {hub}");
                    hub_ok = true;
                }
                if let Ok(body) = resp.json::<Value>().await {
                    let now_trusted = body
                        .pointer("/data/trusted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if now_trusted != trusted {
                        tracing::info!(
                            "设备信任状态变更: {}",
                            if now_trusted { "已被信任，开始上报会话" } else { "未信任，仅登记设备" }
                        );
                        trusted = now_trusted;
                    }
                    let commands: Vec<ControlCmd> = body
                        .pointer("/data/commands")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for cmd in commands {
                        execute(&state, cmd).await;
                    }
                    // 待写入文件（hub 下发的文件传输）
                    let files: Vec<crate::model::FileTransfer> = body
                        .pointer("/data/files")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for f in files {
                        write_transfer(&f);
                    }
                }
            }
            Err(e) => {
                if hub_ok {
                    tracing::warn!("上报 hub 失败: {e}");
                }
                hub_ok = false;
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}

/// 给「活跃」任务（有进程或 10 分钟内有写入）附带最近对话消息
async fn attach_messages(state: &SharedState, tasks: &mut [Task], cache: &mut MsgCache) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let scanner = state.scanner.lock().await;
    for t in tasks.iter_mut() {
        let active = t.process.is_some() || now_ms.saturating_sub(t.mtime_ms) < 10 * 60 * 1000;
        if !active || t.id.contains("pid-") {
            continue;
        }
        // 文件没变化就复用缓存，避免每轮重读大文件
        if let Some((mtime, msgs)) = cache.inner.get(&t.id) {
            if *mtime == t.mtime_ms {
                t.recent_messages = msgs.clone();
                continue;
            }
        }
        if let Ok(msgs) = scanner.messages(&t.id, 80) {
            cache.inner.insert(t.id.clone(), (t.mtime_ms, msgs.clone()));
            t.recent_messages = msgs;
        }
    }
    // 清理消失的会话
    let alive: std::collections::HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    cache.inner.retain(|k, _| alive.contains(k.as_str()));
}

/// 写入 hub 下发的文件到本机目标目录
fn write_transfer(f: &crate::model::FileTransfer) {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let Ok(bytes) = B64.decode(f.content_b64.as_bytes()) else {
        tracing::warn!("文件内容解码失败: {}", f.filename);
        return;
    };
    let dir = std::path::PathBuf::from(&f.dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("创建目录失败 {}: {e}", f.dir);
        return;
    }
    let safe = std::path::Path::new(&f.filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.bin".into());
    let target = dir.join(&safe);
    match std::fs::write(&target, &bytes) {
        Ok(_) => tracing::info!("已写入下发文件: {}", target.display()),
        Err(e) => tracing::warn!("写入下发文件失败: {e}"),
    }
}

async fn execute(state: &SharedState, cmd: ControlCmd) {
    let Some(pid) = cmd.pid else {
        tracing::warn!("命令缺少 pid，跳过: {:?}", cmd);
        return;
    };
    // 输入注入（发布任务）单独处理
    if matches!(cmd.action, crate::model::ControlAction::Input) {
        let text = cmd.text.unwrap_or_default();
        match crate::process::send_input(pid, &text) {
            Ok(_) => tracing::info!("执行 hub 输入命令: 任务 {} pid={pid}", cmd.task_id),
            Err(e) => tracing::warn!("执行 hub 输入命令失败: {e}"),
        }
        return;
    }
    match crate::process::control(pid, cmd.action) {
        Ok(label) => {
            let mut paused = state.paused.write().await;
            match cmd.action {
                crate::model::ControlAction::Pause => {
                    paused.insert(pid);
                }
                _ => {
                    paused.remove(&pid);
                }
            }
            tracing::info!("执行 hub 命令: 任务 {} pid={pid} {label}", cmd.task_id);
        }
        Err(e) => tracing::warn!("执行 hub 命令失败: {e}"),
    }
}
