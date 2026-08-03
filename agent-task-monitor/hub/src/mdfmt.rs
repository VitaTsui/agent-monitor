//! 出站 Markdown 适配：把 agent 输出降级成钉钉真能渲染的样子。
//!
//! 病灶：Claude Code / Codex 的输出天然带表格与代码围栏，而**钉钉 markdown 官方只保证**
//! 标题、加粗、斜体、链接、图片、有序/无序列表、引用；代码围栏不在保证范围内 ——
//! 推过去就是孤零零的 ```。而「人在手机上看结果」正是本项目的核心场景，所以出站前降级：
//!
//! - 围栏行删掉、围栏内的代码原样保留（代码本身要看，围栏符号是噪音）。
//!
//! **表格不再降级**：早先按官方文档把表格转成 `- 表头: 值｜…` 的列表，但实测钉钉已能
//! 渲染 markdown 表格，转成列表反而丢了行列对照、比原表难读。现在原样透传。
//! （判定与转换那几个辅助函数一并删了，留着只会一直报 dead_code；真要按客户端版本
//! 重新降级，从 git 历史取回即可。）
//!
//! 另外提供按长度切分：钉钉单条 markdown 上限约 4000 字符，此前各处是硬截断到 1500/1800，
//! 既浪费额度又会把话切断在半句。切分优先落在换行、其次空格，避免拦腰截断。
//!
//! 设计参考 jingxin-agent `core/channels/markdown.py`（同一套降级判据）。
//! 全部是无状态纯文本变换，且**幂等**：降级过的文本再跑一遍不变。

/// 钉钉单条 markdown 的字符上限（留出余量，官方约 5000 字节）
pub const DINGTALK_MAX_LEN: usize = 4000;

/// 把钉钉渲染不了的语法降级成可读纯文本。幂等。
pub fn downgrade_for_dingtalk(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut in_fence = false;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            i += 1; // 丢掉围栏行本身，围栏内内容原样保留
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

/// 从 markdown 正文提炼一行纯文本标题（钉钉 markdown 消息必须带 title，
/// 会话列表与通知里显示的就是它）。取首个非空行，剥掉块级/行内标记再截断。
pub fn derive_title(text: &str, fallback: &str, limit: usize) -> String {
    for line in text.lines() {
        let mut s = line.trim().to_string();
        if s.is_empty() {
            continue;
        }
        s = strip_images(&s);
        s = strip_link_urls(&s);
        s = s.trim_start_matches(['#', '>', ' ']).to_string();
        s = strip_list_prefix(&s);
        for mark in ["**", "__", "~~", "`", "*"] {
            s = s.replace(mark, "");
        }
        let s = s.trim();
        if !s.is_empty() {
            return s.chars().take(limit).collect();
        }
    }
    fallback.to_string()
}

/// `![alt](url)` → 整个丢掉
fn strip_images(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '!' && i + 1 < b.len() && b[i + 1] == '[' {
            if let Some(end) = find_link_end(&b, i + 1) {
                i = end + 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `[文字](url)` → 只留文字
fn strip_link_urls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '[' {
            if let Some((text_end, end)) = find_link_parts(&b, i) {
                out.extend(&b[i + 1..text_end]);
                i = end + 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// 从 `[` 位置找 `](...)` 的整体结束下标
fn find_link_end(b: &[char], open: usize) -> Option<usize> {
    find_link_parts(b, open).map(|(_, end)| end)
}

/// 返回 (文字结束的 `]` 下标, 整体结束的 `)` 下标)
fn find_link_parts(b: &[char], open: usize) -> Option<(usize, usize)> {
    let close = (open + 1..b.len()).find(|&k| b[k] == ']')?;
    if close + 1 >= b.len() || b[close + 1] != '(' {
        return None;
    }
    let end = (close + 2..b.len()).find(|&k| b[k] == ')')?;
    Some((close, end))
}

/// 去掉列表前缀：`- ` / `* ` / `+ ` / `1. `
fn strip_list_prefix(s: &str) -> String {
    let t = s.trim_start();
    for p in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(p) {
            return r.to_string();
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(r) = t[digits.len()..].strip_prefix(". ") {
            return r.to_string();
        }
    }
    t.to_string()
}

/// 按单条上限切分长文本，尽量断在换行、其次空格，避免拦腰截断。
/// `max_len == 0` 视为不限长。
pub fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 || text.chars().count() <= max_len {
        return if text.is_empty() { vec![] } else { vec![text.to_string()] };
    }
    let mut chunks = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > max_len {
        let window = &rest[..max_len];
        // 断点至少要落在后 40% 区域，否则宁可硬切 —— 断得太靠前会切出一堆碎片
        let floor = max_len * 6 / 10;
        let cut = window
            .iter()
            .rposition(|&c| c == '\n')
            .filter(|&p| p >= floor)
            .or_else(|| window.iter().rposition(|&c| c == ' ').filter(|&p| p >= floor))
            .unwrap_or(max_len);
        let piece: String = rest[..cut].iter().collect();
        chunks.push(piece.trim_end().to_string());
        rest = rest[cut..].to_vec();
        while matches!(rest.first(), Some(&c) if c == '\n' || c == ' ') {
            rest.remove(0);
        }
    }
    if !rest.is_empty() {
        chunks.push(rest.iter().collect());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表格原样保留：钉钉能渲染，转成列表反而丢了行列对照（这条曾断言相反，见模块注释）
    #[test]
    fn table_kept_as_is() {
        let src = "结果如下：\n| 文件 | 状态 |\n| --- | --- |\n| a.rs | 已改 |\n| b.rs | 跳过 |\n完毕";
        assert_eq!(downgrade_for_dingtalk(src), src);
    }

    /// 不规范的表格（缺分隔行）同样原样保留
    #[test]
    fn malformed_table_kept_as_is() {
        let src = "| 只有表头 | 没有分隔 |\n| a | b |";
        assert_eq!(downgrade_for_dingtalk(src), src);
    }

    /// 表格与围栏混排：围栏照常剥掉，表格分毫不动
    #[test]
    fn table_survives_alongside_fence() {
        let src = "| a | b |\n| --- | --- |\n| 1 | 2 |\n```\nlet x = 1;\n```";
        let out = downgrade_for_dingtalk(src);
        assert!(out.contains("| 1 | 2 |"), "表格行要原样留着：{out}");
        assert!(out.contains("let x = 1;"), "代码内容要保留：{out}");
        assert!(!out.contains("```"), "围栏行要去掉：{out}");
    }

    #[test]
    fn fence_lines_removed_code_kept() {
        let src = "看这段：\n```rust\nfn main() {}\n```\n就这样";
        let out = downgrade_for_dingtalk(src);
        assert!(out.contains("fn main() {}"), "代码内容要保留：{out}");
        assert!(!out.contains("```"), "围栏行要去掉：{out}");
    }

    /// 幂等：降级过的再跑一遍不变
    #[test]
    fn downgrade_is_idempotent() {
        let src = "| a | b |\n| --- | --- |\n| 1 | 2 |\n```\nx\n```";
        let once = downgrade_for_dingtalk(src);
        assert_eq!(downgrade_for_dingtalk(&once), once);
    }

    /// 围栏内的竖线不该被当表格转换
    #[test]
    fn table_inside_fence_untouched() {
        let src = "```\n| a | b |\n| --- | --- |\n| 1 | 2 |\n```";
        let out = downgrade_for_dingtalk(src);
        assert!(out.contains("| a | b |"), "围栏内应原样保留：{out}");
    }

    #[test]
    fn title_strips_markup() {
        assert_eq!(derive_title("## **任务完成**", "兜底", 24), "任务完成");
        assert_eq!(derive_title("- [看这里](https://x.com) 改好了", "兜底", 24), "看这里 改好了");
        assert_eq!(derive_title("![图](a.png) 标题在后面", "兜底", 24), "标题在后面");
        assert_eq!(derive_title("\n\n", "兜底", 24), "兜底");
    }

    #[test]
    fn chunk_breaks_at_newline() {
        let text = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let out = chunk_text(&text, 60);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "a".repeat(50), "应断在换行处：{out:?}");
        assert_eq!(out[1], "b".repeat(50));
    }

    /// 没有可断点时硬切，且不丢字符
    #[test]
    fn chunk_without_breakpoint_keeps_all_chars() {
        let text = "x".repeat(250);
        let out = chunk_text(&text, 100);
        assert_eq!(out.len(), 3);
        assert_eq!(out.concat().len(), 250);
    }

    #[test]
    fn chunk_short_text_untouched() {
        assert_eq!(chunk_text("短", 100), vec!["短".to_string()]);
        assert!(chunk_text("", 100).is_empty());
    }

    /// 中文按字符切，不能把多字节字符切坏
    #[test]
    fn chunk_handles_multibyte() {
        let text = "中".repeat(150);
        let out = chunk_text(&text, 100);
        assert_eq!(out.len(), 2);
        assert_eq!(out.concat().chars().count(), 150);
    }
}
