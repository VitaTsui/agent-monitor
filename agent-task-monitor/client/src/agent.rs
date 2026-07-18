//! agent 模式：扫描本机，把任务快照上报给 hub，并执行 hub 下发的控制命令。
use am_core::model::{ControlCmd, ReportPayload, Task};
use crate::state::SharedState;
use serde_json::Value;
use std::collections::HashMap;

/// 活跃任务才携带消息缓存，且仅在会话文件变化时重读
struct MsgCache {
    /// session_id → (mtime_ms, messages)
    inner: HashMap<String, (u64, Vec<am_core::model::MessageBrief>)>,
}

pub async fn report_loop(state: SharedState, hub_url: String) {
    let hub = hub_url.trim_end_matches('/').to_string();
    let owner = std::env::var("AM_USER").ok().filter(|s| !s.is_empty());
    // 全局令牌仅在显式配置时使用（内部部署/兼容旧客户端）；
    // 普通用户走「配对绑定 → 每设备令牌」，无需任何预置密钥。
    let legacy_token = std::env::var("AM_AGENT_TOKEN").ok().filter(|s| !s.is_empty());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("构建 HTTP 客户端失败");
    let mut msg_cache = MsgCache { inner: HashMap::new() };
    let mut hub_ok = false;
    // 未被 hub 信任前，只发送心跳（设备登记），绝不上报任何会话/终端数据
    let mut trusted = false;
    // 待随下一轮上报回传的 git 对比结果
    let mut pending_git_results: Vec<am_core::model::GitResult> = Vec::new();

    loop {
        // 配对阶段：还没有设备令牌（也没配全局令牌）时，不上报，只轮询配对状态。
        // 用户在客户端窗口里登录后，网页会自动认领，这里领到令牌即转入正常上报。
        let has_device_token = state.device_token.read().await.is_some();
        if !has_device_token && legacy_token.is_none() {
            // 先克隆再解构：if-let 直接写 read().await.clone() 的话，读锁临时量会
            // 存活到整个 if/else 结束 —— else 里的 start_pairing 要拿写锁，同任务
            // 读锁未放即等写锁 = 自我死锁；主线程配对引导的 write().await 也会被
            // 这把永不释放的读锁卡死，窗口和托盘永远出不来（Windows「只有进程」即此）。
            let pair = state.pair_info.read().await.clone();
            if let Some((code, pair_token)) = pair {
                match client
                    .get(format!("{hub}/monitor/pair/status"))
                    .query(&[("code", code.as_str()), ("pairToken", pair_token.as_str())])
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if let Ok(body) = resp.json::<Value>().await {
                            if body.pointer("/data/claimed").and_then(Value::as_bool) == Some(true) {
                                if let Some(t) =
                                    body.pointer("/data/deviceToken").and_then(Value::as_str)
                                {
                                    persist_device_token(&state, t).await;
                                    tracing::info!("设备已绑定账号，开始上报");
                                    *state.hub_error.write().await = None;
                                    continue;
                                }
                            }
                            // 配对码过期：重新领一个，窗口下次打开会用新码
                            if body.pointer("/data/expired").and_then(Value::as_bool) == Some(true) {
                                start_pairing(&state, &client, &hub).await;
                            }
                        }
                        state
                            .hub_connected
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(_) => {
                        state
                            .hub_connected
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            } else {
                start_pairing(&state, &client, &hub).await;
            }
            *state.hub_error.write().await =
                Some("未绑定账号：打开客户端窗口登录一次即可自动绑定".into());
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            continue;
        }

        // 始终本地扫描（仅用于本机托盘展示终端列表）；但未信任前不外发任何会话
        let mut scanned = crate::state::local_scan(&state).await;
        // 本轮本机真实存在的会话 pid：hub 下发的命令只允许作用于这些 pid
        let known_pids: std::collections::HashSet<u32> =
            scanned.iter().filter_map(|t| t.pid).collect();
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
            git_results: std::mem::take(&mut pending_git_results),
        };

        let mut req = client.post(format!("{hub}/monitor/report"));
        if let Some(t) = state.device_token.read().await.as_deref() {
            req = req.header("x-device-token", t);
        } else if let Some(t) = &legacy_token {
            req = req.header("x-agent-token", t);
        }
        match req.json(&payload).send().await
        {
            Ok(resp) if !resp.status().is_success() => {
                // 收到响应 ≠ 上报成功：413（负载过大）、401（令牌不对）等
                // 都会走到这里。若照旧标记「已连接」，托盘会一直显示正常，
                // 而实际上没有任何数据同步到 hub。
                let code = resp.status();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!("上报被 hub 拒绝: HTTP {code} {}", body.trim());
                hub_ok = false;
                state
                    .hub_connected
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                // 记下人话原因给托盘显示：这类失败是配置错了，重试一万次也不会好，
                // 必须让用户看见，而不是和断网一样显示「连接中…」。
                *state.hub_error.write().await = Some(describe_reject(code.as_u16(), &body));
                // 设备令牌失效（设备被删除/换绑）：清掉本地令牌，回到配对流程重新绑定
                if code.as_u16() == 401 && legacy_token.is_none() {
                    *state.device_token.write().await = None;
                    let _ = std::fs::remove_file(state.config.data_dir.join("device-token"));
                }
            }
            Ok(resp) => {
                if !hub_ok {
                    tracing::info!("已连上 hub: {hub}");
                    hub_ok = true;
                }
                state
                    .hub_connected
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                // 上报成功即清掉旧的拒绝原因（例如用户刚把令牌改对了）
                if state.hub_error.read().await.is_some() {
                    *state.hub_error.write().await = None;
                }
                if let Ok(body) = resp.json::<Value>().await {
                    // 更新推送：hub 版本比本机新 → 记录，托盘显示「新版本可用」
                    if let Some(hv) = body.pointer("/data/hubVersion").and_then(Value::as_str) {
                        let newer = version_newer(hv, env!("CARGO_PKG_VERSION"));
                        let mut slot = state.hub_latest_version.write().await;
                        let next = newer.then(|| hv.to_string());
                        if *slot != next {
                            if let Some(v) = &next {
                                tracing::info!("检测到新版本可用: v{v}（当前 v{}）", env!("CARGO_PKG_VERSION"));
                            }
                            *slot = next;
                        }
                    }
                    // 强制更新下限：低于它的客户端必须更新才能继续使用（desktop.rs 弹窗执行）
                    if let Some(mv) = body.pointer("/data/minVersion").and_then(Value::as_str) {
                        let mut slot = state.hub_min_version.write().await;
                        if slot.as_deref() != Some(mv) {
                            *slot = Some(mv.to_string());
                        }
                    }
                    // 额度上限由 hub 下发，本地扫描按它执行超额自动暂停/恢复
                    if let Some(limit) = body.pointer("/data/quotaLimit").and_then(Value::as_u64) {
                        state
                            .quota_limit
                            .store(limit, std::sync::atomic::Ordering::Relaxed);
                    }
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
                    state
                        .hub_trusted
                        .store(now_trusted, std::sync::atomic::Ordering::Relaxed);
                    let commands: Vec<ControlCmd> = body
                        .pointer("/data/commands")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for cmd in commands {
                        execute(&state, cmd, &known_pids).await;
                    }
                    // 待写入文件（hub 下发的文件传输）
                    let files: Vec<am_core::model::FileTransfer> = body
                        .pointer("/data/files")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for f in files {
                        write_transfer(&f);
                    }
                    // git 对比请求：本机跑 git，结果随下一轮上报回传
                    let git_queries: Vec<am_core::model::GitQuery> = body
                        .pointer("/data/gitQueries")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for q in git_queries {
                        // git_overview 会起 4 个 git 子进程（含全仓 diff HEAD），是同步阻塞调用。
                        // hub 侧同样的活儿走的是 spawn_blocking，agent 侧不能例外，
                        // 否则大仓库的一次 diff 就把上报循环所在的 worker 线程占住。
                        let cwd = q.cwd.clone();
                        let overview =
                            tokio::task::spawn_blocking(move || am_core::gitdiff::git_overview(&cwd))
                                .await
                                .unwrap_or_default();
                        pending_git_results.push(am_core::model::GitResult {
                            task_id: q.task_id,
                            overview,
                        });
                    }
                }
            }
            Err(e) => {
                if hub_ok {
                    tracing::warn!("上报 hub 失败: {e}");
                }
                hub_ok = false;
                state
                    .hub_connected
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
}

/// 发起配对：向 hub 领配对码，存进 state（窗口用 code 拼 ?pair= 参数）
async fn start_pairing(state: &SharedState, client: &reqwest::Client, hub: &str) {
    let body = serde_json::json!({
        "machineId": state.config.machine_id,
        "hostname": state.config.hostname,
        "platform": state.config.platform,
    });
    if let Ok(resp) = client.post(format!("{hub}/monitor/pair/start")).json(&body).send().await {
        if let Ok(v) = resp.json::<Value>().await {
            if let (Some(code), Some(pt)) = (
                v.pointer("/data/code").and_then(Value::as_str),
                v.pointer("/data/pairToken").and_then(Value::as_str),
            ) {
                tracing::info!("已领取配对码 {code}，等待网页端认领");
                *state.pair_info.write().await = Some((code.to_string(), pt.to_string()));
            }
        }
    }
}

/// 持久化设备令牌（拿到后写盘，下次启动直接上报无需重新配对）
async fn persist_device_token(state: &SharedState, token: &str) {
    *state.device_token.write().await = Some(token.to_string());
    let path = state.config.data_dir.join("device-token");
    if let Err(e) = std::fs::write(&path, token) {
        tracing::warn!("设备令牌写盘失败（重启后需重新配对）: {e}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// a 是否比 b 更新（按点分数字逐段比较；解析不了的段按 0）。
/// 客户端与 hub 版本都出自 Cargo semver，够用且不引依赖。
pub(crate) fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
    };
    let (va, vb) = (parse(a), parse(b));
    for i in 0..va.len().max(vb.len()) {
        let (x, y) = (va.get(i).copied().unwrap_or(0), vb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// 把 hub 的拒绝翻译成用户能据以行动的一句话。
///
/// 这些都是「配置错了」而非「网络抖动」：重试再多次也不会自愈，
/// 必须让托盘上的用户看到该改哪里。
fn describe_reject(code: u16, body: &str) -> String {
    match code {
        401 => "上报令牌不对（请核对 AM_AGENT_TOKEN 与 hub 一致）".into(),
        403 => "hub 拒绝本设备（无权上报）".into(),
        413 => "上报内容过大，已被 hub 拒绝".into(),
        400 => {
            // 400 的具体原因在 body 里（如 machineId 与 hub 本机冲突），原样带出更有用
            let msg = body.trim();
            if msg.is_empty() {
                "上报被 hub 拒绝（400）".into()
            } else {
                // 按字符截断（不是字节），中文原因不会被切出半个字
                let short: String = msg.chars().take(60).collect();
                format!("上报被拒：{short}")
            }
        }
        c if (500..600).contains(&c) => format!("hub 内部错误（{c}），稍后重试"),
        c => format!("上报被拒绝（HTTP {c}）"),
    }
}

/// 给「活跃」任务（有进程或 10 分钟内有写入）附带最近对话消息
async fn attach_messages(state: &SharedState, tasks: &mut [Task], cache: &mut MsgCache) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut scanner = state.scanner.lock().await;
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
fn write_transfer(f: &am_core::model::FileTransfer) {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let Ok(bytes) = B64.decode(f.content_b64.as_bytes()) else {
        tracing::warn!("文件内容解码失败: {}", f.filename);
        return;
    };
    // 目标目录按本机的允许范围复验：不能只信 hub 校验过——
    // hub 的 upload_root 是另一台机器的，且响应链路一旦被篡改就等于本机任意写。
    let dir = match crate::state::safe_upload_dir(&f.dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("拒绝写入下发文件 {}: {e}", f.filename);
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("创建目录失败 {}: {e}", dir.display());
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

/// 执行 hub 下发的控制命令。
/// `known_pids` 是本轮本机扫描出的会话 pid 集合——只对这些 pid 动手，
/// 不无条件信任 hub 响应（响应链路若被中间人篡改，否则可对任意进程发信号）。
async fn execute(state: &SharedState, cmd: ControlCmd, known_pids: &std::collections::HashSet<u32>) {
    let Some(pid) = cmd.pid else {
        tracing::warn!("命令缺少 pid，跳过: {:?}", cmd);
        return;
    };
    if !known_pids.contains(&pid) {
        tracing::warn!(
            "拒绝执行：pid={pid} 不属于本机当前会话（任务 {}）",
            cmd.task_id
        );
        return;
    }
    // 输入注入（发布任务）单独处理
    if matches!(cmd.action, am_core::model::ControlAction::Input) {
        let text = cmd.text.unwrap_or_default();
        // send_input 在 macOS 上走 osascript，会遍历 Terminal/iTerm 的每个窗口与标签页，
        // 常态就要数秒，终端处于模态/无响应时还可能一直挂着 —— 绝不能占住 async worker。
        let res = tokio::task::spawn_blocking(move || am_core::process::send_input(pid, &text)).await;
        match res {
            Ok(Ok(_)) => tracing::info!("执行 hub 输入命令: 任务 {} pid={pid}", cmd.task_id),
            Ok(Err(e)) => tracing::warn!("执行 hub 输入命令失败: {e}"),
            Err(e) => tracing::warn!("执行 hub 输入命令的阻塞任务异常: {e}"),
        }
        return;
    }
    match am_core::process::control(pid, cmd.action) {
        Ok(label) => {
            // 加锁顺序须与 enforce_quota 一致（auto_paused → paused），反序会死锁。
            let mut auto = state.auto_paused.write().await;
            let mut paused = state.paused.write().await;
            match cmd.action {
                am_core::model::ControlAction::Pause => {
                    paused.insert(pid);
                }
                _ => {
                    paused.remove(&pid);
                    // 同 server::control_task：不清 auto 会让额度管控对该 pid 永久失效
                    auto.remove(&pid);
                }
            }
            tracing::info!("执行 hub 命令: 任务 {} pid={pid} {label}", cmd.task_id);
        }
        Err(e) => tracing::warn!("执行 hub 命令失败: {e}"),
    }
}

#[cfg(test)]
mod reject_tests {
    use super::*;

    /// 配置类错误必须给出可据以行动的话，而不是笼统的「连接中…」
    #[test]
    fn actionable_messages_for_config_errors() {
        let m = describe_reject(401, "");
        assert!(m.contains("AM_AGENT_TOKEN"), "401 应指出改哪个配置: {m}");

        // 400 的具体原因在 body 里（如 machineId 冲突），要原样带出
        let m = describe_reject(400, "machineId 与 hub 本机冲突");
        assert!(m.contains("machineId 与 hub 本机冲突"), "400 应带出 body 原因: {m}");
    }

    /// body 为空的 400 不能拼出「上报被拒：」这种半截话
    #[test]
    fn empty_body_400_still_reads_well() {
        let m = describe_reject(400, "   ");
        assert!(!m.ends_with('：'), "不该留下空悬的冒号: {m}");
        assert!(m.contains("400"));
    }

    /// 中文原因按字符截断，不能切出半个字（按字节截会 panic 或乱码）
    #[test]
    fn truncates_by_chars_not_bytes() {
        let long = "会话".repeat(80);
        let m = describe_reject(400, &long);
        assert!(m.chars().count() < 80, "应被截断: {}", m.chars().count());
        // 能正常成串即说明没在字符中间切断
        assert!(m.contains("会话"));
    }

    #[test]
    fn server_errors_are_transient_wording() {
        let m = describe_reject(503, "");
        assert!(m.contains("稍后重试"), "5xx 属于可自愈，措辞应区别于配置错误: {m}");
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    /// 更新推送的判定核心：只有 hub 严格更新才提示
    #[test]
    fn newer_detection() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(version_newer("0.1.10", "0.1.9"), "逐段数字比较，不是字符串比较");
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(!version_newer("0.1.0", "0.1.0"), "相同版本不提示");
        assert!(!version_newer("0.1.0", "0.2.0"), "hub 更旧不提示");
        assert!(version_newer("0.1.0.1", "0.1.0"), "段数不同按 0 补齐");
        assert!(!version_newer("abc", "0.1.0"), "解析不了按 0，不误报");
    }
}
