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

#[cfg(test)]
mod tests {
    use super::*;

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
