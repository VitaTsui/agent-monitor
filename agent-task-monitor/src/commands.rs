//! 汇总某代理会话「可用斜杠命令」：内置命令 + 扫描 .claude/commands 自定义命令。
//! 只读文件系统，不影响终端任务。
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct SlashCommand {
    /// 形如 "/clear"
    pub name: String,
    pub desc: String,
    /// builtin / project / user
    pub source: String,
}

/// Claude Code 常用内置命令
const CLAUDE_BUILTIN: &[(&str, &str)] = &[
    ("/add-dir", "添加工作目录"),
    ("/agents", "管理子代理"),
    ("/clear", "清空对话历史"),
    ("/compact", "压缩对话以节省上下文"),
    ("/config", "查看/修改配置"),
    ("/cost", "查看本次会话用量与花费"),
    ("/doctor", "检查环境与安装"),
    ("/help", "显示帮助"),
    ("/init", "初始化 CLAUDE.md"),
    ("/mcp", "管理 MCP 服务器"),
    ("/memory", "编辑记忆文件"),
    ("/model", "切换模型"),
    ("/permissions", "管理权限"),
    ("/review", "代码审查"),
    ("/status", "查看账号与连接状态"),
    ("/resume", "恢复历史会话"),
    ("/export", "导出对话"),
    ("/vim", "切换 Vim 模式"),
];

/// Codex 常用内置命令
const CODEX_BUILTIN: &[(&str, &str)] = &[
    ("/clear", "清空对话"),
    ("/new", "新建会话"),
    ("/model", "切换模型"),
    ("/help", "显示帮助"),
];

/// 汇总某会话可用命令：provider 内置 + 项目 .claude/commands + 用户 ~/.claude/commands
pub fn collect(provider: &str, project_cwd: &str) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = Vec::new();
    // 只对已知命令体系的 provider 给内置命令；其它（gemini/aider/进程级任务）
    // 返回空 —— 前端没有命令 chips 可点，避免把 Claude 的命令错发给别的代理。
    let builtin = match provider {
        "claude" => CLAUDE_BUILTIN,
        "codex" => CODEX_BUILTIN,
        _ => return out,
    };
    for (name, desc) in builtin {
        out.push(SlashCommand {
            name: name.to_string(),
            desc: desc.to_string(),
            source: "builtin".into(),
        });
    }

    // 项目级自定义命令
    if !project_cwd.is_empty() {
        scan_commands(&Path::new(project_cwd).join(".claude").join("commands"), "project", &mut out);
    }
    // 用户级自定义命令
    if let Some(home) = dirs::home_dir() {
        scan_commands(&home.join(".claude").join("commands"), "user", &mut out);
    }

    // 去重（同名保留先出现的）
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out
}

/// 扫描一个 commands 目录：每个 .md 文件 = 一个命令；子目录形成 /namespace:name
fn scan_commands(dir: &Path, source: &str, out: &mut Vec<SlashCommand>) {
    scan_dir(dir, dir, source, out);
}

fn scan_dir(root: &Path, dir: &Path, source: &str, out: &mut Vec<SlashCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            scan_dir(root, &path, source, out);
        } else if path.extension().and_then(|x| x.to_str()) == Some("md") {
            // 相对 root 的路径 → namespace:name
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let stem: Vec<String> = rel
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect();
            let name = format!("/{}", stem.join(":"));
            let desc = first_desc(&path).unwrap_or_else(|| "自定义命令".into());
            out.push(SlashCommand { name, desc, source: source.into() });
        }
    }
}

/// 取 md 文件 frontmatter 的 description，或首行非空文本作为描述
fn first_desc(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    // frontmatter: description: xxx
    for line in content.lines().take(20) {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("description:") {
            let d = rest.trim().trim_matches('"').trim();
            if !d.is_empty() {
                return Some(truncate(d, 60));
            }
        }
    }
    // 首行非空、非 frontmatter 分隔符
    for line in content.lines() {
        let l = line.trim().trim_start_matches('#').trim();
        if !l.is_empty() && l != "---" {
            return Some(truncate(l, 60));
        }
    }
    None
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}
