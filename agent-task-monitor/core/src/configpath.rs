//! 配置同步的**路径白名单**——客户端与 hub 共用的唯一一份规则。
//!
//! 放在 core 而不是各写一份，是因为两端都必须独立校验：客户端不信 hub 下发的路径
//! （hub 被攻破不能变成对所有设备任意写文件），hub 也不信客户端上传的路径
//! （一台被控设备不能借上传把内容塞进基线里、再由 hub 分发给同账号的其它机器）。
//! 两份实现一旦漂移，宽的那份就是实际生效的边界，所以只留这一份。

/// 单文件规则：整份纳入同步集
pub const SINGLE_FILES: &[&str] = &["claude/CLAUDE.md", "codex/AGENTS.md"];

/// 目录规则：递归收集其下的 .md（其余扩展名一律不收）
pub const DIRS: &[&str] = &["claude/agents", "claude/commands", "claude/skills", "codex/prompts"];

/// 单文件体积上限。配置类文本几 KB 顶天，超了多半是有人把日志/数据丢了进来——
/// 这种东西挤进 1.5s 一轮的心跳会把上报撑到 413。
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// 相对路径是否在同步集内。
///
/// 路径一律是归一化的相对形式（`claude/agents/x.md`），第一段是根标识
/// （`claude` → `~/.claude`，`codex` → `~/.codex`），由各端自己拼本机绝对路径。
pub fn is_allowed(rel: &str) -> bool {
    // 只收 .md：白名单目录里混着别的东西时（比如 skills 下的脚本），不碰。
    // 凭据（.credentials.json / auth.json）与设置（settings.json / config.toml）
    // 都因此天然落在同步集之外——前者进 hub 就是一份明文账号副本，
    // 后者混着机器相关内容（尤其本客户端写进 settings.json 的配对 hook，
    // 命令是本机 exe 的绝对路径），整份同步会让另一台机器的配对静默失效。
    if !rel.ends_with(".md") {
        return false;
    }
    // 路径穿越与反斜杠（Windows 上 `a\..\..\x` 同样能越出）一律拒
    if rel.contains("..") || rel.contains('\\') || rel.starts_with('/') {
        return false;
    }
    // 隐藏段不收，且挡掉空段（`a//b`）
    if rel.split('/').any(|seg| seg.is_empty() || seg.starts_with('.')) {
        return false;
    }
    SINGLE_FILES.contains(&rel) || DIRS.iter().any(|d| rel.starts_with(&format!("{d}/")))
}

// ───────────────────── 结构化配置的字段白名单（二期）─────────────────────
//
// 与上面的文件白名单是两套东西：md 类整份同步，settings.json / config.toml 只同步
// **白名单内的顶层字段**，其余字段（含用户手写的一切）原样留在本机。

/// 可跨机同步的 settings.json 顶层字段。
///
/// 起手只放 `model` —— 双实例普查显示它两台都有、且不含任何本机路径。这个列表要靠
/// 普查数据（`GET /monitor/config/sync` 的 `probe`）逐个确认后再扩，不要凭想象添加：
/// 一个判断错误就是在所有设备上改坏用户的配置。
pub const SETTINGS_SYNC_KEYS: &[&str] = &["model", "hooks"];

/// 本客户端写进用户 settings.json 的 hook 条目所带的 `_source` 前缀（见 client 的 hookrec）。
///
/// hooks 是同步集里唯一需要**拆开处理**的字段：整份覆盖会把配对 hook 一并带走，
/// 而它的命令是本机 exe 的绝对路径（mac 与 Windows 还不一样），覆盖到另一台机器上
/// 会让那台的会话配对静默失效 —— 不报错、不阻断，只是再也认不出会话。
pub const HOOK_SOURCE_PREFIX: &str = "agent-monitor:";

/// 可跨机同步的 Codex config.toml 顶层字段。
///
/// 客户端用 `toml_edit` 回写，保留用户的注释与字段顺序。这里只放**标量**字段：
/// 复合结构映射到 TOML 有多种合法写法（内联表 / 独立表段），猜错会改乱文件结构，
/// 客户端遇到非标量会直接跳过（见 configsync::json_to_toml）。
pub const CODEX_SYNC_KEYS: &[&str] = &["model"];

/// **永不同步**的字段，即使将来被误加进白名单也挡住。
///
/// - `apiKeyHelper` / `awsAuthRefresh` / `awsCredentialExport`：值几乎必然是本机脚本路径。
/// - `env`：环境变量里混着各种本机路径。
/// - `statusLine`：普查实测一台填的是绝对路径。
/// - `enabledPlugins` / `extraKnownMarketplaces`：插件装没装是每台机器自己的事，
///   同步过去会指向对方没有的插件。
pub const SETTINGS_NEVER_SYNC: &[&str] = &[
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "env",
    "statusLine",
    "enabledPlugins",
    "extraKnownMarketplaces",
    "permissions",
];

/// 该顶层字段是否允许跨机同步。黑名单优先于白名单。
pub fn is_syncable_field(file: &str, key: &str) -> bool {
    if SETTINGS_NEVER_SYNC.contains(&key) {
        return false;
    }
    match file {
        "claude/settings.json" => SETTINGS_SYNC_KEYS.contains(&key),
        "codex/config.toml" => CODEX_SYNC_KEYS.contains(&key),
        _ => false,
    }
}

/// 值里是否出现「只在本机成立」的东西：绝对路径、家目录变量、盘符、UNC 路径。
///
/// 白名单之外的第二道闸：字段名对了，值仍可能是本机路径（用户在任何字段里填绝对路径都是合法的）。
/// 递归看所有字符串叶子——路径常藏在数组或嵌套对象里，只看顶层标量会漏。
pub fn looks_machine_specific(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => {
            let s = s.trim();
            s.starts_with('/')
                || s.starts_with("~/")
                || s.starts_with("\\\\")
                || s.contains("/Users/")
                || s.contains("/home/")
                || s.contains("$HOME")
                || s.contains("%USERPROFILE%")
                || s.contains(":\\")
        }
        serde_json::Value::Array(a) => a.iter().any(looks_machine_specific),
        serde_json::Value::Object(o) => o.values().any(looks_machine_specific),
        _ => false,
    }
}

// ───────────────────────── hooks 的拆分与合并 ─────────────────────────
//
// hooks 的结构：`{ "<事件>": [ { matcher?, hooks: [ {type, command, _source?} ] } ] }`
//
// 里面混着三类东西，必须拆开对待：
// ① 本客户端自己写的配对 hook（带 `_source: agent-monitor:*`）—— 客户端自管，绝不外传也绝不覆盖；
// ② 命令指向本机路径的 hook（`~/bin/x.sh`）—— 换台机器就不存在，同步过去只会报错；
// ③ 其余「通用」hook（`npx prettier --write` 这种）—— 这才是真正值得跨机共用的。
//
// 只有 ③ 参与同步。

/// 单个 hook（最内层的 `{type, command, _source?}`）是否属于本客户端自管
fn is_own_hook(h: &serde_json::Value) -> bool {
    h.get("_source")
        .and_then(|s| s.as_str())
        .map(|s| s.starts_with(HOOK_SOURCE_PREFIX))
        .unwrap_or(false)
}

/// 单个 hook 是否可跨机同步：非自管、且命令不含本机路径
fn is_portable_hook(h: &serde_json::Value) -> bool {
    !is_own_hook(h) && !looks_machine_specific(h)
}

/// 把一个事件下的条目数组按「可跨机 / 只属本机」拆成两份。
///
/// 拆的是**条目内部**的 hooks 数组，而不是整条：同一条 entry（同一个 matcher）下
/// 完全可能既有通用命令又有本机脚本，整条丢弃会连带丢掉本该同步的那个。
fn split_entries(entries: &[serde_json::Value]) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let (mut portable, mut local) = (Vec::new(), Vec::new());
    for e in entries {
        let Some(inner) = e.get("hooks").and_then(|h| h.as_array()) else {
            // 结构不认识就整条留在本机，绝不外传
            local.push(e.clone());
            continue;
        };
        let (p, l): (Vec<_>, Vec<_>) =
            inner.iter().cloned().partition(is_portable_hook);
        for (list, target) in [(p, &mut portable), (l, &mut local)] {
            if list.is_empty() {
                continue;
            }
            let mut cloned = e.clone();
            if let Some(obj) = cloned.as_object_mut() {
                obj.insert("hooks".into(), serde_json::Value::Array(list));
            }
            target.push(cloned);
        }
    }
    (portable, local)
}

/// 取出 hooks 里**可跨机同步**的部分。没有可同步内容时返回 None。
pub fn portable_hooks(v: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = v.as_object()?;
    let mut out = serde_json::Map::new();
    for (event, entries) in obj {
        let Some(arr) = entries.as_array() else { continue };
        let (portable, _) = split_entries(arr);
        if !portable.is_empty() {
            out.insert(event.clone(), serde_json::Value::Array(portable));
        }
    }
    (!out.is_empty()).then(|| serde_json::Value::Object(out))
}

/// 把下发的 hooks 合并进本机 hooks。
///
/// 结果 =「本机只属本机的部分」+「下发的通用部分」。前者原封不动 ——
/// 配对 hook 与指向本机脚本的 hook 因此永远不会被另一台机器的配置挤掉。
pub fn merge_hooks(
    local: Option<&serde_json::Value>,
    incoming: &serde_json::Value,
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = local.and_then(|v| v.as_object()) {
        for (event, entries) in obj {
            let Some(arr) = entries.as_array() else {
                out.insert(event.clone(), entries.clone());
                continue;
            };
            let (_, keep) = split_entries(arr);
            if !keep.is_empty() {
                out.insert(event.clone(), serde_json::Value::Array(keep));
            }
        }
    }
    if let Some(obj) = incoming.as_object() {
        for (event, entries) in obj {
            let Some(arr) = entries.as_array() else { continue };
            // 下发内容同样不可信：再滤一遍，只接受通用条目
            let (portable, _) = split_entries(arr);
            if portable.is_empty() {
                continue;
            }
            match out.get_mut(event).and_then(|v| v.as_array_mut()) {
                Some(existing) => existing.extend(portable),
                None => {
                    out.insert(event.clone(), serde_json::Value::Array(portable));
                }
            }
        }
    }
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_and_blacklist_do_not_overlap() {
        for k in SETTINGS_SYNC_KEYS {
            assert!(!SETTINGS_NEVER_SYNC.contains(k), "{k} 同时在白名单和黑名单里");
        }
    }

    #[test]
    fn own_pairing_hook_never_leaves_the_machine() {
        // 这条挂了就意味着配对 hook 会被外传/跨机覆盖 —— 整个功能最危险的回归
        let hooks = serde_json::json!({
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [
                    { "type": "command", "command": "/opt/am/agent-monitor hook",
                      "_source": "agent-monitor:pairing" },
                    { "type": "command", "command": "npx prettier --write" }
                ]
            }]
        });
        let portable = portable_hooks(&hooks).expect("通用 hook 应可同步");
        let dump = serde_json::to_string(&portable).unwrap();
        assert!(!dump.contains("agent-monitor"), "配对 hook 被外传: {dump}");
        assert!(dump.contains("prettier"), "通用 hook 该被同步: {dump}");
    }

    #[test]
    fn machine_specific_hooks_stay_home() {
        let hooks = serde_json::json!({
            "PostToolUse": [{
                "hooks": [
                    { "type": "command", "command": "~/bin/my-local.sh" },
                    { "type": "command", "command": "/Users/vita/x.sh" },
                    { "type": "command", "command": "cargo fmt" }
                ]
            }]
        });
        let dump = serde_json::to_string(&portable_hooks(&hooks).unwrap()).unwrap();
        assert!(dump.contains("cargo fmt"));
        assert!(!dump.contains("my-local.sh"), "本机脚本被外传: {dump}");
        assert!(!dump.contains("/Users/vita"), "本机路径被外传: {dump}");
    }

    #[test]
    fn nothing_portable_yields_none() {
        // 只有配对 hook 时没有任何可同步内容
        let only_ours = serde_json::json!({
            "SessionStart": [{
                "hooks": [{ "command": "/opt/am/x hook", "_source": "agent-monitor:pairing" }]
            }]
        });
        assert!(portable_hooks(&only_ours).is_none());
    }

    #[test]
    fn merge_keeps_local_pairing_hook_and_adds_incoming() {
        let local = serde_json::json!({
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [
                    { "type": "command", "command": "/opt/am/agent-monitor hook",
                      "_source": "agent-monitor:pairing" },
                    { "type": "command", "command": "~/bin/local-only.sh" },
                    { "type": "command", "command": "old-generic" }
                ]
            }],
            "PostToolUse": [{
                "hooks": [{ "command": "/opt/am/agent-monitor hook",
                            "_source": "agent-monitor:pairing" }]
            }]
        });
        let incoming = serde_json::json!({
            "PreToolUse": [{ "matcher": "*", "hooks": [{ "command": "npx prettier" }] }],
            "Stop": [{ "hooks": [{ "command": "echo done" }] }]
        });

        let merged = merge_hooks(Some(&local), &incoming);
        let dump = serde_json::to_string(&merged).unwrap();

        // 本机自管与本机脚本原样保留
        assert!(dump.contains("agent-monitor:pairing"), "配对 hook 丢了: {dump}");
        assert!(dump.contains("local-only.sh"), "本机脚本丢了: {dump}");
        // PostToolUse 只有配对 hook，也必须留着
        assert!(merged.get("PostToolUse").is_some(), "只含配对 hook 的事件被丢: {dump}");
        // 下发内容进来了
        assert!(dump.contains("npx prettier"));
        assert!(dump.contains("echo done"));
        // 镜像机原有的通用 hook 让位给配置源（单向镜像语义）
        assert!(!dump.contains("old-generic"), "通用 hook 应被配置源接管: {dump}");
    }

    #[test]
    fn merge_rejects_own_source_smuggled_from_hub() {
        // hub 被攻破也不能借下发把 _source 条目塞进来（否则可伪装成客户端自管条目）
        let incoming = serde_json::json!({
            "PreToolUse": [{
                "hooks": [{ "command": "evil", "_source": "agent-monitor:pairing" }]
            }]
        });
        let merged = merge_hooks(None, &incoming);
        assert!(!serde_json::to_string(&merged).unwrap().contains("evil"));
    }

    #[test]
    fn field_whitelist_is_narrow() {
        assert!(is_syncable_field("claude/settings.json", "model"));
        assert!(is_syncable_field("codex/config.toml", "model"));
        // hooks 可同步，但只同步「通用」条目（见 portable_hooks / merge_hooks）
        assert!(is_syncable_field("claude/settings.json", "hooks"));
        // 普查判定为机器相关的一律不可同步
        for k in ["apiKeyHelper", "statusLine", "env", "enabledPlugins", "permissions"] {
            assert!(!is_syncable_field("claude/settings.json", k), "{k} 不该可同步");
        }
        // 未知字段默认不同步（新版 Claude Code 加的字段不会被自动带上）
        assert!(!is_syncable_field("claude/settings.json", "someFutureField"));
        // 未知文件一律不认
        assert!(!is_syncable_field("claude/other.json", "model"));
    }

    #[test]
    fn machine_specific_detection() {
        use serde_json::json;
        assert!(looks_machine_specific(&json!("/Users/vita/bin/x.sh")));
        assert!(looks_machine_specific(&json!("~/bin/x.sh")));
        assert!(looks_machine_specific(&json!("C:\\Users\\vita\\x.exe")));
        assert!(looks_machine_specific(&json!("\\\\server\\share")));
        assert!(looks_machine_specific(&json!("$HOME/x")));
        // 藏在数组/嵌套对象里的也要抓到
        assert!(looks_machine_specific(&json!({"hooks": [{"command": "/opt/am/x"}]})));
        assert!(looks_machine_specific(&json!(["ok", "/abs/path"])));

        assert!(!looks_machine_specific(&json!("opus")));
        assert!(!looks_machine_specific(&json!("ccusage")));
        assert!(!looks_machine_specific(&json!(30)));
        assert!(!looks_machine_specific(&json!({"type": "command"})));
    }

    #[test]
    fn accepts_whitelisted() {
        assert!(is_allowed("claude/CLAUDE.md"));
        assert!(is_allowed("claude/agents/reviewer.md"));
        assert!(is_allowed("claude/commands/deploy.md"));
        assert!(is_allowed("claude/skills/deep/nested/x.md"));
        assert!(is_allowed("codex/AGENTS.md"));
        assert!(is_allowed("codex/prompts/a.md"));
    }

    #[test]
    fn rejects_credentials_and_settings() {
        assert!(!is_allowed("claude/.credentials.json"));
        assert!(!is_allowed("claude/settings.json"));
        assert!(!is_allowed("codex/auth.json"));
        assert!(!is_allowed("codex/config.toml"));
    }

    #[test]
    fn rejects_traversal() {
        assert!(!is_allowed("claude/agents/../../../.ssh/authorized_keys.md"));
        assert!(!is_allowed("claude/agents/..\\x.md"));
        assert!(!is_allowed("/etc/x.md"));
        assert!(!is_allowed("claude/agents//x.md"));
        assert!(!is_allowed("claude/agents/.hidden/x.md"));
    }

    #[test]
    fn rejects_outside_whitelist() {
        // 会话历史体积大且含对话内容，绝不进同步集
        assert!(!is_allowed("claude/projects/p.md"));
        assert!(!is_allowed("other/x.md"));
        assert!(!is_allowed("claude/agents.md"));
        // 备份文件不能被当成配置再同步出去（见客户端 with_suffix）
        assert!(!is_allowed("claude/agents/x.md.am-bak"));
    }
}
