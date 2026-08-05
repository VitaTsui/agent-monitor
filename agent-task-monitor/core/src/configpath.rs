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
pub const SETTINGS_SYNC_KEYS: &[&str] = &["model"];

/// 可跨机同步的 Codex config.toml 顶层字段
pub const CODEX_SYNC_KEYS: &[&str] = &["model"];

/// **永不同步**的字段，即使将来被误加进白名单也挡住。
///
/// - `hooks`：本客户端自己写进去的配对 hook，命令是**本机 exe 的绝对路径**
///   （见 client 的 hookrec），覆盖到另一台机器上会让那台的会话配对静默失效——
///   不报错、不阻断，只是再也认不出会话。这是整个二期最危险的一个字段。
/// - `apiKeyHelper` / `awsAuthRefresh` / `awsCredentialExport`：值几乎必然是本机脚本路径。
/// - `env`：环境变量里混着各种本机路径。
/// - `statusLine`：普查实测一台填的是绝对路径。
/// - `enabledPlugins` / `extraKnownMarketplaces`：插件装没装是每台机器自己的事，
///   同步过去会指向对方没有的插件。
pub const SETTINGS_NEVER_SYNC: &[&str] = &[
    "hooks",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_is_never_syncable() {
        // 这条挂了就意味着配对 hook 会被跨机覆盖 —— 二期最危险的回归
        assert!(!is_syncable_field("claude/settings.json", "hooks"));
        for k in SETTINGS_SYNC_KEYS {
            assert!(!SETTINGS_NEVER_SYNC.contains(k), "{k} 同时在白名单和黑名单里");
        }
    }

    #[test]
    fn field_whitelist_is_narrow() {
        assert!(is_syncable_field("claude/settings.json", "model"));
        assert!(is_syncable_field("codex/config.toml", "model"));
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
