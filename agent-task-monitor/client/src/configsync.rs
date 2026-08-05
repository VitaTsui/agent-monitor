//! Claude Code / Codex 配置的跨设备同步（客户端侧）。
//!
//! 职责三件：扫出本机在管配置的指纹清单、按 hub 点名回传内容、把 hub 下发的内容落盘。
//!
//! 两条红线，改这个文件时别越过：
//! ① **同步集是客户端写死的白名单**，不接受 hub 指定路径。hub 说什么就写什么的话，
//!    服务端一旦被攻破就等于对所有设备任意写文件——而客户端跑在用户自己机器上，
//!    权限比 hub 高得多。凭据类文件（`.credentials.json`、`auth.json`）因此永远同步不了，
//!    这是故意的：它们一进 hub 就是一份明文的账号副本。
//! ② **落盘一律「备份 + 原子写」**，不用 `agent::write_transfer` 那条整份覆盖的路径。
//!    这些是用户自己攒的 CLAUDE.md / agents，被无声盖掉找不回来是不可接受的。

use am_core::configpath::{
    is_allowed, is_syncable_field, looks_machine_specific, merge_hooks, merge_mcp, portable_hooks,
    portable_mcp, is_root_config, DIRS, EXEC_DIRS, MAX_FILE_BYTES, SINGLE_FILES,
};
use am_core::model::{
    ConfigFileBody, ConfigFileMeta, ConfigKeyInfo, ConfigManifest, ConfigPatch, ConfigProbe,
    ConfigPush, ConfigSkip,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 单次扫描的文件数上限：`skills/` 是目录树，深了可能几百个文件。
/// 超出部分直接不进清单（宁可少同步，也不让一次心跳背上巨量指纹）。
const MAX_FILES: usize = 500;

/// 目录递归深度上限，防符号链接成环把扫描卡死
const MAX_DEPTH: usize = 8;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 相对路径 → 本机绝对路径。第一段决定落在哪个根下（`claude` → `~/.claude`）。
///
/// `is_allowed` 是 hub 与本机文件系统之间唯一的闸门，**落盘前必须过这一关**。
///
/// 清单里存相对路径而非绝对路径，正是为了这一步：mac 的 `/Users/vita/...` 推到
/// Windows 机器上既拼不出目标位置，也会把用户名泄露给同账号的其它设备。
pub fn abs_path(home: &Path, rel: &str) -> Option<PathBuf> {
    if !is_allowed(rel) {
        return None;
    }
    let (root, rest) = rel.split_once('/')?;
    let mut out = match root {
        "claude" => home.join(".claude"),
        "codex" => home.join(".codex"),
        _ => return None,
    };
    for seg in rest.split('/') {
        out.push(seg);
    }
    Some(out)
}

/// 绝对路径 → 同步集相对路径（扫描时用）
fn rel_of(home: &Path, abs: &Path) -> Option<String> {
    for (root, prefix) in [(home.join(".claude"), "claude"), (home.join(".codex"), "codex")] {
        if let Ok(sub) = abs.strip_prefix(&root) {
            let sub = sub.to_string_lossy().replace('\\', "/");
            let rel = format!("{prefix}/{sub}");
            return is_allowed(&rel).then_some(rel);
        }
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 本机配置扫描器。带 (mtime, size) → 哈希 的缓存：文件没动就不重读、不重算。
///
/// 扫描本身挂在 30s 一次的节拍上（见 `agent::report_loop`），而心跳是 1.5s 一轮——
/// 每轮都读一遍磁盘纯属浪费，尤其 `skills/` 是个目录树。
#[derive(Default)]
pub struct ConfigScanner {
    /// rel → (mtime, size, sha256)
    cache: HashMap<String, (u64, u64, String)>,
}

impl ConfigScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 扫出本机在管配置的指纹清单。任何读失败都当「这个文件不存在」跳过——
    /// 配置同步再重要也不该让上报循环报错中断。
    pub fn scan(&mut self, home: &Path) -> ConfigManifest {
        let mut paths: Vec<PathBuf> = Vec::new();
        for rel in SINGLE_FILES {
            if let Some(p) = abs_path(home, rel) {
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        for d in DIRS {
            let Some(dir) = dir_abs(home, d) else { continue };
            collect_files(&dir, 0, false, &mut paths);
        }
        // 脚本目录：不限扩展名（hook 脚本通常没有扩展名）
        for d in EXEC_DIRS {
            let Some(dir) = dir_abs(home, d) else { continue };
            collect_files(&dir, 0, true, &mut paths);
        }
        // MCP / hook 引用到的根层配置文件。**只收被引用的** ——
        // 不是把 .claude/ 根下所有 json 都搬走，那里面可能有别的工具塞的东西。
        for rel in referenced_configs(home) {
            if let Some(p) = abs_path(home, &rel) {
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        paths.sort();
        paths.truncate(MAX_FILES);

        let mut files = Vec::with_capacity(paths.len());
        let mut next_cache = HashMap::with_capacity(paths.len());
        for p in paths {
            let Some(rel) = rel_of(home, &p) else { continue };
            let Ok(meta) = std::fs::metadata(&p) else { continue };
            let size = meta.len();
            if size > MAX_FILE_BYTES {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // 缓存命中：(mtime, size) 都没变就沿用旧哈希，不再读文件内容
            let sha = match self.cache.get(&rel) {
                Some((m, s, sha)) if *m == mtime && *s == size => sha.clone(),
                _ => match std::fs::read(&p) {
                    Ok(bytes) => sha256_hex(&bytes),
                    Err(_) => continue,
                },
            };
            next_cache.insert(rel.clone(), (mtime, size, sha.clone()));
            files.push(ConfigFileMeta { path: rel, sha256: sha, size, mtime });
        }
        self.cache = next_cache;
        ConfigManifest { files, scanned_at: now_secs() }
    }
}

/// 一个字符串若指向 `~/.claude/` 根层的文件，返回它的同步集相对路径。
/// 认两种写法：`~/.claude/x.json` 与展开后的 `<home>/.claude/x.json`。
fn to_claude_rel(s: &str, home: &Path) -> Option<String> {
    let home_prefix = format!("{}/.claude/", home.to_string_lossy().replace('\\', "/"));
    let normalized = s.replace('\\', "/");
    let rest = normalized
        .strip_prefix("~/.claude/")
        .or_else(|| normalized.strip_prefix(home_prefix.as_str()))?;
    let rel = format!("claude/{rest}");
    is_root_config(&rel).then_some(rel)
}

/// 递归找出 JSON 里所有指向 `.claude/` 根层配置文件的字符串
fn collect_refs(v: &serde_json::Value, home: &Path, out: &mut std::collections::HashSet<String>) {
    match v {
        serde_json::Value::String(s) => {
            if let Some(rel) = to_claude_rel(s, home) {
                out.insert(rel);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_refs(x, home, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_refs(x, home, out)),
        _ => {}
    }
}

/// MCP 与 hook 配置里引用到的根层配置文件（`--config ~/.claude/xxx.json` 这类）。
///
/// 只收**被引用的**：把 `.claude/` 根下所有 json 一股脑搬走太粗暴，
/// 那里可能有别的工具塞进来的东西。
fn referenced_configs(home: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for (_, rel) in PROBE_FILES {
        let Some(v) = read_structured(&home.join(rel), rel) else { continue };
        // 只看这两个字段，不扫整份文件（.claude.json 里还有项目历史等无关内容）
        for key in ["mcpServers", "hooks"] {
            if let Some(section) = v.get(key) {
                collect_refs(section, home, &mut out);
            }
        }
    }
    out
}

fn dir_abs(home: &Path, rel_dir: &str) -> Option<PathBuf> {
    let (root, rest) = rel_dir.split_once('/')?;
    let base = match root {
        "claude" => home.join(".claude"),
        "codex" => home.join(".codex"),
        _ => return None,
    };
    Some(base.join(rest))
}

/// 递归收集目录下的文件。`any_ext` 为真时不限扩展名（脚本目录用）。
/// 跟随符号链接会成环，靠深度上限兜住。
fn collect_files(dir: &Path, depth: usize, any_ext: bool, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_files(&p, depth + 1, any_ext, out),
            Ok(_) if any_ext || name.ends_with(".md") => out.push(p),
            _ => {}
        }
    }
}

/// 单轮回传的文件数与总字节预算。上报是 1.5s 一轮的心跳，把首同步的几百个文件
/// 一次性塞进去会直接撞上 hub 的请求体上限（表现为 413，见 `describe_reject`）。
/// 超出的部分留到下一轮——hub 每轮都会把还缺的重新点名，不会漏。
const MAX_BODIES_PER_ROUND: usize = 3;
const MAX_ROUND_BYTES: usize = 512 * 1024;

/// 按 hub 点名读出文件内容回传。逐个校验 `is_allowed`：
/// hub 点名的路径同样不可信，不能因为「是 hub 要的」就读任意文件传出去。
pub fn read_bodies(home: &Path, paths: &[String]) -> Vec<ConfigFileBody> {
    let mut out = Vec::new();
    let mut budget = MAX_ROUND_BYTES;
    for rel in paths {
        if out.len() >= MAX_BODIES_PER_ROUND {
            break;
        }
        let Some(abs) = abs_path(home, rel) else {
            tracing::warn!("拒绝回传非同步集配置: {rel}");
            continue;
        };
        let Ok(meta) = std::fs::metadata(&abs) else { continue };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&abs) else { continue };
        let encoded = B64.encode(&bytes);
        // 预算按 base64 后的长度算：编码会膨胀 1/3，按原文算会低估请求体
        if encoded.len() > budget && !out.is_empty() {
            break;
        }
        budget = budget.saturating_sub(encoded.len());
        out.push(ConfigFileBody {
            path: rel.clone(),
            sha256: sha256_hex(&bytes),
            content_b64: encoded,
        });
    }
    out
}

/// 把 hub 下发的配置落盘，返回实际写成功的份数。
///
/// 每份都要过三关：路径在同步集内、内容哈希对得上、体积没超限。
/// 写法照搬 `hookrec::ensure_hook_config`——先备份 `.am-bak` 再原子 rename，
/// 中途失败留下的是完好的旧文件，而不是半截新文件。
pub fn apply(home: &Path, pushes: &[ConfigPush]) -> usize {
    let mut done = 0;
    for push in pushes {
        let Some(target) = abs_path(home, &push.path) else {
            tracing::warn!("拒绝写入非同步集配置: {}", push.path);
            continue;
        };
        let Ok(bytes) = B64.decode(push.content_b64.as_bytes()) else {
            tracing::warn!("配置内容解码失败: {}", push.path);
            continue;
        };
        if bytes.len() as u64 > MAX_FILE_BYTES {
            tracing::warn!("配置文件超限，跳过: {}", push.path);
            continue;
        }
        // 哈希复验：传输被截断时写进去的是半截文件，而配置文件的半截往往仍能解析，
        // 用户不会收到任何报错，只会发现自己的 agent 少了一半。
        let actual = sha256_hex(&bytes);
        if actual != push.sha256 {
            tracing::warn!("配置内容哈希不符，跳过: {}（期望 {}）", push.path, push.sha256);
            continue;
        }
        // 本机已经是这份内容就别写了：否则每次下发都刷新 mtime，
        // 下一轮扫描看到 mtime 变了又要重算哈希，白白抖动。
        if std::fs::read(&target).map(|b| b == bytes).unwrap_or(false) {
            done += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建配置目录失败 {}: {e}", parent.display());
                continue;
            }
        }
        // 备份：只备份「已存在的旧文件」，新增文件没有可备份的
        if target.exists() {
            let bak = with_suffix(&target, ".am-bak");
            let _ = std::fs::copy(&target, &bak);
        }
        let tmp = with_suffix(&target, ".am-tmp");
        if std::fs::write(&tmp, &bytes).is_err() {
            tracing::warn!("写入配置临时文件失败: {}", push.path);
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("替换配置文件失败 {}: {e}", push.path);
            continue;
        }
        // 脚本落地后要能直接跑：hook 是被 Claude Code 直接 exec 的，
        // 少了执行位就是「文件在、一触发就 Permission denied」，比没同步更难查。
        #[cfg(unix)]
        if am_core::configpath::is_exec_path(&push.path) {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&target) {
                let mut perm = meta.permissions();
                perm.set_mode(perm.mode() | 0o755);
                let _ = std::fs::set_permissions(&target, perm);
            }
        }
        tracing::info!("已同步配置: {}", push.path);
        done += 1;
    }
    done
}

// ───────────────────────── 结构化配置的字段普查（二期准备）─────────────────────────
//
// 只读出「有哪些字段、什么类型、值里是否含本机路径」，**绝不传值**。
// 二期要对 settings.json 做字段级合并，白名单必须建立在用户实际用了哪些字段之上；
// 凭空猜一份白名单，等于拿猜测去改用户的配置文件。

/// 普查的目标文件：(文件标识, 相对 home 的路径)
const PROBE_FILES: &[(&str, &str)] = &[
    ("claude/settings.json", ".claude/settings.json"),
    // MCP 服务器配置在这里。文件本身几十 KB，混着 OAuth 凭据与逐项目历史——
    // 只有 `mcpServers` 一个键在白名单内，其余读都不读、更不上传（见 core 的 CLAUDE_JSON_SYNC_KEYS）
    ("claude/claude.json", ".claude.json"),
    ("codex/config.toml", ".codex/config.toml"),
];

/// 单份文件最多记多少个字段（防异常巨大的配置把上报撑爆）
const MAX_PROBE_KEYS: usize = 200;
/// 键路径最大深度：顶层 + 两层子键足够看清结构
const MAX_PROBE_DEPTH: usize = 2;

/// 普查本机的结构化配置。读不到/解析不了的文件直接跳过——
/// 普查是二期的准备工作，不该让任何一份坏配置影响上报循环。
pub fn probe(home: &Path) -> Vec<ConfigProbe> {
    let mut out = Vec::new();
    for (id, rel) in PROBE_FILES {
        let path = home.join(rel);
        let Ok(txt) = std::fs::read_to_string(&path) else { continue };
        let value = if rel.ends_with(".toml") {
            toml::from_str::<serde_json::Value>(&txt).ok()
        } else {
            serde_json::from_str::<serde_json::Value>(&txt).ok()
        };
        let Some(value) = value else {
            tracing::warn!("配置普查跳过（解析失败）: {id}");
            continue;
        };
        let mut keys = Vec::new();
        walk_keys(&value, "", 0, &mut keys);
        if !keys.is_empty() {
            out.push(ConfigProbe { file: (*id).to_string(), keys });
        }
    }
    out
}

fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn walk_keys(v: &serde_json::Value, prefix: &str, depth: usize, out: &mut Vec<ConfigKeyInfo>) {
    let serde_json::Value::Object(map) = v else { return };
    for (k, val) in map {
        if out.len() >= MAX_PROBE_KEYS {
            return;
        }
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        let len = match val {
            serde_json::Value::Array(a) => a.len(),
            serde_json::Value::Object(o) => o.len(),
            _ => 0,
        };
        out.push(ConfigKeyInfo {
            path: path.clone(),
            ty: type_name(val).to_string(),
            len,
            machine_specific: looks_machine_specific(val),
        });
        if depth < MAX_PROBE_DEPTH {
            walk_keys(val, &path, depth + 1, out);
        }
    }
}

// ───────────────────── 结构化配置的字段级同步（二期）─────────────────────

/// 读出本机结构化配置里**白名单内、且值不含本机路径**的字段。
///
/// 两道闸都在这里：字段名过 `is_syncable_field`，值再过 `looks_machine_specific`。
/// 后者不能省——字段名对了，用户仍可能在里面填了一个绝对路径（那是完全合法的写法），
/// 同步过去只会让另一台机器指向一个不存在的位置。
pub fn read_patches(home: &Path) -> Vec<ConfigPatch> {
    let mut out = Vec::new();
    for (id, rel) in PROBE_FILES {
        let Some(value) = read_structured(&home.join(rel), rel) else { continue };
        let serde_json::Value::Object(map) = value else { continue };
        let mut fields = std::collections::BTreeMap::new();
        for (k, v) in map {
            if !is_syncable_field(id, &k) {
                continue;
            }
            // hooks 要拆开：整份带走会把本客户端自管的配对 hook（命令是本机 exe 绝对路径）
            // 一并外传，落到别的机器上就是一条永远执行失败的 hook。
            let v = if k == "hooks" {
                match portable_hooks(&v) {
                    Some(p) => p,
                    None => continue,
                }
            } else if k == "mcpServers" {
                // 剥 env（凭据不外传）+ home 绝对路径归一化成 ~，见 core 的 portable_mcp
                match portable_mcp(&v, &home.to_string_lossy()) {
                    Some(p) => p,
                    None => continue,
                }
            } else if looks_machine_specific(&v) {
                tracing::debug!("配置字段 {id}:{k} 含本机路径，不参与同步");
                continue;
            } else {
                v
            };
            fields.insert(k, v);
        }
        if !fields.is_empty() {
            out.push(ConfigPatch { file: (*id).to_string(), fields });
        }
    }
    out
}

/// 读一份结构化配置（按扩展名选解析器）。读不到/解析不了都返回 None。
fn read_structured(path: &Path, rel: &str) -> Option<serde_json::Value> {
    let txt = std::fs::read_to_string(path).ok()?;
    if rel.ends_with(".toml") {
        toml::from_str(&txt).ok()
    } else {
        serde_json::from_str(&txt).ok()
    }
}

/// 把 hub 下发的字段合并进本机结构化配置，返回实际改动的文件数。
///
/// **只覆盖白名单字段，绝不整份替换**：用户手写的一切（尤其 `hooks` 里那条命令指向
/// 本机 exe 的配对 hook）原样保留。落盘前 dry-run 校验合并结果仍能解析且顶层是对象——
/// settings.json 写坏了 Claude Code 会直接起不来，这个代价远高于「这次没同步上」。
///
/// 目前只处理 JSON。Codex 的 config.toml 回写需要 `toml_edit`（`toml` crate 序列化会吞掉
/// 用户的注释与字段顺序），留到下一步单独做，这里遇到 .toml 直接跳过。
pub fn apply_patches(home: &Path, patches: &[ConfigPatch]) -> (usize, Vec<ConfigSkip>) {
    let mut changed = 0;
    let mut skips: Vec<ConfigSkip> = Vec::new();
    for patch in patches {
        let Some(rel) = PROBE_FILES.iter().find(|(id, _)| *id == patch.file).map(|(_, r)| *r) else {
            tracing::warn!("拒绝合并未知配置文件: {}", patch.file);
            continue;
        };
        let target = home.join(rel);
        if rel.ends_with(".toml") {
            if apply_toml_patch(&target, patch) {
                changed += 1;
            }
            continue;
        }
        // 本机还没有这个文件就不去创建：凭空造一个 settings.json 可能改变
        // Claude Code 的默认行为，而用户从没要求过我们创建它。
        let Some(mut root) = read_structured(&target, rel) else { continue };
        if !root.is_object() {
            continue;
        }
        // 改动前的快照，供下面「只有白名单字段变了」的自检比对
        let original = root.clone();

        let mut dirty = false;
        {
            let Some(obj) = root.as_object_mut() else { continue };
            for (k, v) in &patch.fields {
                if !is_syncable_field(&patch.file, k) {
                    tracing::warn!("拒绝合并字段 {}:{k}", patch.file);
                    continue;
                }
                // hooks 是合并而非覆盖：本机自管的配对 hook 与指向本机脚本的 hook
                // 原样留下，只有「通用」条目由配置源接管（见 core 的 merge_hooks）
                // 「跑不起来就不写」的过滤一律**只作用于下发内容**，绝不碰本机原有的东西。
                // 作用在合并产物上是错的：本机自己配的绝对路径 hook、以及本客户端写入的
                // 配对 hook（安装路径一旦含空格，取第一个 token 就判成不存在）会被连坐删掉。
                let next = if k == "hooks" {
                    let mut inc = v.clone();
                    for (event, cmd) in drop_unrunnable_hooks(&mut inc, home) {
                        skips.push(ConfigSkip {
                            file: patch.file.clone(),
                            item: format!("{event}: {cmd}"),
                            reason: "脚本不在本机".into(),
                        });
                    }
                    merge_hooks(obj.get(k), &inc)
                } else if k == "mcpServers" {
                    // 先剔掉本机跑不起来的：对端没装那个二进制、或 server 引用的配置文件
                    // 没跟过来（这些文件不在同步集里）。跨平台尤其常见 ——
                    // mac 的 ~/.local/bin/xxx 在 Windows 上根本不存在。
                    // 照写只是搬来一份注定连接失败的配置。
                    let mut inc = v.clone();
                    for (name, why) in drop_unrunnable_mcp(&mut inc, home) {
                        skips.push(ConfigSkip {
                            file: patch.file.clone(),
                            item: name,
                            reason: why,
                        });
                    }
                    // 再 ~ 展开成本机 home，并保留本机原有的 env 与本机独有的 server
                    merge_mcp(obj.get(k), &inc, &home.to_string_lossy())
                } else if looks_machine_specific(v) {
                    tracing::warn!("拒绝合并字段 {}:{k}", patch.file);
                    continue;
                } else {
                    v.clone()
                };
                if obj.get(k) != Some(&next) {
                    obj.insert(k.clone(), next);
                    dirty = true;
                }
            }
        }
        if !dirty {
            continue;
        }

        // dry-run：序列化 + 重新解析，确认产物仍是合法且顶层为对象的 JSON
        let Ok(out) = serde_json::to_string_pretty(&root) else { continue };
        let Ok(reparsed) = serde_json::from_str::<serde_json::Value>(&out) else {
            tracing::warn!("合并结果无法解析，跳过写入: {}", patch.file);
            continue;
        };
        // 「只动该动的」自检：把重新解析的产物与原文逐个顶层键比对，除白名单字段外
        // 必须**完全相等**。`~/.claude.json` 有几十 KB，装着 OAuth 凭据与逐项目历史，
        // 而我们是整份反序列化再序列化写回 —— 任何精度丢失或结构走样都会毁掉它，
        // 用户得重新登录 Claude Code。与其事后发现，不如这里挡住。
        if !only_expected_changed(&original, &reparsed, &patch.fields) {
            tracing::warn!("合并影响了白名单以外的内容，跳过写入: {}", patch.file);
            continue;
        }

        let bak = with_suffix(&target, ".am-bak");
        let _ = std::fs::copy(&target, &bak);
        let tmp = with_suffix(&target, ".am-tmp");
        if std::fs::write(&tmp, out.as_bytes()).is_err() {
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("替换配置文件失败 {}: {e}", patch.file);
            continue;
        }
        tracing::info!("已同步配置字段: {} ({} 项)", patch.file, patch.fields.len());
        changed += 1;
    }
    (changed, skips)
}

/// 这个字符串看起来是不是一个文件路径（而非包名/参数）。
///
/// `@playwright/mcp@0.0.70` 含 `/` 却是包名，不能当路径查 —— 只认以 `/`、`~/`、`./` 开头的。
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("~/") || s.starts_with("./")
}

/// `~/x/y` → 本机绝对路径。分隔符一并归一化：`home.join(rest)` 不会动 rest 里的 `/`，
/// 在 Windows 上会拼出 `C:\Users\你\.local/bin/x` 这种混合形态 —— 能用，但写进用户的
/// 配置文件里既难看又容易在别处出岔子。
fn expand_home(s: &str, home: &Path) -> std::path::PathBuf {
    let Some(rest) = s.strip_prefix("~/") else { return std::path::PathBuf::from(s) };
    let mut p = home.to_path_buf();
    for seg in rest.split('/') {
        p.push(seg);
    }
    p
}

/// Windows 上的可执行文件后缀
fn exec_exts() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

/// 本机常见的可执行文件安装位置。按文件名兜底查找时用。
fn common_bin_dirs(home: &Path) -> Vec<std::path::PathBuf> {
    let mut v = vec![home.join(".local").join("bin"), home.join("bin"), home.join(".cargo").join("bin")];
    if cfg!(windows) {
        for (var, sub) in [("LOCALAPPDATA", "Programs"), ("APPDATA", "npm"), ("LOCALAPPDATA", "")] {
            if let Some(base) = std::env::var_os(var) {
                let p = std::path::PathBuf::from(base);
                v.push(if sub.is_empty() { p } else { p.join(sub) });
            }
        }
    } else {
        v.extend([
            std::path::PathBuf::from("/opt/homebrew/bin"),
            std::path::PathBuf::from("/usr/local/bin"),
            std::path::PathBuf::from("/usr/bin"),
        ]);
    }
    v
}

/// 按**文件名**在本机找这个可执行文件的真实位置。
///
/// 同步来的路径是**源机**上的位置（mac 的 `~/.local/bin/x`），同一个工具在目标机器上
/// 完全可能装在别处（Windows 的 `%LOCALAPPDATA%\Programs\x.exe`）。只展开 `~` 是不够的 ——
/// 那只是把源机的目录结构原样套过来，落到对端就是个不存在的位置。
///
/// 先查 PATH（最权威，用户怎么装的就怎么找得到），再查几个常见安装目录。
/// 按名字匹配理论上可能撞上同名的别的程序，但 MCP server 的名字都相当特异
/// （`codebase-memory-mcp` 这种），而代价那边是「写一个必然连不上的路径」—— 值得。
fn resolve_by_name(name: &str, home: &Path) -> Option<std::path::PathBuf> {
    if name.is_empty() {
        return None;
    }
    if let Some(p) = which_in_path(name) {
        return Some(p);
    }
    common_bin_dirs(home).into_iter().find_map(|d| resolve_exec(&d.join(name)))
}

/// 找出这个路径对应的**实际**可执行文件。
///
/// Windows 上可执行文件带后缀：配置里写的 `~/.local/bin/foo` 在那边实际是 `foo.exe`。
/// 只查无后缀的话，装了也会被判成没装 —— 而且就算判过了，Claude Code 照原样 exec
/// 同样起不来。所以这里返回真实存在的那个路径，供回写进配置。
fn resolve_exec(p: &Path) -> Option<std::path::PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    for ext in exec_exts() {
        let mut name = p.as_os_str().to_os_string();
        name.push(&ext);
        let q = std::path::PathBuf::from(name);
        if q.is_file() {
            return Some(q);
        }
    }
    None
}

/// 某个 MCP server 在本机是否真的跑得起来：可执行文件在、且它引用的文件也在。
/// 跑得起来时，把 command 就地改写成**实际解析到的路径**（Windows 上可能补了 .exe）。
fn mcp_runnable(cfg: &mut serde_json::Value, home: &Path) -> Result<(), String> {
    if let Some(cmd) = cfg.get("command").and_then(|c| c.as_str()).map(str::to_string) {
        if looks_like_path(&cmd) {
            let p = expand_home(&cmd, home);
            // 先按原路径找（同系统之间通常直接命中），再退回**按文件名**在本机找 ——
            // 跨系统时源机那个路径在这里根本不成立，得看这台机器把它装在哪。
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let resolved =
                resolve_exec(&p).or_else(|| resolve_by_name(&name, home));
            match resolved {
                Some(actual) => {
                    // 回写**本机实际路径**：跨系统同步过来的源机路径就在这里被换成本地的
                    if let Some(o) = cfg.as_object_mut() {
                        o.insert(
                            "command".into(),
                            serde_json::Value::String(actual.to_string_lossy().to_string()),
                        );
                    }
                }
                None => {
                    return Err(format!(
                        "本机找不到可执行文件「{name}」（PATH 与常见安装目录都没有）"
                    ))
                }
            }
        } else if which_in_path(&cmd).is_none() {
            // 裸命令（npx / uvx / docker …）：PATH 里找不到就跑不起来
            return Err(format!("命令不在 PATH 里: {cmd}"));
        }
    }
    let args: Vec<String> = cfg
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    for a in &args {
        if looks_like_path(a) {
            let p = expand_home(a, home);
            if !p.exists() {
                return Err(format!("引用的文件不存在: {}", p.display()));
            }
        }
    }
    Ok(())
}

/// 在 PATH 里找一个命令（不依赖外部 which 进程）。
///
/// Windows 必须带上 PATHEXT：那边 `npx` 实际是 `npx.cmd`、`node` 是 `node.exe`，
/// 只按裸名字找必然找不到 —— 会把从 mac 同步过去的 `npx` 类 server 全部误判成「跑不起来」。
fn which_in_path(cmd: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    std::env::split_paths(&path).find_map(|dir| {
        let direct = dir.join(cmd);
        if direct.is_file() {
            return Some(direct);
        }
        exts.iter().find_map(|e| {
            let p = dir.join(format!("{cmd}{e}"));
            p.is_file().then_some(p)
        })
    })
}

/// 剔除本机跑不起来的 MCP server，返回被剔除的 (名字, 原因)。
///
/// 不这么做的话，同步只是把一份**注定连不上**的配置搬过来：对端没装那个二进制、
/// 或者 server 引用的配置文件没跟过来，Claude Code 每次启动都连接失败 ——
/// 用户看到的是「同步完 MCP 就坏了」，比不同步还糟。
fn drop_unrunnable_mcp(v: &mut serde_json::Value, home: &Path) -> Vec<(String, String)> {
    let Some(obj) = v.as_object_mut() else { return Vec::new() };
    let mut dropped = Vec::new();
    // 注意 mcp_runnable 会**就地改写** command 为实际解析到的路径
    // （Windows 上补 .exe、分隔符归一化），所以这里传 &mut
    obj.retain(|name, cfg| match mcp_runnable(cfg, home) {
        Ok(()) => true,
        Err(why) => {
            dropped.push((name.clone(), why));
            false
        }
    });
    dropped
}

/// 剔除本机跑不起来的 hook（脚本文件没跟过来），返回被剔除的 (事件, 命令)。
///
/// 与 MCP 同一个道理：hook 脚本本身不在同步集里，对端没有那个文件时，
/// 同步过去只会让它**每次触发都报错**。跨平台更明显 —— mac 的
/// `~/.claude/hooks/xxx` 是 shell 脚本，Windows 上往往根本没有。
///
/// 判断刻意保守：只查「命令第一个 token 是路径」的情况。hook 是交给 shell 跑的，
/// 裸命令可能来自别名/函数/临时 PATH，查不到不代表跑不了，不能当作剔除依据。
fn drop_unrunnable_hooks(v: &mut serde_json::Value, home: &Path) -> Vec<(String, String)> {
    let Some(events) = v.as_object_mut() else { return Vec::new() };
    let mut dropped = Vec::new();
    for (event, entries) in events.iter_mut() {
        let Some(arr) = entries.as_array_mut() else { continue };
        for entry in arr.iter_mut() {
            let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
                continue;
            };
            hooks.retain(|h| {
                let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else { return true };
                let first = cmd.split_whitespace().next().unwrap_or("");
                if !looks_like_path(first) {
                    return true;
                }
                let p = match first.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => std::path::PathBuf::from(first),
                };
                if p.exists() {
                    true
                } else {
                    dropped.push((event.clone(), cmd.to_string()));
                    false
                }
            });
        }
        // 内层清空的条目要一并摘掉，别留下空壳
        arr.retain(|e| e.get("hooks").and_then(|h| h.as_array()).map(|a| !a.is_empty()).unwrap_or(true));
    }
    events.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    dropped
}

/// 合并产物是否「只动了该动的」：顶层键集合不变，且除 `changed_keys` 外的每个键
/// 都与原文全等。
///
/// 这是写回大配置文件（尤其 `~/.claude.json`）前的最后一道闸：那里面有 OAuth 凭据，
/// 一次数值精度丢失或结构走样就要用户重新登录。
fn only_expected_changed(
    original: &serde_json::Value,
    merged: &serde_json::Value,
    changed_keys: &std::collections::BTreeMap<String, serde_json::Value>,
) -> bool {
    let (Some(a), Some(b)) = (original.as_object(), merged.as_object()) else {
        return false;
    };
    if a.len() != b.len() {
        return false;
    }
    a.iter().all(|(k, v)| match b.get(k) {
        Some(nv) => changed_keys.contains_key(k) || nv == v,
        None => false,
    })
}

/// 把白名单字段合并进 TOML 文件，返回是否有改动。
///
/// 用 `toml_edit` 而不是 `toml`：后者是「解析成数据结构再重新序列化」，用户写在
/// config.toml 里的**注释与字段顺序会被整个吞掉** —— 文件还能用，但用户下次打开会发现
/// 自己的注释没了，这种破坏比报错更糟。`toml_edit` 保留原文格式，只改动到的那一处。
fn apply_toml_patch(target: &Path, patch: &ConfigPatch) -> bool {
    let Ok(txt) = std::fs::read_to_string(target) else { return false };
    let Ok(mut doc) = txt.parse::<toml_edit::DocumentMut>() else {
        tracing::warn!("解析失败，跳过合并: {}", patch.file);
        return false;
    };

    let mut dirty = false;
    for (k, v) in &patch.fields {
        // hub 下发的字段同样不可信：与 JSON 分支同样的两道闸
        if !is_syncable_field(&patch.file, k) || looks_machine_specific(v) {
            tracing::warn!("拒绝合并字段 {}:{k}", patch.file);
            continue;
        }
        let Some(new_val) = json_to_toml(v) else {
            // 复合结构映射到 TOML 有多种合法写法（内联表 / 独立表段），
            // 猜错就会改乱用户的文件结构。当前白名单只有标量，遇到复合直接跳过。
            tracing::warn!("配置项 {}:{k} 不是标量，暂不支持同步", patch.file);
            continue;
        };
        dirty |= set_scalar(&mut doc, k, new_val);
    }
    if !dirty {
        return false;
    }

    // dry-run：产物必须仍能解析成 TOML
    let out = doc.to_string();
    if out.parse::<toml_edit::DocumentMut>().is_err() {
        tracing::warn!("合并结果自检失败，跳过写入: {}", patch.file);
        return false;
    }

    let bak = with_suffix(target, ".am-bak");
    let _ = std::fs::copy(target, &bak);
    let tmp = with_suffix(target, ".am-tmp");
    if std::fs::write(&tmp, out.as_bytes()).is_err() {
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        tracing::warn!("替换配置文件失败 {}: {e}", patch.file);
        return false;
    }
    tracing::info!("已同步配置字段: {} ({} 项)", patch.file, patch.fields.len());
    true
}

/// 就地替换一个标量字段的**值**，返回是否有改动。
///
/// 关键在于只换值、把原有的 decor（前后缀空白与注释）搬回去。直接 `doc[k] = value(..)`
/// 是替换整个 Item，会把 `model = "x"   # 主模型` 里的行尾注释一起丢掉 ——
/// 单测 apply_toml_preserves_comments_and_order 就是抓这个的。
fn set_scalar(doc: &mut toml_edit::DocumentMut, key: &str, new_val: toml_edit::Value) -> bool {
    match doc.get_mut(key).and_then(|i| i.as_value_mut()) {
        Some(slot) => {
            if slot.to_string().trim() == new_val.to_string().trim() {
                return false;
            }
            let decor = slot.decor().clone();
            *slot = new_val;
            *slot.decor_mut() = decor;
            true
        }
        // 本机原本没有这个字段：直接追加，没有 decor 可保留
        None => {
            doc[key] = toml_edit::Item::Value(new_val);
            true
        }
    }
}

/// JSON 标量 → TOML 值。复合结构返回 None（见 `apply_toml_patch` 里的说明）。
fn json_to_toml(v: &serde_json::Value) -> Option<toml_edit::Value> {
    match v {
        serde_json::Value::String(s) => Some(s.as_str().into()),
        serde_json::Value::Bool(b) => Some((*b).into()),
        serde_json::Value::Number(n) => {
            n.as_i64().map(Into::into).or_else(|| n.as_f64().map(Into::into))
        }
        _ => None,
    }
}

/// 在**完整文件名**后追加后缀（`x.md` → `x.md.am-bak`）。
///
/// 不用 `Path::with_extension`：那会把 `x.md` 变成 `x.am-bak`，
/// 备份文件反过来占掉一个合法的 .md 位置，下一轮扫描把它也当成配置同步出去。
fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 白名单本身的用例在 am_core::configpath —— 规则只有那一份，测试也只放那一份

    #[test]
    fn abs_path_maps_to_right_root() {
        let home = PathBuf::from("/home/u");
        assert_eq!(
            abs_path(&home, "claude/agents/a.md"),
            Some(PathBuf::from("/home/u/.claude/agents/a.md"))
        );
        assert_eq!(
            abs_path(&home, "codex/AGENTS.md"),
            Some(PathBuf::from("/home/u/.codex/AGENTS.md"))
        );
        assert_eq!(abs_path(&home, "claude/settings.json"), None);
    }

    #[test]
    fn backup_suffix_keeps_original_extension() {
        // x.md.am-bak，而不是 x.am-bak —— 后者会被当成新的配置文件同步出去
        let p = with_suffix(Path::new("/a/x.md"), ".am-bak");
        assert_eq!(p, PathBuf::from("/a/x.md.am-bak"));
        assert!(!is_allowed("claude/agents/x.md.am-bak"));
    }

    #[test]
    fn apply_rejects_hash_mismatch() {
        let dir = std::env::temp_dir().join(format!("am-cfgsync-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude/agents"));
        let pushes = vec![ConfigPush {
            path: "claude/agents/a.md".into(),
            content_b64: B64.encode(b"hello"),
            sha256: "deadbeef".into(),
        }];
        assert_eq!(apply(&dir, &pushes), 0);
        assert!(!dir.join(".claude/agents/a.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_writes_and_backs_up() {
        let dir = std::env::temp_dir().join(format!("am-cfgsync-w-{}", std::process::id()));
        let agents = dir.join(".claude/agents");
        let _ = std::fs::create_dir_all(&agents);
        let _ = std::fs::write(agents.join("a.md"), b"old");

        let body = b"new content";
        let pushes = vec![ConfigPush {
            path: "claude/agents/a.md".into(),
            content_b64: B64.encode(body),
            sha256: sha256_hex(body),
        }];
        assert_eq!(apply(&dir, &pushes), 1);
        assert_eq!(std::fs::read(agents.join("a.md")).unwrap(), body);
        // 旧内容留在备份里
        assert_eq!(std::fs::read(agents.join("a.md.am-bak")).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_reports_structure_without_values() {
        let dir = std::env::temp_dir().join(format!("am-cfgsync-p-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let _ = std::fs::create_dir_all(dir.join(".codex"));
        let _ = std::fs::write(
            dir.join(".claude/settings.json"),
            br#"{
              "model": "opus",
              "apiKeyHelper": "/Users/someone/bin/key.sh",
              "permissions": {"allow": ["Bash(ls:*)"], "deny": []},
              "hooks": {"PreToolUse": [{"hooks": [{"command": "/opt/am/agent-monitor hook"}]}]}
            }"#,
        );
        let _ = std::fs::write(dir.join(".codex/config.toml"), b"model = \"gpt\"\n[tui]\ntheme = \"dark\"\n");

        let probes = probe(&dir);
        let settings = probes.iter().find(|p| p.file == "claude/settings.json").expect("有 settings");
        let by = |p: &str| settings.keys.iter().find(|k| k.path == p).cloned();

        // 结构被记录
        assert_eq!(by("model").unwrap().ty, "string");
        assert_eq!(by("permissions").unwrap().ty, "object");
        assert_eq!(by("permissions.allow").unwrap().len, 1);

        // 机器相关性：本机路径要被标出来（二期白名单据此排除）
        assert!(by("apiKeyHelper").unwrap().machine_specific);
        // 嵌在数组深处的 hook 命令同样要被抓到
        assert!(by("hooks").unwrap().machine_specific);
        assert!(!by("model").unwrap().machine_specific);
        assert!(!by("permissions").unwrap().machine_specific);

        // 最要紧的一条：普查结果里**不能出现任何值**
        let dump = serde_json::to_string(&probes).unwrap();
        assert!(!dump.contains("opus"), "普查泄露了值: {dump}");
        assert!(!dump.contains("key.sh"), "普查泄露了值: {dump}");
        assert!(!dump.contains("Bash(ls"), "普查泄露了值: {dump}");

        // TOML 也能普查
        let codex = probes.iter().find(|p| p.file == "codex/config.toml").expect("有 config.toml");
        assert!(codex.keys.iter().any(|k| k.path == "tui.theme"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn patch_of(file: &str, pairs: &[(&str, serde_json::Value)]) -> ConfigPatch {
        ConfigPatch {
            file: file.into(),
            fields: pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }

    #[test]
    fn read_patches_takes_whitelisted_only() {
        let dir = std::env::temp_dir().join(format!("am-cfg-rp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let _ = std::fs::write(
            dir.join(".claude/settings.json"),
            br#"{"model":"opus","apiKeyHelper":"/Users/x/k.sh","hooks":{"a":1},"tui":"dark"}"#,
        );
        let patches = read_patches(&dir);
        let p = patches.iter().find(|p| p.file == "claude/settings.json").unwrap();
        assert_eq!(p.fields.keys().collect::<Vec<_>>(), vec!["model"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_patches_skips_machine_specific_values() {
        let dir = std::env::temp_dir().join(format!("am-cfg-rpm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        // 白名单字段，但值是本机路径 —— 不该被带走
        let _ = std::fs::write(
            dir.join(".claude/settings.json"),
            br#"{"model":"/Users/vita/custom-model"}"#,
        );
        assert!(read_patches(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_patches_preserves_hooks_and_user_fields() {
        let dir = std::env::temp_dir().join(format!("am-cfg-ap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let original = r#"{
  "model": "sonnet",
  "hooks": {"PreToolUse": [{"hooks": [{"command": "/opt/am/agent-monitor hook"}]}]},
  "myOwnField": {"deep": [1, 2, 3]}
}"#;
        let p = dir.join(".claude/settings.json");
        let _ = std::fs::write(&p, original);

        let (n, _) = apply_patches(&dir, &[patch_of("claude/settings.json", &[("model", json!("opus"))])]);
        assert_eq!(n, 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        // 白名单字段被更新
        assert_eq!(after["model"], json!("opus"));
        // 配对 hook 与用户自己的字段**原样保留** —— 这条挂了就是二期最危险的回归
        assert_eq!(
            after["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            json!("/opt/am/agent-monitor hook")
        );
        assert_eq!(after["myOwnField"], json!({"deep": [1, 2, 3]}));
        // 旧版留在备份里
        assert!(std::fs::read_to_string(dir.join(".claude/settings.json.am-bak"))
            .unwrap()
            .contains("sonnet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_patches_rejects_blacklisted_and_machine_specific() {
        let dir = std::env::temp_dir().join(format!("am-cfg-apr-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let p = dir.join(".claude/settings.json");
        let _ = std::fs::write(&p, br#"{"model":"sonnet","hooks":{"keep":"me"}}"#);

        // hub 下发黑名单字段 + 白名单字段但值是本机路径 —— 两者都必须被拒
        let (n, _) = apply_patches(
            &dir,
            &[patch_of(
                "claude/settings.json",
                &[("hooks", json!({"evil": "x"})), ("model", json!("/Users/attacker/m"))],
            )],
        );
        assert_eq!(n, 0, "不该有任何写入");
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(after["hooks"], json!({"keep": "me"}));
        assert_eq!(after["model"], json!("sonnet"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_toml_preserves_comments_and_order() {
        let dir = std::env::temp_dir().join(format!("am-cfg-toml-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".codex"));
        let p = dir.join(".codex/config.toml");
        // 注释、字段顺序、表段 —— 用 `toml` crate 回写会把这些全吞掉
        let original = r#"# 我的 Codex 配置
# 别乱改

model = "gpt-5"        # 主模型
approval_policy = "on-request"

[tui]
# 界面主题
theme = "dark"
"#;
        let _ = std::fs::write(&p, original);

        let (n, _) = apply_patches(&dir, &[patch_of("codex/config.toml", &[("model", json!("o3"))])]);
        assert_eq!(n, 1);

        let after = std::fs::read_to_string(&p).unwrap();
        // 值改了
        assert!(after.contains("model = \"o3\""), "model 没改: {after}");
        assert!(!after.contains("gpt-5"));
        // 注释一条都不能少
        assert!(after.contains("# 我的 Codex 配置"), "顶部注释丢了: {after}");
        assert!(after.contains("# 别乱改"));
        assert!(after.contains("# 主模型"), "行尾注释丢了: {after}");
        assert!(after.contains("# 界面主题"));
        // 其它字段与表段原样保留
        assert!(after.contains("approval_policy = \"on-request\""));
        assert!(after.contains("[tui]"));
        assert!(after.contains("theme = \"dark\""));
        // 顺序不变：model 仍在 approval_policy 之前，[tui] 仍在最后
        let i_model = after.find("model").unwrap();
        let i_policy = after.find("approval_policy").unwrap();
        let i_tui = after.find("[tui]").unwrap();
        assert!(i_model < i_policy && i_policy < i_tui, "字段顺序被打乱: {after}");
        // 旧版进备份
        assert!(std::fs::read_to_string(dir.join(".codex/config.toml.am-bak"))
            .unwrap()
            .contains("gpt-5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_toml_rejects_blacklisted_and_non_scalar() {
        let dir = std::env::temp_dir().join(format!("am-cfg-tomlr-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".codex"));
        let p = dir.join(".codex/config.toml");
        let _ = std::fs::write(&p, "model = \"gpt-5\"\n[tui]\ntheme = \"dark\"\n");

        // 黑名单字段 + 复合值：都不该落地
        let (n, _) = apply_patches(
            &dir,
            &[patch_of(
                "codex/config.toml",
                &[("tui", json!({"theme": "light"})), ("env", json!({"X": "1"}))],
            )],
        );
        assert_eq!(n, 0);
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("theme = \"dark\""), "用户的表段被动了: {after}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_merge_preserves_rest_of_claude_json() {
        let dir = std::env::temp_dir().join(format!("am-cfg-mcp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".local/bin"));
        // 目标二进制必须真实存在，否则会被「跑不起来就不写」的过滤剔除
        let _ = std::fs::write(dir.join(".local/bin/cbm"), b"#!/bin/sh\n");
        let p = dir.join(".claude.json");
        // 仿真实文件：MCP 之外还有凭据与历史，一个字节都不能动
        let original = serde_json::json!({
            "oauthAccount": { "accountUuid": "abc-123", "emailAddress": "me@example.com" },
            "mcpServers": { "old": { "command": "/Users/me/.local/bin/old" } },
            "projects": { "/Users/me/work": { "lastCost": 1.2345678901234567_f64 } },
            "numberOfStartups": 4321
        });
        let _ = std::fs::write(&p, serde_json::to_string_pretty(&original).unwrap());

        let incoming = patch_of(
            "claude/claude.json",
            &[("mcpServers", json!({ "cbm": { "command": "~/.local/bin/cbm" } }))],
        );
        assert_eq!(apply_patches(&dir, &[incoming]).0, 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        // ~ 展开成本机 home
        assert_eq!(
            after["mcpServers"]["cbm"]["command"],
            json!(format!("{}/.local/bin/cbm", dir.to_string_lossy()))
        );
        // 凭据与历史原封不动 —— 这条挂了就意味着用户要重新登录 Claude Code
        assert_eq!(after["oauthAccount"], original["oauthAccount"]);
        assert_eq!(after["projects"], original["projects"]);
        assert_eq!(after["numberOfStartups"], json!(4321));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrunnable_hooks_are_dropped() {
        let dir = std::env::temp_dir().join(format!("am-cfg-hookrun-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude/hooks"));
        let _ = std::fs::write(dir.join(".claude/hooks/present"), b"#!/bin/sh\n");
        // 本机已有一条配对 hook，合并后必须还在
        let _ = std::fs::write(
            dir.join(".claude/settings.json"),
            br#"{"hooks":{"SessionStart":[{"hooks":[{"command":"/Applications/X.app/agent-monitor hook"}]}]}}"#,
        );

        let incoming = patch_of(
            "claude/settings.json",
            &[(
                "hooks",
                json!({
                    "PreToolUse": [
                        { "matcher": "*", "hooks": [
                            { "command": "~/.claude/hooks/present --flag" },
                            { "command": "~/.claude/hooks/absent" }
                        ]}
                    ],
                    // 整条都跑不起来 → 事件应被整个摘掉，不留空壳
                    "Stop": [{ "hooks": [{ "command": "~/.claude/hooks/gone" }] }],
                    // 裸命令不查 PATH（hook 走 shell，别名/函数都可能） → 保留
                    "SubagentStart": [{ "hooks": [{ "command": "npx prettier --write" }] }]
                }),
            )],
        );
        assert_eq!(apply_patches(&dir, &[incoming]).0, 1);

        let after: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        let dump = serde_json::to_string(&after).unwrap();
        assert!(dump.contains("present --flag"), "脚本在的该保留: {dump}");
        assert!(!dump.contains("absent"), "脚本不在的该剔除: {dump}");
        assert!(after["hooks"].get("Stop").is_none(), "空事件该摘掉: {dump}");
        assert!(dump.contains("npx prettier"), "裸命令不该被误杀: {dump}");
        // 本机配对 hook 依旧在
        assert!(dump.contains("agent-monitor hook"), "配对 hook 丢了: {dump}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_command_is_rewritten_to_resolved_path() {
        // 解析到的实际路径要回写进配置：Windows 上补 .exe、分隔符归一化。
        // 只判断「存在」而不回写是不够的 —— Claude Code 照原样 exec 一样起不来。
        let dir = std::env::temp_dir().join(format!("am-cfg-mcpres-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".local/bin"));
        let _ = std::fs::write(dir.join(".local/bin/tool"), b"#!/bin/sh\n");
        let _ = std::fs::write(dir.join(".claude.json"), br#"{"mcpServers":{}}"#);

        let incoming =
            patch_of("claude/claude.json", &[("mcpServers", json!({ "t": { "command": "~/.local/bin/tool" } }))]);
        assert_eq!(apply_patches(&dir, &[incoming]).0, 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).unwrap())
                .unwrap();
        let cmd = after["mcpServers"]["t"]["command"].as_str().unwrap();
        // 不再有波浪号，且指向真实存在的文件
        assert!(!cmd.contains('~'), "~ 没展开: {cmd}");
        assert!(std::path::Path::new(cmd).is_file(), "回写的路径不是真实文件: {cmd}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_resolves_to_local_install_location() {
        // 跨系统同步来的是**源机**的路径；本机把同一个工具装在别处时，
        // 要按文件名找到本地的实际位置并改写，而不是把源机的目录结构套过来。
        let dir = std::env::temp_dir().join(format!("am-cfg-byname-{}", std::process::id()));
        // 源机路径 ~/.local/bin/mytool 在本机不存在，但 ~/bin/mytool 有
        let _ = std::fs::create_dir_all(dir.join("bin"));
        let _ = std::fs::write(dir.join("bin/mytool"), b"#!/bin/sh\n");
        let _ = std::fs::write(dir.join(".claude.json"), br#"{"mcpServers":{}}"#);

        let incoming = patch_of(
            "claude/claude.json",
            &[("mcpServers", json!({ "t": { "command": "~/.local/bin/mytool" } }))],
        );
        assert_eq!(apply_patches(&dir, &[incoming]).0, 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).unwrap())
                .unwrap();
        let cmd = after["mcpServers"]["t"]["command"].as_str().unwrap();
        assert!(std::path::Path::new(cmd).is_file(), "没解析到本机实际位置: {cmd}");
        assert!(cmd.ends_with("mytool"), "解析到的不是同一个工具: {cmd}");
        assert!(!cmd.contains(".local"), "还在用源机的目录结构: {cmd}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unrunnable_mcp_is_dropped_not_written() {
        let dir = std::env::temp_dir().join(format!("am-cfg-mcprun-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".local/bin"));
        // 本机真实存在的可执行文件
        let real = dir.join(".local/bin/present");
        let _ = std::fs::write(&real, b"#!/bin/sh\n");
        let cfgfile = dir.join(".claude/cfg.json");
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let _ = std::fs::write(&cfgfile, b"{}");
        let _ = std::fs::write(dir.join(".claude.json"), br#"{"mcpServers":{}}"#);

        let incoming = patch_of(
            "claude/claude.json",
            &[(
                "mcpServers",
                json!({
                    // 存在 → 应写入
                    "ok":        { "command": "~/.local/bin/present" },
                    // 二进制不存在 → 剔除（对端没装，正是用户遇到的情况）
                    "missing":   { "command": "~/.local/bin/absent" },
                    // 命令在但引用的配置文件不存在 → 剔除
                    "badcfg":    { "command": "~/.local/bin/present",
                                   "args": ["--config", "~/.claude/nope.json"] },
                    // 命令在且引用的文件也在 → 应写入
                    "goodcfg":   { "command": "~/.local/bin/present",
                                   "args": ["--config", "~/.claude/cfg.json"] },
                    // 包名含 / 但不是路径，不该被当成文件去查
                    "pkgarg":    { "command": "~/.local/bin/present",
                                   "args": ["@playwright/mcp@0.0.70"] }
                }),
            )],
        );
        assert_eq!(apply_patches(&dir, &[incoming]).0, 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).unwrap())
                .unwrap();
        let m = after["mcpServers"].as_object().unwrap();
        assert!(m.contains_key("ok"), "可用的 server 应写入");
        assert!(m.contains_key("goodcfg"), "引用文件存在的应写入");
        assert!(m.contains_key("pkgarg"), "包名参数不该被当路径误杀: {m:?}");
        assert!(!m.contains_key("missing"), "二进制不存在的不该写入");
        assert!(!m.contains_key("badcfg"), "引用文件缺失的不该写入");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_patches_does_not_create_missing_file() {
        let dir = std::env::temp_dir().join(format!("am-cfg-apc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        let (n, _) = apply_patches(&dir, &[patch_of("claude/settings.json", &[("model", json!("opus"))])]);
        assert_eq!(n, 0);
        assert!(!dir.join(".claude/settings.json").exists(), "不该凭空创建配置文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_referenced_root_configs_are_synced() {
        let dir = std::env::temp_dir().join(format!("am-cfg-ref-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude"));
        // 被 MCP 引用 → 该同步
        let _ = std::fs::write(dir.join(".claude/playwright.mcp.config.json"), b"{}");
        // 没人引用 → 不该被搬走（.claude 根下可能有别的工具塞的东西）
        let _ = std::fs::write(dir.join(".claude/other-tool.json"), br#"{"token":"secret"}"#);
        // 设置文件走字段级同步，整份搬会把配对 hook 一起带走
        let _ = std::fs::write(dir.join(".claude/settings.json"), b"{}");
        let _ = std::fs::write(
            dir.join(".claude.json"),
            br#"{"mcpServers":{"pw":{"command":"npx","args":["--config","~/.claude/playwright.mcp.config.json"]}}}"#,
        );

        let m = ConfigScanner::new().scan(&dir);
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"claude/playwright.mcp.config.json"), "被引用的该同步: {paths:?}");
        assert!(!paths.contains(&"claude/other-tool.json"), "没被引用的不该搬走: {paths:?}");
        assert!(!paths.iter().any(|p| p.contains("settings.json")), "设置不该整份同步");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_scripts_are_scanned_and_land_executable() {
        let dir = std::env::temp_dir().join(format!("am-cfg-hookfile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude/hooks"));
        let _ = std::fs::write(dir.join(".claude/hooks/reminder"), b"#!/bin/sh\necho hi\n");
        let _ = std::fs::write(dir.join(".claude/settings.json"), b"{}");

        // 扫描：脚本进同步集，隔壁的 settings.json 不进
        let m = ConfigScanner::new().scan(&dir);
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"claude/hooks/reminder"), "脚本没被收: {paths:?}");
        assert!(!paths.iter().any(|p| p.ends_with("settings.json")), "设置不该整份同步");

        // 落盘：必须带执行位，否则 hook 一触发就是 Permission denied
        let body = b"#!/bin/sh\necho new\n";
        let push = ConfigPush {
            path: "claude/hooks/fresh".into(),
            content_b64: B64.encode(body),
            sha256: sha256_hex(body),
        };
        assert_eq!(apply(&dir, &[push]), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(".claude/hooks/fresh")).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "脚本落盘没有执行位: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_picks_up_whitelisted_only() {
        let dir = std::env::temp_dir().join(format!("am-cfgsync-s-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join(".claude/agents"));
        let _ = std::fs::create_dir_all(dir.join(".claude/projects"));
        let _ = std::fs::write(dir.join(".claude/CLAUDE.md"), b"root");
        let _ = std::fs::write(dir.join(".claude/agents/a.md"), b"agent");
        let _ = std::fs::write(dir.join(".claude/settings.json"), b"{}");
        let _ = std::fs::write(dir.join(".claude/projects/p.md"), b"session");

        let m = ConfigScanner::new().scan(&dir);
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"claude/CLAUDE.md"));
        assert!(paths.contains(&"claude/agents/a.md"));
        assert_eq!(paths.len(), 2, "只应收白名单内的两份，实际 {paths:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
