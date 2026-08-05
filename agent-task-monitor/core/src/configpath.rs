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

/// **可执行脚本目录**：其下的文件不限扩展名，落盘后补上执行位。
///
/// hook 脚本通常没有扩展名（`~/.claude/hooks/cbm-session-reminder`），按 .md 规则会被整个
/// 漏掉 —— 于是 hooks 配置同步过去了、脚本没有，每次触发都报错。
///
/// 放开这个目录意味着**分发会被自动执行的内容**：脚本落地后 Claude Code 触发 hook 时会直接
/// 跑它，不需要用户点任何东西。边界因此划得很死：仅此一个目录，不含隐藏文件，
/// 单文件仍受 `MAX_FILE_BYTES` 限制。
pub const EXEC_DIRS: &[&str] = &["claude/hooks"];

/// 该相对路径是否属于「落盘后要补执行位」的脚本
pub fn is_exec_path(rel: &str) -> bool {
    EXEC_DIRS.iter().any(|d| rel.starts_with(&format!("{d}/")))
}

/// `.claude/` **根层**的配置文件：MCP 与 hook 常用 `--config ~/.claude/xxx.json` 引用它们，
/// 那个文件不跟过去，server 照样起不来。
///
/// 只放根层、非隐藏、且**排除 `settings*`**（它走字段级同步，整份搬会把配对 hook 一起带走）。
/// `~/.claude.json` 不在这个目录下，不受影响。
///
/// 注意这只是「允许落盘」的边界；**上传侧另有一道**：客户端只把真正被 MCP/hook 引用到的
/// 文件放进同步集，不会把根目录下所有 json 都搬走（见 client 的 referenced_configs）。
pub fn is_root_config(rel: &str) -> bool {
    let Some(name) = rel.strip_prefix("claude/") else { return false };
    if name.contains('/') || name.starts_with("settings.") {
        return false;
    }
    matches!(
        name.rsplit_once('.').map(|(_, e)| e),
        Some("json") | Some("yaml") | Some("yml") | Some("toml") | Some("txt")
    )
}

/// 单文件体积上限。配置类文本几 KB 顶天，超了多半是有人把日志/数据丢了进来——
/// 这种东西挤进 1.5s 一轮的心跳会把上报撑到 413。
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// 相对路径是否在同步集内。
///
/// 路径一律是归一化的相对形式（`claude/agents/x.md`），第一段是根标识
/// （`claude` → `~/.claude`，`codex` → `~/.codex`），由各端自己拼本机绝对路径。
pub fn is_allowed(rel: &str) -> bool {
    // 路径穿越与反斜杠（Windows 上 `a\..\..\x` 同样能越出）一律拒
    if rel.contains("..") || rel.contains('\\') || rel.starts_with('/') {
        return false;
    }
    // 隐藏段不收，且挡掉空段（`a//b`）
    if rel.split('/').any(|seg| seg.is_empty() || seg.starts_with('.')) {
        return false;
    }
    // hooks 目录：脚本没有固定扩展名，按 .md 规则会被整个漏掉（见 EXEC_DIRS）
    if is_exec_path(rel) {
        return true;
    }
    // MCP / hook 引用的根层配置文件（`--config ~/.claude/xxx.json`）
    if is_root_config(rel) {
        return true;
    }
    // 其余一律只收 .md：白名单目录里混着别的东西时（比如 skills 下的脚本），不碰。
    // 凭据（.credentials.json / auth.json）与设置（settings.json / config.toml）
    // 都因此天然落在文件同步集之外——前者进 hub 就是一份明文账号副本，
    // 后者混着机器相关内容（尤其本客户端写进 settings.json 的配对 hook，
    // 命令是本机 exe 的绝对路径），整份同步会让另一台机器的配对静默失效。
    // settings.json / config.toml / claude.json 走的是**字段级**同步，不是整份搬。
    if !rel.ends_with(".md") {
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

/// `~/.claude.json` 里可同步的顶层字段。
///
/// 这个文件有几十 KB，混着 OAuth 凭据、逐项目的会话历史与各种运行状态 ——
/// **只取 `mcpServers` 这一个键**，其余一概不碰、也不上传。
pub const CLAUDE_JSON_SYNC_KEYS: &[&str] = &["mcpServers"];

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
        "claude/claude.json" => CLAUDE_JSON_SYNC_KEYS.contains(&key),
        "codex/config.toml" => CODEX_SYNC_KEYS.contains(&key),
        _ => false,
    }
}

// ───────────────────────── MCP 服务器配置 ─────────────────────────
//
// `mcpServers` 与 hooks 一样需要拆开处理，原因有两个：
// ① `command` 常是含用户名的绝对路径（`/Users/vita/.local/bin/x`）—— 直接搬到另一台
//    机器就指向了不存在的位置。所以同步前把 home 前缀归一化成 `~`，落盘时再展开成
//    对方自己的 home。**这正是配置同步该干的事**：把机器相关的形式转成可移植的。
// ② `env` 里常放 API key。凭据不离开本机是这个功能的底线，所以一律剥掉 ——
//    代价是依赖 env 的 server 需要在各机器上自行补齐那几个变量。

/// 递归把值里的 home 绝对路径前缀换成 `~`（同步出去时用）。
///
/// `home` 为空时只原样返回：hub 侧复检时并不知道对端的 home，
/// 空前缀会 match 上每一个字符串，把 `/etc/x` 变成 `~/etc/x` 这种荒唐结果。
fn normalize_home(v: &serde_json::Value, home: &str) -> serde_json::Value {
    if home.is_empty() {
        return v.clone();
    }
    match v {
        serde_json::Value::String(s) => {
            let out = match s.strip_prefix(home) {
                Some(rest) if rest.starts_with('/') || rest.is_empty() => format!("~{rest}"),
                _ => s.clone(),
            };
            serde_json::Value::String(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(|x| normalize_home(x, home)).collect())
        }
        serde_json::Value::Object(o) => serde_json::Value::Object(
            o.iter().map(|(k, x)| (k.clone(), normalize_home(x, home))).collect(),
        ),
        other => other.clone(),
    }
}

/// 递归把 `~` 展开成本机 home（落盘时用）。
///
/// 必须展开：`command` 交给系统直接 exec，不经过 shell，`~` 不会被展开成家目录，
/// 留着它 MCP server 根本起不来。
fn localize_home(v: &serde_json::Value, home: &str) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => {
            let out = if s == "~" {
                home.to_string()
            } else if let Some(rest) = s.strip_prefix("~/") {
                // 用平台分隔符拼：mac 上写的 `~/a/b` 同步到 Windows 要变成
                // `C:\Users\你\a\b`。混合分隔符 Windows 多半也认，但没必要赌。
                let sep = std::path::MAIN_SEPARATOR;
                let rest = if sep == '/' { rest.to_string() } else { rest.replace('/', "\\") };
                format!("{home}{sep}{rest}")
            } else {
                s.clone()
            };
            serde_json::Value::String(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(|x| localize_home(x, home)).collect())
        }
        serde_json::Value::Object(o) => serde_json::Value::Object(
            o.iter().map(|(k, x)| (k.clone(), localize_home(x, home))).collect(),
        ),
        other => other.clone(),
    }
}

/// 取出可跨机同步的 MCP 配置：剥掉 `env`、把 home 绝对路径归一化成 `~`。
/// 没有任何 server 时返回 None。
pub fn portable_mcp(v: &serde_json::Value, home: &str) -> Option<serde_json::Value> {
    let obj = v.as_object()?;
    let mut out = serde_json::Map::new();
    for (name, cfg) in obj {
        let mut cfg = normalize_home(cfg, home);
        if let Some(o) = cfg.as_object_mut() {
            // 凭据不离开本机。留下键名也没意义（值才是密钥），整个 env 摘掉。
            o.remove("env");
        }
        out.insert(name.clone(), cfg);
    }
    (!out.is_empty()).then(|| serde_json::Value::Object(out))
}

/// 把下发的 MCP 配置落到本机形态：`~` 展开成本机 home，并保留本机原有的 `env`。
///
/// 保留 env 很关键：同步过来的配置里没有 env（上传时剥掉了），若直接覆盖，
/// 本机原本配好的那几个 API key 就被抹掉了 —— 用户会发现 server 突然连不上。
pub fn merge_mcp(
    local: Option<&serde_json::Value>,
    incoming: &serde_json::Value,
    home: &str,
) -> serde_json::Value {
    let local_obj = local.and_then(|v| v.as_object());
    // **以本机现有的为底**，再用下发的覆盖同名条目。
    // 从空表开始是错的：那会把本机独有的 server（对方没有的那些）整个抹掉 ——
    // 用户在这台机器上配好的 MCP 会在一次同步后凭空消失、随即连不上。
    let mut out = local_obj.cloned().unwrap_or_default();
    if let Some(obj) = incoming.as_object() {
        for (name, cfg) in obj {
            let mut cfg = localize_home(cfg, home);
            // 把本机该 server 原有的 env 搬回去
            if let (Some(o), Some(prev_env)) = (
                cfg.as_object_mut(),
                local_obj
                    .and_then(|l| l.get(name))
                    .and_then(|c| c.get("env"))
                    .filter(|e| !e.is_null()),
            ) {
                o.insert("env".into(), prev_env.clone());
            }
            out.insert(name.clone(), cfg);
        }
    }
    serde_json::Value::Object(out)
}

/// 值里是否出现「只在本机成立」的东西：绝对路径、家目录变量、盘符、UNC 路径。
///
/// 白名单之外的第二道闸：字段名对了，值仍可能是本机路径（用户在任何字段里填绝对路径都是合法的）。
/// 递归看所有字符串叶子——路径常藏在数组或嵌套对象里，只看顶层标量会漏。
/// 值里是否出现「只在本机成立」的东西。
///
/// **判据是绝对路径，不是「路径」**：`~/`、`$HOME/`、`%USERPROFILE%` 是相对家目录的写法，
/// 每台机器各自解析到自己的 home，跨机语义完全一致 —— `~/.claude/hooks/x` 在两台机器上
/// 指的都是「我的 hooks 目录下的 x」，正是该同步的东西。早先把它们一并判为机器相关，
/// 结果是用户真正想共用的那批 hook 被整个滤掉（实测就是这个现象）。
///
/// 真正不可移植的是含用户名或应用安装位置的绝对路径：`/Users/vita/...`、`/Applications/...`、
/// `C:\Users\...`、UNC。
pub fn looks_machine_specific(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => {
            let s = s.trim();
            s.starts_with('/')
                || s.starts_with("\\\\")
                || s.contains("/Users/")
                || s.contains("/home/")
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

/// 本客户端可执行文件名。配对 hook 的命令一定包含它（`<装在哪>/agent-monitor hook`）。
const OWN_EXE_NAME: &str = "agent-monitor";

/// 单个 hook（最内层的 `{type, command, _source?}`）是否属于本客户端自管。
///
/// **两条判据缺一不可**：
/// ① `_source` 标记 —— 新版客户端写入时会带；
/// ② 命令里含本客户端可执行名 —— **早期版本写入的配对 hook 没有标记**（实测本机三条
///    配对 hook 的 `_source` 全是空的）。只认标记就会漏，漏了就意味着把别人机器的
///    配对 hook 覆盖掉、或把自己的外传出去。
///
/// 判据②宁可宽：误判成「自管」最多是这条 hook 不参与同步，而漏判的代价是配对静默失效。
fn is_own_hook(h: &serde_json::Value) -> bool {
    let tagged = h
        .get("_source")
        .and_then(|s| s.as_str())
        .map(|s| s.starts_with(HOOK_SOURCE_PREFIX))
        .unwrap_or(false);
    let by_command = h
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| c.contains(OWN_EXE_NAME))
        .unwrap_or(false);
    tagged || by_command
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
                    { "type": "command", "command": "/Users/vita/x.sh" },
                    { "type": "command", "command": "/Applications/Foo.app/bin/x" },
                    { "type": "command", "command": "cargo fmt" }
                ]
            }]
        });
        let dump = serde_json::to_string(&portable_hooks(&hooks).unwrap()).unwrap();
        assert!(dump.contains("cargo fmt"));
        assert!(!dump.contains("/Users/vita"), "本机绝对路径被外传: {dump}");
        assert!(!dump.contains("Foo.app"), "应用绝对路径被外传: {dump}");
    }

    #[test]
    fn home_relative_hooks_are_portable() {
        // `~/.claude/hooks/*` 在每台机器上各自解析到自己的 home，跨机语义一致 ——
        // 这正是用户最想共用的一批 hook。早先把 `~/` 一并判为机器相关，把它们全滤掉了。
        let hooks = serde_json::json!({
            "SessionStart": [
                { "matcher": "startup", "hooks": [{ "command": "~/.claude/hooks/cbm-session-reminder" }] },
                { "matcher": "*", "hooks": [{ "command": "$HOME/.claude/hooks/x" }] }
            ]
        });
        let dump = serde_json::to_string(&portable_hooks(&hooks).unwrap()).unwrap();
        assert!(dump.contains("cbm-session-reminder"), "~/ 类 hook 该同步: {dump}");
        assert!(dump.contains("$HOME/.claude/hooks/x"), "$HOME 类 hook 该同步: {dump}");
    }

    #[test]
    fn untagged_pairing_hook_is_still_recognized() {
        // 早期版本写入的配对 hook 没有 _source 标记（实测本机就是这样）。
        // 只认标记会漏 —— 漏了就意味着把它外传、或覆盖掉别人机器上的那条。
        let hooks = serde_json::json!({
            "SessionStart": [{
                "hooks": [
                    { "type": "command",
                      "command": "/Applications/终端任务监控.app/Contents/MacOS/agent-monitor hook" },
                    { "type": "command", "command": "~/.claude/hooks/mine" }
                ]
            }]
        });
        let dump = serde_json::to_string(&portable_hooks(&hooks).unwrap()).unwrap();
        assert!(!dump.contains("agent-monitor"), "无标记的配对 hook 被外传: {dump}");
        assert!(dump.contains("mine"), "用户自己的 hook 该同步: {dump}");

        // 合并时同样要留住它：即使命令路径与配置源不同，也不能被对方那条顶掉
        let incoming = serde_json::json!({
            "SessionStart": [{ "hooks": [{ "command": "~/.claude/hooks/mine" }] }]
        });
        let merged = serde_json::to_string(&merge_hooks(Some(&hooks), &incoming)).unwrap();
        assert!(merged.contains("终端任务监控.app"), "本机无标记配对 hook 被挤掉: {merged}");
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
                    { "type": "command", "command": "/Users/me/local-only.sh" },
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

        // 本机自管与本机绝对路径脚本原样保留
        assert!(dump.contains("agent-monitor:pairing"), "配对 hook 丢了: {dump}");
        assert!(dump.contains("local-only.sh"), "本机绝对路径脚本丢了: {dump}");
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
    fn mcp_strips_env_and_normalizes_home() {
        let home = "/Users/vita";
        let mcp = serde_json::json!({
            "playwright": { "command": "npx", "args": ["-y", "@playwright/mcp@latest"] },
            "cbm": {
                "command": "/Users/vita/.local/bin/codebase-memory-mcp",
                "args": ["--root", "/Users/vita/work"],
                "env": { "SECRET_TOKEN": "sk-must-not-leave" }
            }
        });
        let p = portable_mcp(&mcp, home).expect("应有可同步内容");
        let dump = serde_json::to_string(&p).unwrap();

        // 凭据绝不外传
        assert!(!dump.contains("sk-must-not-leave"), "env 泄露: {dump}");
        assert!(!dump.contains("SECRET_TOKEN"), "env 泄露: {dump}");
        // home 绝对路径归一化成 ~，其余原样
        assert_eq!(p["cbm"]["command"], serde_json::json!("~/.local/bin/codebase-memory-mcp"));
        assert_eq!(p["cbm"]["args"][1], serde_json::json!("~/work"));
        assert_eq!(p["playwright"]["command"], serde_json::json!("npx"));
        assert!(!dump.contains("/Users/vita"), "本机用户名外传: {dump}");
    }

    #[test]
    fn mcp_merge_localizes_and_keeps_local_env() {
        let incoming = serde_json::json!({
            "cbm": { "command": "~/.local/bin/codebase-memory-mcp", "args": ["--root", "~/work"] }
        });
        let local = serde_json::json!({
            "cbm": {
                "command": "/Users/bob/.local/bin/codebase-memory-mcp",
                "env": { "MY_KEY": "local-secret" }
            }
        });
        let merged = merge_mcp(Some(&local), &incoming, "/Users/bob");

        // ~ 必须展开：command 直接 exec，不经 shell，留着 ~ 就起不来
        assert_eq!(
            merged["cbm"]["command"],
            serde_json::json!("/Users/bob/.local/bin/codebase-memory-mcp")
        );
        assert_eq!(merged["cbm"]["args"][1], serde_json::json!("/Users/bob/work"));
        // 本机原有的 env 要留住，否则用户配好的 key 被同步抹掉
        assert_eq!(merged["cbm"]["env"]["MY_KEY"], serde_json::json!("local-secret"));
    }

    #[test]
    fn mcp_merge_keeps_servers_only_this_machine_has() {
        // 本机独有的 server 必须活下来。从空表开始合并会让它们在一次同步后凭空消失，
        // 用户只会看到「同步完 MCP 就连不上了」。
        let incoming = serde_json::json!({ "shared": { "command": "npx" } });
        let local = serde_json::json!({
            "shared": { "command": "npx" },
            "ludo":   { "command": "/opt/ludo/bin/server", "env": { "K": "v" } }
        });
        let merged = merge_mcp(Some(&local), &incoming, "/Users/bob");
        assert_eq!(merged["ludo"]["command"], serde_json::json!("/opt/ludo/bin/server"));
        assert_eq!(merged["ludo"]["env"]["K"], serde_json::json!("v"));
        assert!(merged.get("shared").is_some());
    }

    #[test]
    fn claude_json_only_exposes_mcp_servers() {
        // 这个文件里还有 OAuth 凭据与逐项目历史，只有 mcpServers 可以动
        assert!(is_syncable_field("claude/claude.json", "mcpServers"));
        for k in ["oauthAccount", "projects", "userID", "hasCompletedOnboarding", "tipsHistory"] {
            assert!(!is_syncable_field("claude/claude.json", k), "{k} 不该可同步");
        }
    }

    #[test]
    fn machine_specific_detection() {
        use serde_json::json;
        // 绝对路径：含用户名或应用安装位置，换台机器就不成立
        assert!(looks_machine_specific(&json!("/Users/vita/bin/x.sh")));
        assert!(looks_machine_specific(&json!("C:\\Users\\vita\\x.exe")));
        assert!(looks_machine_specific(&json!("\\\\server\\share")));
        // 藏在数组/嵌套对象里的也要抓到
        assert!(looks_machine_specific(&json!({"hooks": [{"command": "/opt/am/x"}]})));
        assert!(looks_machine_specific(&json!(["ok", "/abs/path"])));

        // 相对家目录的写法**不是**机器相关：每台机器各自解析到自己的 home，
        // `~/.claude/hooks/x` 在两台机器上指的都是「我的 hooks 目录下的 x」
        assert!(!looks_machine_specific(&json!("~/bin/x.sh")));
        assert!(!looks_machine_specific(&json!("~/.claude/hooks/gate")));
        assert!(!looks_machine_specific(&json!("$HOME/x")));
        assert!(!looks_machine_specific(&json!("%USERPROFILE%\\x")));

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
    fn hooks_scripts_sync_but_neighbours_do_not() {
        // 脚本没有扩展名，正是按 .md 规则会被漏掉的那批
        assert!(is_allowed("claude/hooks/cbm-session-reminder"));
        assert!(is_allowed("claude/hooks/sub/dir/script.sh"));
        assert!(is_exec_path("claude/hooks/cbm-session-reminder"));

        // 设置与凭据绝不能跟着开
        assert!(!is_allowed("claude/settings.json"));
        assert!(!is_allowed("claude/settings.local.json"));
        assert!(!is_allowed("claude/.credentials.json"));
        // 根层配置文件可以（MCP 的 --config 引用它），但上传侧只收被引用的
        assert!(is_allowed("claude/playwright.mcp.config.json"));
        assert!(is_root_config("claude/playwright.mcp.config.json"));
        // 仅根层，且排除 settings*
        assert!(!is_root_config("claude/sub/x.json"));
        assert!(!is_root_config("claude/settings.json"));
        assert!(!is_root_config("claude/x.exe"));
        assert!(!is_exec_path("claude/agents/x.md"));
        // 穿越与隐藏文件在 hooks 目录下同样挡住
        assert!(!is_allowed("claude/hooks/../settings.json"));
        assert!(!is_allowed("claude/hooks/.hidden"));
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
