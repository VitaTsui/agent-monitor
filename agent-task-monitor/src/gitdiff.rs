//! 计算某项目目录的 git 概览（改动文件 + 相对上次提交的统一 diff）。
//! hub 本机会话直接调用；远程会话由 agent 调用后回传。只读，不改动仓库。
use crate::model::{GitFile, GitOverview};

fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

/// 统一 diff 的最大字节数，防超大改动撑爆传输/前端
const MAX_DIFF_BYTES: usize = 400_000;

pub fn git_overview(cwd: &str) -> GitOverview {
    if cwd.trim().is_empty() {
        return GitOverview {
            error: "该会话没有项目目录".into(),
            ..Default::default()
        };
    }
    // 是否 git 仓库
    let is_repo = git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false);
    if !is_repo {
        return GitOverview {
            is_repo: false,
            error: "该项目目录不是 git 仓库".into(),
            ..Default::default()
        };
    }

    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // 改动文件：git status --porcelain
    let mut files = Vec::new();
    let mut untracked = Vec::new();
    if let Some(status) = git(cwd, &["status", "--porcelain"]) {
        for line in status.lines() {
            if line.len() < 3 {
                continue;
            }
            let code = &line[..2];
            let path = line[3..].to_string();
            if code == "??" {
                untracked.push(path);
            } else {
                files.push(GitFile {
                    status: code.to_string(),
                    path,
                });
            }
        }
    }

    // 相对上次提交的统一 diff（暂存 + 未暂存）；仓库无提交时回退到工作区 diff
    let mut diff = git(cwd, &["diff", "HEAD"])
        .or_else(|| git(cwd, &["diff"]))
        .unwrap_or_default();
    let truncated = diff.len() > MAX_DIFF_BYTES;
    if truncated {
        // 按字符边界安全截断
        let mut end = MAX_DIFF_BYTES;
        while end > 0 && !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
        diff.push_str("\n…（diff 过大，已截断）");
    }

    GitOverview {
        is_repo: true,
        branch,
        files,
        diff,
        untracked,
        error: String::new(),
    }
}
