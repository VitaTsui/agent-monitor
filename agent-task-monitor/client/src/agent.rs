//! agent 模式：扫描本机，把任务快照上报给 hub，并执行 hub 下发的控制命令。
use am_core::model::{ControlCmd, ReportPayload, Task};
use crate::state::SharedState;
use serde_json::Value;
use std::collections::HashMap;

/// 活跃任务才携带消息缓存，且仅在「与结果相关的文件」变化时重读。
///
/// 缓存键必须同时带上**子会话记录目录**的最新写入时刻：消息末尾那条 `bgtasks` 快照里
/// 「子会话还在不在跑」是由 `<会话>/subagents/*.jsonl` 决定的，父会话 jsonl 不动、
/// 子会话跑完，答案照样变了。只按父会话 mtime 缓存的话，父会话闲着的那段时间里，
/// 跑完的子会话胶囊会一直挂着，非得等用户下次敲字才清掉。
///
/// 还得给缓存**压一个最长寿命**：快照里「子会话跑完没」有两道判定是随墙钟翻的
/// （静置够久 → 收尾、停在半路太久 → 放弃），翻的那一刻没有任何文件在变，纯按 mtime
/// 做键的话它们永远轮不到执行。取 `SUBAGENT_SETTLE_MS`（= 那两道判定的时间分辨率），
/// 代价是活跃会话每 5 分钟多重算一次消息。
struct MsgCache {
    /// session_id → (父会话 mtime_ms, 子会话记录最新写入 ms, 算出来的时刻, messages)
    inner: HashMap<String, (u64, u64, std::time::Instant, Vec<am_core::model::MessageBrief>)>,
}

/// 下发后「待确认是否真的提交」的记录。Cursor 内嵌终端粘贴态会吞掉提交回车，表现为
/// 「任务贴进去了但只换行没发出」。下发后盯会话 jsonl：下一轮扫描里若该会话最新用户
/// 消息不是这条文本，就确认没提交、经桥接补一个回车（最多 MAX_RESUBMIT 次）。只在
/// **正向确认没提交**时补——不盲发，避免误提交/误确认交互式选择框。
struct PendingSubmit {
    text: String,
    shell_pid: u32,
    last_ms: u64,
    retries: u8,
}

/// 待确认提交表：session_id → 记录。跨 execute()（下发）与 report 循环（检测）共享。
static PENDING_SUBMITS: std::sync::Mutex<Option<HashMap<String, PendingSubmit>>> =
    std::sync::Mutex::new(None);

/// 分片传输中「这一次传输实际落到哪个文件」：`dir|filename` → 真实落盘路径。
///
/// 同名文件不再覆盖而是自动改名（见 [`unique_target`]），但改名只能在**第 0 片**定一次：
/// 后续片若各自再算一遍，第 1 片会看到第 0 片刚建好的 `a (1).png` 已存在、于是算出
/// `a (2).png`，每片各自成文件，传完一个都不完整。故第 0 片把结果记在这里，后续片照取。
static CHUNK_TARGETS: std::sync::Mutex<Option<HashMap<String, std::path::PathBuf>>> =
    std::sync::Mutex::new(None);

/// 确认没提交后，等多久补第一个回车 / 两次补回车之间的间隔
/// 配置清单的扫描/上报间隔。心跳是 1.5s 一轮，但配置文件几乎不动，
/// 每轮都扫盘、都把几百条指纹塞进上报纯属浪费。hub 会把收到的清单缓存住，
/// 期间的 pull/push 照常每轮推进，所以这里放慢不影响同步速度。
const CONFIG_SCAN_INTERVAL_SECS: u64 = 30;

const RESUBMIT_WAIT_MS: u64 = 2000;
/// 最多补几次回车，仍不提交就放弃（避免无限补）
const MAX_RESUBMIT: u8 = 2;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 判断会话最新用户提示词是否就是刚下发的这条文本（= 已提交）。
/// scanner 对长文本可能截断，故用「归一化空白后取较短者前 40 字比较」容错。
fn submit_landed(prompt: &str, dispatched: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (p, d) = (norm(prompt), norm(dispatched));
    if p.is_empty() || d.is_empty() {
        return false;
    }
    let n = 40.min(p.chars().count()).min(d.chars().count());
    let head = |s: &str| s.chars().take(n).collect::<String>();
    head(&p) == head(&d)
}

pub async fn report_loop(state: SharedState, hub_url: String) {
    let hub = hub_url.trim_end_matches('/').to_string();
    let owner = std::env::var("AM_USER").ok().filter(|s| !s.is_empty());
    // 全局令牌仅在显式配置时使用（内部部署/兼容旧客户端）；
    // 普通用户走「配对绑定 → 每设备令牌」，无需任何预置密钥。
    let legacy_token = std::env::var("AM_AGENT_TOKEN").ok().filter(|s| !s.is_empty());
    // 连接池空闲超时短一点 + TCP keepalive：休眠/唤醒后不会复用死 socket
    // 而挂起，能尽快用新连接重连（自动恢复连接的关键）。
    fn build_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .connect_timeout(std::time::Duration::from_secs(4))
            .pool_idle_timeout(std::time::Duration::from_secs(15))
            .tcp_keepalive(std::time::Duration::from_secs(20))
            .build()
            .expect("构建 HTTP 客户端失败")
    }
    let mut client = build_client();
    let mut msg_cache = MsgCache { inner: HashMap::new() };
    let mut hub_ok = false;
    // 连续网络失败次数：用于给失败日志限流（首次必打，之后每 ~60s 一条）
    let mut net_fail_streak: u32 = 0;
    // 未被 hub 信任前，只发送心跳（设备登记），绝不上报任何会话/终端数据
    let mut trusted = false;
    let mut pending_dir_results: Vec<am_core::model::DirResult> = Vec::new();
    let mut pending_fs_op_results: Vec<am_core::model::FsOpResult> = Vec::new();
    let mut pending_file_fetches: Vec<am_core::model::FileFetchResult> = Vec::new();
    // 下发文件的落盘回报：hub 拿它回填任务正文里的路径（见 write_transfer）
    let mut pending_file_results: Vec<am_core::model::FileTransferResult> = Vec::new();
    // 配置同步：扫描器带哈希缓存；清单每 CONFIG_SCAN_INTERVAL_SECS 报一次（不是每轮），
    // hub 侧会把它缓存下来，pull/push 每轮都能基于缓存推进。
    let mut cfg_scanner = crate::configsync::ConfigScanner::new();
    let mut cfg_manifest: Option<am_core::model::ConfigManifest> = None;
    let mut last_cfg_scan: Option<std::time::Instant> = None;
    let mut pending_cfg_bodies: Vec<am_core::model::ConfigFileBody> = Vec::new();

    // 监听会话目录：文件一有写入（用户在终端里发了任务、助手产生输出）就立刻唤醒本
    // 循环扫描上报，而不必干等 1.5s 轮询——后者在窗口关到托盘/失焦后会被 macOS
    // 定时器节流压到约一分钟一次，导致「发了任务但面板迟迟不更新」。FSEvents/inotify
    // 这类文件事件不受定时器节流影响，能可靠唤醒后台进程。轮询保留为兜底。
    let file_changed = std::sync::Arc::new(tokio::sync::Notify::new());
    let _fs_watcher = {
        use notify::{RecursiveMode, Watcher};
        let fc = file_changed.clone();
        let dirs = {
            let scanner = state.scanner.lock().await;
            let home = dirs::home_dir().unwrap_or_default();
            vec![scanner.projects_dir().to_path_buf(), home.join(".codex/sessions")]
        };
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // 内容有变化才唤醒（元数据/访问时间等噪声忽略），避免空转
            if matches!(res, Ok(ev) if ev.kind.is_create() || ev.kind.is_modify() || ev.kind.is_remove()) {
                fc.notify_one();
            }
        }) {
            Ok(mut w) => {
                for d in &dirs {
                    if d.is_dir() {
                        if let Err(e) = w.watch(d, RecursiveMode::Recursive) {
                            tracing::warn!("监听会话目录失败 {}: {e}", d.display());
                        }
                    }
                }
                Some(w)
            }
            Err(e) => {
                tracing::warn!("创建文件监听失败，退回纯轮询: {e}");
                None
            }
        }
    };

    // 上一轮循环结束的时刻：用于检测系统睡眠/唤醒（间隔远超预期即刚恢复）
    let mut last_tick = std::time::Instant::now();
    loop {
        // 开机/唤醒检测：距上一轮已过去远超正常 1.5s（阈值 8s），大概率系统
        // 刚从睡眠/休眠恢复 —— 连接池里可能全是死 socket，重建客户端并强制
        // 下一轮当作断线重连，尽快恢复连接。
        if last_tick.elapsed() >= std::time::Duration::from_secs(8) {
            tracing::info!("检测到系统恢复（间隔 {:?}），重建连接自动重连", last_tick.elapsed());
            client = build_client();
            hub_ok = false;
            state
                .hub_connected
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
        last_tick = std::time::Instant::now();

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
        // 下发后检测：本轮扫到的会话里，之前下发但只换行没提交的，补一个回车
        check_pending_submits(&state, &scanned);
        // 本轮本机真实存在的会话 pid：hub 下发的命令只允许作用于这些 pid
        let known_pids: std::collections::HashSet<u32> =
            scanned.iter().filter_map(|t| t.pid).collect();
        // IDE 内嵌终端(Cursor/VSCode)的 claude_pid → 终端 shell pid：扫描时已算好的终端锚，
        // 下发走文件桥时直接用，免得 execute 里再 ide_shell_pid() 另起一次扫描。只收 IDE 终端，
        // 复刻 ide_shell_pid「非 IDE 返回 None」的语义（非 IDE 走原生注入，不进桥接分支）。
        let ide_shell_of: std::collections::HashMap<u32, u32> = scanned
            .iter()
            .filter_map(|t| {
                let p = t.process.as_ref()?;
                matches!(p.ide, am_core::model::IdeKind::Cursor | am_core::model::IdeKind::Vscode)
                    .then(|| p.shell_pid.map(|s| (p.pid, s)))
                    .flatten()
            })
            .collect();
        // 活跃会话的项目目录：文件上传允许写进这些目录（项目常不在家目录下，
        // 见 safe_upload_dir_within）
        let session_dirs: Vec<std::path::PathBuf> = scanned
            .iter()
            .filter_map(|t| t.process.as_ref())
            .map(|p| p.cwd.clone())
            .filter(|c| !c.is_empty())
            .map(std::path::PathBuf::from)
            .collect();
        let tasks = if trusted {
            attach_messages(&state, &mut scanned, &mut msg_cache).await;
            scanned
        } else {
            Vec::new()
        };

        // 配置清单：仅在设备已被信任后才扫、才发 —— 未信任设备一个字节的本机信息都不外发，
        // 而文件路径里带着用户自己起的 agent/skill 名字，同样算本机信息。
        if trusted {
            let due = last_cfg_scan
                .map(|t| t.elapsed().as_secs() >= CONFIG_SCAN_INTERVAL_SECS)
                .unwrap_or(true);
            if due {
                if let Some(home) = dirs::home_dir() {
                    cfg_manifest = Some(cfg_scanner.scan(&home));
                }
                last_cfg_scan = Some(std::time::Instant::now());
            }
        }

        let payload = ReportPayload {
            machine_id: state.config.machine_id.clone(),
            hostname: state.config.hostname.clone(),
            platform: state.config.platform.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            owner: owner.clone(),
            tasks,
            dir_results: std::mem::take(&mut pending_dir_results),
            fs_op_results: std::mem::take(&mut pending_fs_op_results),
            file_fetch_results: std::mem::take(&mut pending_file_fetches),
            file_results: std::mem::take(&mut pending_file_results),
            // take：清单发出去就清空，下一轮不再重发。这一轮若上报失败，最多等
            // 一个扫描周期后重来——不值得为此在内存里长期挂一份待发清单。
            config_manifest: cfg_manifest.take(),
            config_bodies: std::mem::take(&mut pending_cfg_bodies),
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
                    crate::secrets::clear(&state.config.data_dir);
                }
            }
            Ok(resp) => {
                if !hub_ok {
                    tracing::info!("已连上 hub: {hub}");
                    hub_ok = true;
                }
                net_fail_streak = 0;
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
                    // 解析失败必须出声。hub 那头是「drain 即交付」——响应发出时队列已经清空，
                    // 这里再静默当成「没有命令」，那条命令就永久消失了：钉钉/网页显示「已下发」，
                    // 终端什么也没收到，而两头都不会留下任何痕迹。宁可丢一批也要留下证据。
                    let commands: Vec<ControlCmd> = match body.pointer("/data/commands") {
                        Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                            .unwrap_or_else(|e| {
                                crate::state::client_log(&format!(
                                    "下发命令解析失败，本批 {} 条被丢弃：{e}；原文 {}",
                                    v.as_array().map(|a| a.len()).unwrap_or(0),
                                    v.to_string().chars().take(300).collect::<String>()
                                ));
                                Vec::new()
                            }),
                        _ => Vec::new(),
                    };
                    for cmd in commands {
                        execute(&state, cmd, &known_pids, &ide_shell_of).await;
                    }
                    // 待写入文件（hub 下发的文件传输）。同样不能静默吞——文件丢了，
                    // 回填进任务的路径却还在，agent 只会报「文件不存在」。
                    let files: Vec<am_core::model::FileTransfer> = match body.pointer("/data/files")
                    {
                        Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                            .unwrap_or_else(|e| {
                                crate::state::client_log(&format!(
                                    "下发文件解析失败，本批 {} 个被丢弃：{e}",
                                    v.as_array().map(|a| a.len()).unwrap_or(0)
                                ));
                                Vec::new()
                            }),
                        _ => Vec::new(),
                    };
                    for mut f in files {
                        // 落盘那一刻现算目标目录：hub 排队 + 网络往返期间会话可能又 cd 了，
                        // 事先算好的绝对路径就已经过时（见 model 的 FileTransfer::by_session）
                        if f.by_session {
                            match session_root_now(&state, &f.task_id).await {
                                Some(root) => f.dir = join_rel(&root, &f.rel_dir),
                                // 解析不出来（会话记录已删/读不到）就退回 hub 算的那份，
                                // 总比整份传输直接失败强
                                None => crate::state::client_log(&format!(
                                    "下发文件按会话解析目录失败（task={}），退回 hub 给的 {}",
                                    f.task_id, f.dir
                                )),
                            }
                        }
                        if let Some(r) = write_transfer(&f, &session_dirs) {
                            pending_file_results.push(r);
                        }
                    }
                    // 目录列举请求（上传选目录）：列出 cwd/rel 下的子目录
                    let dir_queries: Vec<am_core::model::DirQuery> = body
                        .pointer("/data/dirQueries")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for q in dir_queries {
                        // 根以本机现读为准：hub 那份来自定期扫描的快照，会话 cd 过就偏了
                        let root = match q.by_session {
                            true => session_root_now(&state, &q.task_id).await.unwrap_or(q.cwd),
                            false => q.cwd,
                        };
                        let (dirs, files) = list_entries(&root, &q.rel);
                        pending_dir_results.push(am_core::model::DirResult {
                            dirs,
                            files,
                            task_id: q.task_id,
                            rel: q.rel,
                            root,
                        });
                    }
                    // 文件夹操作（上传选目录弹窗里的新建/删除/重命名）
                    let fs_ops: Vec<am_core::model::FsOp> = body
                        .pointer("/data/fsOps")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for mut op in fs_ops {
                        // 必须与目录浏览同一个根，否则「网页上看到的目录」与「操作落到的目录」是两个
                        if op.by_session {
                            if let Some(root) = session_root_now(&state, &op.task_id).await {
                                op.cwd = root;
                            }
                        }
                        let (ok, msg) = run_fs_op(&op);
                        pending_fs_op_results.push(am_core::model::FsOpResult {
                            op_id: op.op_id,
                            ok,
                            msg,
                        });
                    }
                    // 现取文件（网页要看 agent 输出里引用的截图）
                    let fetches: Vec<am_core::model::FileFetch> = body
                        .pointer("/data/fileFetches")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    for mut f in fetches {
                        // 会话内容里的相对图片路径也是终端按当前目录写下的，根同上
                        if f.by_session {
                            if let Some(root) = session_root_now(&state, &f.task_id).await {
                                f.cwd = root;
                            }
                        }
                        pending_file_fetches.push(read_session_file(&f));
                    }
                    // 配置同步：hub 点名索要的文件内容（下一轮随上报回传）
                    let cfg_pulls: Vec<String> = body
                        .pointer("/data/configPulls")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    if !cfg_pulls.is_empty() {
                        if let Some(home) = dirs::home_dir() {
                            pending_cfg_bodies = crate::configsync::read_bodies(&home, &cfg_pulls);
                        }
                    }
                    // 配置同步：hub 下发的配置内容（备份 + 原子写，路径白名单在 configsync 内复验）
                    let cfg_pushes: Vec<am_core::model::ConfigPush> = body
                        .pointer("/data/configPushes")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    if !cfg_pushes.is_empty() {
                        if let Some(home) = dirs::home_dir() {
                            if crate::configsync::apply(&home, &cfg_pushes) > 0 {
                                // 落盘改变了本机状态，立刻重扫一次报上去，
                                // 否则 hub 手里的清单还是旧的，下一轮会把同样的文件再推一遍。
                                last_cfg_scan = None;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                // 连不上 hub 也必须留痕。原先只在 `hub_ok` 为真时打日志 ——
                // 也就是「本来连着、突然断了」才记一条；而**从来没连上过**的客户端
                // （hub_ok 恒为 false）一条都不打，托盘也没有原因可显示。
                // 实际后果：换服务器后客户端拿着作废的设备令牌空转，日志里干干净净，
                // 用户只看到网页上什么都没有，无从查起（这个 bug 就是这么被发现的）。
                // 首次失败必打，之后每 ~60s 一条，既不刷屏也不至于全无痕迹。
                if hub_ok || net_fail_streak == 0 || net_fail_streak % 40 == 0 {
                    tracing::warn!("上报 hub 失败（第 {} 次）: {e}", net_fail_streak + 1);
                }
                net_fail_streak = net_fail_streak.saturating_add(1);
                hub_ok = false;
                state
                    .hub_connected
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                // 托盘要能说出「为什么连不上」，而不是永远停在「连接中…」
                *state.hub_error.write().await = Some(format!("连不上 hub（{hub}）：{e}"));
            }
        }

        // 正常等 1.5s；但会话文件一变就提前醒来立即上报。文件事件后稍等 150ms
        // 聚合连续写入（一次编辑常触发多条事件），避免同一动作触发多轮扫描。
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(1500)) => {}
            _ = file_changed.notified() => {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
        }
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
    // 系统安全存储（mac 钥匙串 / Windows DPAPI），失败回退受限权限文件
    crate::secrets::save(&state.config.data_dir, token);
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
    // 400/401 的具体原因 hub 已经在 body 里说清了（且比这里的猜测准），原样带出。
    // 401 尤其要这样：现在它的主因是**设备令牌失效**（换了 hub、设备被删），
    // hub 会答「设备未绑定账号：打开客户端窗口登录一次即可自动绑定」——正是该做的事；
    // 而原先硬编码成「核对 AM_AGENT_TOKEN」，会把人引去查一个多数机器上根本没设的变量。
    let from_body = |fallback: &str| -> String {
        let msg = body.trim();
        if msg.is_empty() {
            fallback.to_string()
        } else {
            // 按字符截断（不是字节），中文原因不会被切出半个字
            let short: String = msg.chars().take(60).collect();
            format!("上报被拒：{short}")
        }
    };
    match code {
        401 => from_body("上报令牌无效（设备未绑定或令牌已失效），打开客户端窗口登录一次即可重新绑定"),
        403 => "hub 拒绝本设备（无权上报）".into(),
        413 => "上报内容过大，已被 hub 拒绝".into(),
        400 => from_body("上报被 hub 拒绝（400）"),
        c if (500..600).contains(&c) => format!("hub 内部错误（{c}），稍后重试"),
        c => format!("上报被拒绝（HTTP {c}）"),
    }
}

/// 下发后检测「是否真的提交」，没提交则补回车（见 PendingSubmit）。
/// 每轮扫描调用一次：拿本轮各会话最新用户提示词，与待确认表逐条比对。
fn check_pending_submits(state: &SharedState, tasks: &[Task]) {
    let mut guard = PENDING_SUBMITS.lock().unwrap();
    let Some(pending) = guard.as_mut() else { return };
    if pending.is_empty() {
        return;
    }
    let now = now_ms();
    let task_of: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    // 「已提交」有两种样子，缺一不可：
    //   · 会话最新用户消息就是它 —— claude 空闲，输入已被接受；
    //   · 它还躺在终端的原生排队里 —— claude 正忙，输入进了队列。回车同样是生效了的。
    // 早先只认前者，于是向正在跑的会话发消息必然误判：排队项不会成为用户消息
    //（scanner 对 queue-operation 一律不产出简报），submit_landed 恒为 false，每条都要
    // 白补满 MAX_RESUBMIT 次回车 —— 那几下若落在权限确认框上就是替人误确认。
    let landed = |t: &Task, text: &str| {
        submit_landed(&t.prompt, text) || t.queued_inputs.iter().any(|q| submit_landed(q, text))
    };
    pending.retain(|sid, p| {
        match task_of.get(sid.as_str()) {
            // 已被接受或已进排队 → 提交成功，清除
            Some(t) if landed(t, &p.text) => false,
            // 会话在本轮扫描里（能确认它当前状态），且最新消息不是这条 → 没提交
            Some(_) => {
                if now.saturating_sub(p.last_ms) < RESUBMIT_WAIT_MS {
                    return true; // 还没到补发时机，继续等
                }
                if p.retries >= MAX_RESUBMIT {
                    crate::state::client_log(&format!(
                        "补回车 {} 次后会话 {sid} 仍未提交，放弃",
                        p.retries
                    ));
                    return false;
                }
                // 经桥接补一个回车（空文本 → 扩展只送一个提交回车）
                let sent =
                    crate::bridge::send_via_extension(&state.config.data_dir, p.shell_pid, "", true);
                p.retries += 1;
                p.last_ms = now;
                crate::state::client_log(&format!(
                    "检测到会话 {sid} 只换行未提交，补回车（第 {} 次，发送={sent}）",
                    p.retries
                ));
                true
            }
            // 本轮没扫到该会话（配对暂缺/已结束）：无从确认，超时(~10s)后放弃，不盲补
            None => now.saturating_sub(p.last_ms) < 5 * RESUBMIT_WAIT_MS,
        }
    });
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
        // 父会话与子会话记录都没变化、且没过最长寿命，才复用缓存
        let sub_ms = scanner.subagents_mtime(&t.id);
        let max_age =
            std::time::Duration::from_millis(am_core::scanner::SUBAGENT_SETTLE_MS);
        if let Some((mtime, subs, at, msgs)) = cache.inner.get(&t.id) {
            if *mtime == t.mtime_ms && *subs == sub_ms && at.elapsed() < max_age {
                t.recent_messages = msgs.clone();
                continue;
            }
        }
        if let Ok(msgs) = scanner.messages(&t.id, 80) {
            cache.inner.insert(
                t.id.clone(),
                (t.mtime_ms, sub_ms, std::time::Instant::now(), msgs.clone()),
            );
            t.recent_messages = msgs;
        }
    }
    // 清理消失的会话
    let alive: std::collections::HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    cache.inner.retain(|k, _| alive.contains(k.as_str()));
}

/// 会话**此刻**的工作目录（现读它的 jsonl 尾部）。
///
/// 每轮上报里这类请求通常 0～2 条，逐条加锁的开销可以忽略；换来的是「解析发生在
/// 用它的那一刻」——扫描循环被 App Nap 压到一两分钟一轮也不影响定位准确性。
async fn session_root_now(state: &crate::state::AppState, task_id: &str) -> Option<String> {
    if task_id.is_empty() {
        return None;
    }
    state.scanner.lock().await.session_cwd_now(task_id)
}

/// `root` + 相对子路径（子路径用 '/' 分隔，按本机分隔符拼回去）。
///
/// 只做拼接，不做越界校验 —— 调用方随后走 `safe_upload_dir_within`，那里才是权威闸门。
fn join_rel(root: &str, rel: &str) -> String {
    let rel = rel.trim().trim_matches('/');
    if rel.is_empty() {
        return root.to_string();
    }
    let sep = if root.contains('\\') { '\\' } else { '/' };
    let joined: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    format!(
        "{}{sep}{}",
        root.trim_end_matches(['/', '\\']),
        joined.join(&sep.to_string())
    )
}

/// 写入 hub 下发的文件到本机目标目录。
///
/// 返回值是给 hub 的回报（`transfer_id` 为空 = hub 没要求回报，恒为 None）：**落盘名的
/// 决定权在这里**，撞名会改名（见 `unique_target`），hub 拼进任务正文的路径得跟着改，
/// 否则指向的是目录里那个同名旧文件。失败也回报，别让 hub 干等到超时。
///
/// 分片只在最后一片落完时回报一次（名字是第 0 片定的），中途失败即刻回报 —— 后续片
/// 还会来，但 hub 那边按 transfer_id 覆盖，后到的不会把已知的失败翻回成功。
fn write_transfer(
    f: &am_core::model::FileTransfer,
    session_dirs: &[std::path::PathBuf],
) -> Option<am_core::model::FileTransferResult> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let report = |ok: bool, path: String, err: String| -> Option<am_core::model::FileTransferResult> {
        (!f.transfer_id.is_empty()).then(|| am_core::model::FileTransferResult {
            transfer_id: f.transfer_id.clone(),
            path,
            ok,
            err,
        })
    };
    // 这条链路的成败必须落进 client.log：GUI 客户端的 tracing 输出没人看得到，
    // 写失败时是彻底静默的 —— 网页说「已上传」、路径也回填进了输入框，终端却报文件
    // 不存在，从两头都查不出原因。落盘的路径也一并记上，改名后到底叫什么一目了然。
    let Ok(bytes) = B64.decode(f.content_b64.as_bytes()) else {
        crate::state::client_log(&format!("下发文件内容解码失败：{}", f.filename));
        return report(false, String::new(), "内容解码失败".into());
    };
    // 目标目录按本机的允许范围复验：不能只信 hub 校验过——
    // hub 的 upload_root 是另一台机器的，且响应链路一旦被篡改就等于本机任意写。
    // 允许写进家目录，或任一活跃会话的项目目录（项目常不在家目录下）。
    let dir = match crate::state::safe_upload_dir_within(&f.dir, session_dirs) {
        Ok(d) => d,
        Err(e) => {
            crate::state::client_log(&format!(
                "拒绝写入下发文件 {}：{e}（目标目录 {}，不在本机允许范围内）",
                f.filename, f.dir
            ));
            return report(false, String::new(), format!("目标目录不在允许范围内：{e}"));
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::state::client_log(&format!("创建下发目录失败 {}：{e}", dir.display()));
        return report(false, String::new(), format!("创建目录失败：{e}"));
    }
    let safe = std::path::Path::new(&f.filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file.bin".into());

    // 分片：第 0 片建/截断，其余追加。hub 的下发队列是 FIFO、agent 也按序处理，
    // 所以顺序有保证，不必在文件里按 offset 定位。
    //
    // chunk_total 为 0 或 1 都当整份处理 —— 0 是旧版 hub（没有这个字段）落到的默认值。
    let chunked = f.chunk_total > 1;
    // 落盘路径：同名不覆盖，改名成 `a (1).png`（见 unique_target）。
    // 分片只在第 0 片定名，后续片必须落回同一个文件（见 CHUNK_TARGETS）。
    let key = format!("{}|{}", f.dir, safe);
    let target = if !chunked || f.chunk_index == 0 {
        let t = unique_target(&dir, &safe);
        if chunked {
            let mut g = CHUNK_TARGETS.lock().unwrap();
            g.get_or_insert_with(HashMap::new).insert(key.clone(), t.clone());
        }
        t
    } else {
        // 取不到（客户端在传输中途重启过）就退回原名：宁可写到原名去，也不要把这一片
        // 丢进一个凭空另起的文件里 —— 那种残片没人认得出来，只会在目录里越积越多。
        CHUNK_TARGETS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(&key).cloned())
            .unwrap_or_else(|| dir.join(&safe))
    };
    let res = if !chunked || f.chunk_index == 0 {
        std::fs::write(&target, &bytes)
    } else {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .and_then(|mut fh| fh.write_all(&bytes))
    };
    let outcome = match res {
        // 记落盘全路径：撞名会改名（见 unique_target），回填进输入框的却是上游算的名字，
        // 两者对不上时终端就会报「文件不存在」—— 有这行才看得出到底叫什么、落在哪。
        Ok(_) if !chunked => {
            crate::state::client_log(&format!("已写入下发文件：{}", target.display()));
            report(true, target.display().to_string(), String::new())
        }
        Ok(_) if f.chunk_index + 1 >= f.chunk_total => {
            crate::state::client_log(&format!(
                "已写入下发文件（{} 片）：{}",
                f.chunk_total,
                target.display()
            ));
            report(true, target.display().to_string(), String::new())
        }
        // 中间片：名字已定但内容还没齐，此时报路径会让 hub 把半个文件当成品拼进任务
        Ok(_) => None,
        // 中途某片失败就别再追加了：后续分片会接在残缺内容后面，拼出一个看着"成功"
        // 却是坏的文件。协议是单向下发，没有回执通道能叫停后续分片——但至少能把失败
        // 回报上去，让 hub 别再等这次传输。
        Err(e) => {
            crate::state::client_log(&format!(
                "写入下发文件失败（第 {}/{} 片，目标 {}）：{e}",
                f.chunk_index + 1,
                f.chunk_total.max(1),
                target.display()
            ));
            report(false, String::new(), format!("写盘失败：{e}"))
        }
    };
    // 最后一片落完就撤掉登记，免得这张表随传输次数一直长
    if chunked && f.chunk_index + 1 >= f.chunk_total {
        if let Some(m) = CHUNK_TARGETS.lock().unwrap().as_mut() {
            m.remove(&key);
        }
    }
    outcome
}

/// 目标目录下取一个不会撞名的路径：已存在就在扩展名前挂序号，`a.png` → `a (1).png`。
///
/// 原来是直接 `fs::write` 覆盖 —— 上传一个同名文件，目标目录里那份就没了，且毫无提示。
/// 传上去的多半是「刚改过的同一个文件」或「另一批同名图片」，两种情形下被悄悄抹掉的
/// 都可能是还需要的东西。
///
/// 序号格式跟资源管理器、浏览器下载一致，一眼能看出是同名文件的第几份。
/// 上限一万：真排到那儿说明目录已经不对劲了，再找下去不如退回原名，别在这儿空转。
fn unique_target(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let target = dir.join(name);
    if !target.exists() {
        return target;
    }
    let p = std::path::Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // 扩展名连点一起带上；没有扩展名（Makefile、LICENSE）就是空串，序号直接缀在末尾
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for i in 1..10_000u32 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    target
}

/// 执行 hub 下发的控制命令。
/// `known_pids` 是本轮本机扫描出的会话 pid 集合——只对这些 pid 动手，
/// 不无条件信任 hub 响应（响应链路若被中间人篡改，否则可对任意进程发信号）。
async fn execute(
    state: &SharedState,
    cmd: ControlCmd,
    known_pids: &std::collections::HashSet<u32>,
    ide_shell_of: &std::collections::HashMap<u32, u32>,
) {
    let Some(pid) = cmd.pid else {
        // 会话没配对到进程（前端显示为「Claude Code / 等待输入」这类占位标题）时 pid 为空，
        // 命令无处可投——网页却已提示「下发成功」。落盘让这种「发了没反应」可查。
        crate::state::client_log(&format!(
            "命令缺少 pid，跳过（任务 {}，会话可能尚未配对到终端进程）",
            cmd.task_id
        ));
        return;
    };
    if !known_pids.contains(&pid) {
        // 落盘可见日志：GUI 应用的 stderr(tracing) 看不到，注入失败要能在 client.log 查到。
        // 「网页提示下发成功、终端却没收到」多半就是这里——pid 不在本机本轮扫描到的会话集合
        // （会话未配对到进程 / pid 已变 / 该会话在别的设备）。
        crate::state::client_log(&format!(
            "拒绝执行输入：pid={pid} 不在本机当前会话集合（任务 {}，本机已知 {} 个会话 pid）",
            cmd.task_id,
            known_pids.len()
        ));
        return;
    }
    // 输入注入（发布任务）单独处理
    if matches!(cmd.action, am_core::model::ControlAction::Input) {
        let from_select = cmd.from_select;
        let text = cmd.text.unwrap_or_default();
        let preview: String = text.chars().take(20).collect();
        // 选项作答不补提交回车。它是一串纯序号按键（"14"/"24"/"136"），最后一个数字已经是
        // 「提交/下一题」键 —— 再补一个回车就落到翻页后的下一题上，把它按默认高亮项答掉。
        // 实测：下发「14」（选项1 + 下一题），第二题当场被那个回车替人选了默认项。
        //
        // 只认纯数字：选择卡的「自行输入」发的是文本，那条路仍要回车才提交得了。
        // from_select 已经限定了这是在回答选择卡，此时纯数字不会是别的东西。
        let submit = !(from_select && !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()));
        // 目标是 Cursor/VSCode 内嵌终端（ConPTY/编辑器内置，注入不进去）、且有活着的桥接
        // 扩展在管这个终端，就把任务写进文件桥交给扩展 terminal.sendText 送达（全平台）。
        // 终端 shell pid 用扫描时已算好的终端锚（与配对同锚）；本轮没扫到（罕见）再退回
        // ide_shell_pid() 现算，保证不漏。
        if let Some(shell_pid) =
            ide_shell_of.get(&pid).copied().or_else(|| am_core::process::ide_shell_pid(pid))
        {
            let live = crate::bridge::has_live_terminal(&state.config.data_dir, shell_pid);
            crate::state::client_log(&format!(
                "桥接判定：会话 {} claude pid={pid} → 内嵌终端 shell pid={shell_pid}，扩展在管={live}",
                cmd.task_id
            ));
            if live
                && crate::bridge::send_via_extension(&state.config.data_dir, shell_pid, &text, submit)
            {
                crate::state::client_log(&format!(
                    "注入输入：经 Cursor/VSCode 扩展桥接（终端 pid={shell_pid}，{preview}…）"
                ));
                // 记一笔待确认提交：下一轮扫描若该会话没出现这条用户消息，就确认没提交、补回车。
                // 只对桥接（Cursor 内嵌终端）路径记——它才有粘贴态吞回车的问题。
                //
                // 选择卡的作答绝不能记（from_select）：补回车的判据是「会话最新用户消息不是
                // 刚发的那条 ⇒ 没提交」，而作答（一个「1」或自定义答案）永远不会成为一条用户
                // 消息 —— 判据恒成立，于是必补满 MAX_RESUBMIT 次。那两个回车正好落在下一题上，
                // 替人确认了默认高亮项：答完第一题，后面的题就被自动答完、卡片跟着消失。
                // 线上抓到过：08:47:16 下发「1」，18.1 秒补第一个回车，20.1 秒补第二个。
                // 选择卡本也不需要这道保险：作答走的是按键（选项序号 + 下一题键，见 web 的
                // TerminalFeed），压根没有「文字进了输入框却没提交」那回事。
                if !text.trim().is_empty() && !from_select {
                    let mut g = PENDING_SUBMITS.lock().unwrap();
                    g.get_or_insert_with(HashMap::new).insert(
                        cmd.task_id.clone(),
                        PendingSubmit {
                            text: text.trim().to_string(),
                            shell_pid,
                            last_ms: now_ms(),
                            retries: 0,
                        },
                    );
                }
                return;
            }
        }
        // send_input 在 macOS 上走 osascript，会遍历 Terminal/iTerm 的每个窗口与标签页，
        // 常态就要数秒，终端处于模态/无响应时还可能一直挂着 —— 绝不能占住 async worker。
        let res =
            tokio::task::spawn_blocking(move || am_core::process::send_input_ex(pid, &text, submit))
                .await;
        match res {
            Ok(Ok(m)) => crate::state::client_log(&format!(
                "注入输入成功：pid={pid} {m}（{preview}…）"
            )),
            Ok(Err(e)) => crate::state::client_log(&format!("注入输入失败：pid={pid} {e}")),
            Err(e) => crate::state::client_log(&format!("注入输入阻塞任务异常：pid={pid} {e}")),
        }
        return;
    }
    // 终端按键注入（撤回排队 ↑ / 打断 Esc / 选择卡的 Tab+回车）单独处理
    if matches!(cmd.action, am_core::model::ControlAction::TermKey) {
        let spec = cmd.text.unwrap_or_default();
        // 内嵌终端（Cursor/VSCode）同样注入不进按键 —— 那头是 ConPTY，
        // WriteConsoleInput「可能报错、也可能成功却没送达」。输入早就改走桥接了，
        // 按键这条却一直没有，于是在编辑器里跑的会话上，撤回/打断/选择卡提交
        // 统统石沉大海。扩展只会发文本，所以把键名还原成终端本就认的控制字符发过去。
        if let Some(chars) = am_core::process::key_spec_to_chars(&spec) {
            if let Some(shell_pid) =
                ide_shell_of.get(&pid).copied().or_else(|| am_core::process::ide_shell_pid(pid))
            {
                if crate::bridge::has_live_terminal(&state.config.data_dir, shell_pid)
                    // submit=false：这串本身就是按键，补回车会多出一下
                    && crate::bridge::send_via_extension(
                        &state.config.data_dir,
                        shell_pid,
                        &chars,
                        false,
                    )
                {
                    crate::state::client_log(&format!(
                        "注入按键：经 Cursor/VSCode 扩展桥接（终端 pid={shell_pid}，{spec}）"
                    ));
                    return;
                }
            }
        }
        let spec_log = spec.clone();
        let res =
            tokio::task::spawn_blocking(move || am_core::process::send_terminal_keys(pid, &spec))
                .await;
        let spec = spec_log;
        match res {
            Ok(Ok(m)) => crate::state::client_log(&format!("注入按键成功：pid={pid} {spec} {m}")),
            Ok(Err(e)) => crate::state::client_log(&format!("注入按键失败：pid={pid} {spec} {e}")),
            Err(e) => crate::state::client_log(&format!("注入按键阻塞任务异常：pid={pid} {e}")),
        }
        return;
    }
    // 「中断当前任务」＝按 Esc。内嵌终端里进程级的中断根本递不进 TUI，
    // 与按键走同一条桥接才送得到（Windows 上 control() 内部也已改成发 Esc）。
    if matches!(cmd.action, am_core::model::ControlAction::Interrupt) {
        if let Some(shell_pid) =
            ide_shell_of.get(&pid).copied().or_else(|| am_core::process::ide_shell_pid(pid))
        {
            if crate::bridge::has_live_terminal(&state.config.data_dir, shell_pid)
                && crate::bridge::send_via_extension(
                    &state.config.data_dir,
                    shell_pid,
                    "\x1b",
                    false,
                )
            {
                crate::state::client_log(&format!(
                    "中断当前任务：经 Cursor/VSCode 扩展桥接发 Esc（终端 pid={shell_pid}）"
                ));
                return;
            }
        }
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
            // 落盘，别只 tracing。注入输入/按键的成败一直写 client.log，唯独控制类
            // 命令没有 —— 于是「点了没反应」时日志里一片空白，连它到底执行没执行都看不出来。
            crate::state::client_log(&format!(
                "执行控制命令成功：pid={pid} {label}（任务 {}）",
                cmd.task_id
            ));
        }
        Err(e) => crate::state::client_log(&format!(
            "执行控制命令失败：pid={pid} {:?} {e}（任务 {}）",
            cmd.action, cmd.task_id
        )),
    }
}

#[cfg(test)]
mod reject_tests {
    use super::*;

    /// 配置类错误必须给出可据以行动的话，而不是笼统的「连接中…」
    #[test]
    fn actionable_messages_for_config_errors() {
        // 401 空 body：得说清「重新绑定」这条出路。原先这里断言的是
        // 「核对 AM_AGENT_TOKEN」——那是单用户时代的主因，如今 401 多半是
        // 设备令牌失效（换了 hub / 设备被删），指去查一个多数机器上没设的
        // 环境变量只会带偏（这条测试当初就把错误文案给焊死了）。
        let m = describe_reject(401, "");
        assert!(m.contains("登录"), "401 应指出怎么重新绑定: {m}");

        // 401 有 body：hub 的原话比本地猜测准，必须原样带出
        let m = describe_reject(401, "设备未绑定账号：打开客户端窗口登录一次即可自动绑定");
        assert!(m.contains("设备未绑定账号"), "401 应带出 hub 的原因: {m}");

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


/// 执行会话目录内的文件夹操作（新建/删除/重命名）。全程用 canonicalize 卡在会话根内，
/// 越权/非法一律拒绝。返回 (成功, 提示语)。
fn run_fs_op(op: &am_core::model::FsOp) -> (bool, String) {
    use std::path::Path;
    let bad = |n: &str| n.is_empty() || n.contains('/') || n.contains('\\') || n == "." || n == "..";
    if bad(&op.name) {
        return (false, "非法名称".into());
    }
    if op.rel.split('/').any(|s| s == "..") {
        return (false, "非法路径".into());
    }
    let root = Path::new(&op.cwd);
    let Ok(canon_root) = root.canonicalize() else {
        return (false, "会话目录不可用".into());
    };
    // 目标所在目录（rel）必须存在且在根内
    let base = root.join(op.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let Ok(canon_base) = base.canonicalize() else {
        return (false, "目录不存在".into());
    };
    if !canon_base.starts_with(&canon_root) {
        return (false, "越权目录".into());
    }
    match op.op.as_str() {
        "mkdir" => {
            let target = canon_base.join(&op.name);
            if target.exists() {
                return (false, "同名已存在".into());
            }
            match std::fs::create_dir(&target) {
                Ok(_) => (true, "已新建文件夹".into()),
                Err(e) => (false, format!("新建失败：{e}")),
            }
        }
        "delete" => {
            let target = canon_base.join(&op.name);
            let Ok(ct) = target.canonicalize() else {
                return (false, "不存在".into());
            };
            // 不允许删根本身，且必须在根内
            if ct == canon_root || !ct.starts_with(&canon_root) {
                return (false, "越权目录".into());
            }
            let r = if ct.is_dir() {
                std::fs::remove_dir_all(&ct)
            } else {
                std::fs::remove_file(&ct)
            };
            match r {
                Ok(_) => (true, "已删除".into()),
                Err(e) => (false, format!("删除失败：{e}")),
            }
        }
        "rename" => {
            if bad(&op.new_name) {
                return (false, "非法新名称".into());
            }
            let target = canon_base.join(&op.name);
            let Ok(ct) = target.canonicalize() else {
                return (false, "不存在".into());
            };
            if ct == canon_root || !ct.starts_with(&canon_root) {
                return (false, "越权目录".into());
            }
            let dst = canon_base.join(&op.new_name);
            if dst.exists() {
                return (false, "同名已存在".into());
            }
            match std::fs::rename(&ct, &dst) {
                Ok(_) => (true, "已重命名".into()),
                Err(e) => (false, format!("重命名失败：{e}")),
            }
        }
        _ => (false, "未知操作".into()),
    }
}

/// 列出 root/rel 下的子目录名（仅目录；防越出 root；隐藏目录排后；上限 300）
/// 列出 root/rel 下的子目录与文件（各自排序，隐藏项靠后）。
/// 越出根或读取失败时返回两个空表。
/// 现取上限。整份要经 base64 塞进上报体，再由 hub 中转给网页，太大三头都难受。
const MAX_FETCH_BYTES: u64 = 10 * 1024 * 1024;

/// 读会话目录里的一个文件，回给 hub 中转（网页据此显示 agent 输出里引用的截图）。
///
/// **只允许会话目录内的文件**：canonicalize 后必须仍在 cwd 之下。会话内容里的路径
/// 不可全信 —— 一句 `![](../../.ssh/id_rsa)` 就能把目录外的东西读走。判据与
/// [`list_entries`] 同源。
fn read_session_file(q: &am_core::model::FileFetch) -> am_core::model::FileFetchResult {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::path::Path;
    let fail = |e: &str| am_core::model::FileFetchResult {
        fetch_id: q.fetch_id.clone(),
        err: e.to_string(),
        mime: String::new(),
        content_b64: String::new(),
    };
    if q.rel.split(['/', '\\']).any(|s| s == "..") {
        return fail("非法路径");
    }
    let base = Path::new(&q.cwd).join(q.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let (Ok(file), Ok(root)) = (base.canonicalize(), Path::new(&q.cwd).canonicalize()) else {
        return fail("文件不存在");
    };
    if !file.starts_with(&root) {
        return fail("越出会话目录");
    }
    let Ok(meta) = std::fs::metadata(&file) else {
        return fail("读不到文件");
    };
    if !meta.is_file() {
        return fail("不是文件");
    }
    if meta.len() > MAX_FETCH_BYTES {
        return fail(&format!("文件过大（{} MB）", meta.len() / 1024 / 1024));
    }
    match std::fs::read(&file) {
        Ok(bytes) => am_core::model::FileFetchResult {
            fetch_id: q.fetch_id.clone(),
            err: String::new(),
            mime: image_mime(&bytes).to_string(),
            content_b64: B64.encode(&bytes),
        },
        Err(e) => fail(&e.to_string()),
    }
}

/// 按魔数判图片类型。**不看扩展名** —— 扩展名是内容里写的，改个名就能让页面按别的
/// 类型解析；魔数是文件自己说的。认不出就给 octet-stream（页面不会当图片渲染）。
fn image_mime(b: &[u8]) -> &'static str {
    match b {
        _ if b.starts_with(b"\x89PNG") => "image/png",
        _ if b.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
        _ if b.starts_with(b"GIF8") => "image/gif",
        _ if b.starts_with(b"RIFF") && b.len() > 11 && &b[8..12] == b"WEBP" => "image/webp",
        _ if b.starts_with(b"<svg") || b.starts_with(b"<?xml") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn list_entries(root: &str, rel: &str) -> (Vec<String>, Vec<String>) {
    use std::path::Path;
    let empty = || (Vec::new(), Vec::new());
    // rel 已在 hub 侧拒绝 ".."，这里再兜一层
    if rel.split('/').any(|s| s == "..") {
        return empty();
    }
    let base = Path::new(root).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let (Ok(canon_base), Ok(canon_root)) = (base.canonicalize(), Path::new(root).canonicalize())
    else {
        return empty();
    };
    if !canon_base.starts_with(&canon_root) {
        return empty();
    }
    let Ok(rd) = std::fs::read_dir(&canon_base) else {
        return empty();
    };
    // 常规项在前、隐藏项在后，各自字典序
    let order = |v: &mut Vec<String>| {
        v.sort_by(|a, b| {
            (a.starts_with('.'), a.to_lowercase()).cmp(&(b.starts_with('.'), b.to_lowercase()))
        });
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for e in rd.filter_map(|e| e.ok()).take(1000) {
        let Ok(name) = e.file_name().into_string() else { continue };
        match e.file_type() {
            Ok(t) if t.is_dir() => dirs.push(name),
            // 符号链接等按文件处理，够用即可
            Ok(_) => files.push(name),
            Err(_) => {}
        }
        if dirs.len() >= 300 && files.len() >= 300 {
            break;
        }
    }
    dirs.truncate(300);
    files.truncate(300);
    order(&mut dirs);
    order(&mut files);
    (dirs, files)
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    fn fetch(cwd: &str, rel: &str) -> am_core::model::FileFetchResult {
        read_session_file(&am_core::model::FileFetch {
            fetch_id: "t".into(),
            cwd: cwd.into(),
            rel: rel.into(),
            // 这些用例验的是「根之内/之外」的边界，直接给定根，不走按会话解析
            task_id: String::new(),
            by_session: false,
        })
    }

    /// 会话内容里的路径**不可全信** —— agent 输出里一句 `![](../../.ssh/id_rsa)`
    /// 就能把会话目录外的文件读走。这道边界必须守住。
    #[test]
    fn refuses_paths_outside_session_dir() {
        let root = std::env::temp_dir().join(format!("am-fetch-{}", std::process::id()));
        let inner = root.join("sub");
        std::fs::create_dir_all(&inner).unwrap();
        // 目录外的「机密」，以及目录内的正常图片
        std::fs::write(root.parent().unwrap().join("am-outside-secret.txt"), b"secret").unwrap();
        std::fs::write(inner.join("shot.png"), b"\x89PNG\r\n\x1a\n rest").unwrap();
        let cwd = inner.to_string_lossy().to_string();

        // 正常读：认出 PNG
        let ok = fetch(&cwd, "shot.png");
        assert!(ok.err.is_empty(), "同目录文件应能读到: {}", ok.err);
        assert_eq!(ok.mime, "image/png");

        // 越界：`..` 段直接拒
        assert!(!fetch(&cwd, "../../am-outside-secret.txt").err.is_empty());
        // 目录本身不是文件
        assert!(!fetch(&cwd, ".").err.is_empty());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(root.parent().unwrap().join("am-outside-secret.txt"));
    }

    /// MIME 按魔数判，不按扩展名 —— 扩展名是内容里写的，改个名就能让页面按别的类型解析
    #[test]
    fn mime_from_magic_not_extension() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(image_mime(&[0xff, 0xd8, 0xff, 0xe0]), "image/jpeg");
        // 伪装成图片的文本：不认，页面据此不会当图片渲染
        assert_eq!(image_mime(b"#!/bin/sh\nrm -rf /"), "application/octet-stream");
    }
}

#[cfg(test)]
mod transfer_report_tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    fn transfer(dir: &std::path::Path, name: &str, body: &[u8], id: &str) -> am_core::model::FileTransfer {
        am_core::model::FileTransfer {
            dir: dir.to_string_lossy().to_string(),
            filename: name.into(),
            content_b64: B64.encode(body),
            chunk_index: 0,
            chunk_total: 0,
            transfer_id: id.into(),
            // 同上：用例直接给绝对 dir，不走按会话解析
            task_id: String::new(),
            rel_dir: String::new(),
            by_session: false,
        }
    }

    fn workdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("am-xfer-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 回报的必须是**改名后**的真实路径。
    ///
    /// 这是整条回程的存在理由：钉钉来的图片一律叫「图片.jpg」，目标目录里几乎必有同名旧图，
    /// 落盘会改成「图片 (1).jpg」。若回报原名，hub 拼进任务的路径就指向那张**旧图** ——
    /// agent 照着读得到内容、不报错，只是读的是上一版。
    #[test]
    fn reports_renamed_path_not_original() {
        let dir = workdir("rename");
        std::fs::write(dir.join("图片.jpg"), b"OLD").unwrap();
        let roots = vec![dir.clone()];

        let r = write_transfer(&transfer(&dir, "图片.jpg", b"NEW", "tid-1"), &roots)
            .expect("要求回报时必须有回报");
        assert!(r.ok, "落盘应成功：{}", r.err);
        assert_eq!(r.transfer_id, "tid-1");
        assert_eq!(
            r.path,
            dir.join("图片 (1).jpg").to_string_lossy(),
            "回报的应是改名后的路径"
        );
        // 旧图必须原封不动 —— 撞名是改名，不是覆盖
        assert_eq!(std::fs::read(dir.join("图片.jpg")).unwrap(), b"OLD");
        assert_eq!(std::fs::read(dir.join("图片 (1).jpg")).unwrap(), b"NEW");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不撞名时回报的就是原路径（hub 的预判命中，不该被当成改名）
    #[test]
    fn reports_original_path_when_no_clash() {
        let dir = workdir("noclash");
        let roots = vec![dir.clone()];

        let r = write_transfer(&transfer(&dir, "图片-2.jpg", b"NEW", "tid-2"), &roots).unwrap();
        assert!(r.ok);
        assert_eq!(r.path, dir.join("图片-2.jpg").to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// transferId 为空 = 旧 hub 没要求回报：照常落盘，但不产生回报
    #[test]
    fn silent_when_hub_did_not_ask() {
        let dir = workdir("silent");
        let roots = vec![dir.clone()];

        assert!(write_transfer(&transfer(&dir, "a.txt", b"x", ""), &roots).is_none());
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"x");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败也必须回报：hub 那头有个等结果的窗口，不回报它只能干等到超时，
    /// 再拿自己算的名字去拼路径 —— 而那个路径下根本没有文件
    #[test]
    fn reports_failure_so_hub_stops_waiting() {
        let dir = workdir("fail");
        let roots = vec![dir.clone()];
        let mut f = transfer(&dir, "bad.bin", b"", "tid-3");
        f.content_b64 = "不是合法的 base64!!".into();

        let r = write_transfer(&f, &roots).expect("失败同样要回报");
        assert!(!r.ok);
        assert_eq!(r.transfer_id, "tid-3");
        assert!(r.path.is_empty(), "失败时不该给出路径");
        assert!(!r.err.is_empty(), "要说明原因");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
