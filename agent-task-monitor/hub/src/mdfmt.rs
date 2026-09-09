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
//! 切分还必须**认得表格**：markdown 表格是「表头 + 分隔行 + 数据行」的整体，一旦从中间切开，
//! 后半片只剩数据行，各家渲染器都不再当表格看，用户收到的就是一大坨 `| a | b |` 字面量
//! （线上实拍过）。所以按行切、表头与分隔行绑在一起，被迫切开表格时在下一片开头补回表头。
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
/// 挑出正文里引用的**本地图片**路径（`![alt](rel/path.png)`，非 http/data 的那些），
/// 顺带把这些标记从正文里换成 `[图: alt]`。
///
/// 各渠道只认得公网 URL 或自家上传的媒体，本地相对路径原样发过去就是一串没用的字面量。
/// 所以正文里留个可读的占位，图另走各渠道自己的发图通路。
pub fn take_local_images(text: &str) -> (String, Vec<(String, String)>) {
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut found: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '!' && i + 1 < b.len() && b[i + 1] == '[' {
            if let Some(end) = find_link_end(&b, i + 1) {
                let whole: String = b[i..=end].iter().collect();
                if let Some((alt, src)) = split_image(&whole) {
                    let local = !src.starts_with("http://")
                        && !src.starts_with("https://")
                        && !src.starts_with("data:");
                    if local && !src.is_empty() {
                        out.push_str(&format!(
                            "[图: {}]",
                            if alt.is_empty() { &src } else { &alt }
                        ));
                        found.push((alt, src));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    (out, found)
}

/// 拆 `![alt](src)` —— 只在 take_local_images 里用，形态已由 find_link_end 保证
fn split_image(whole: &str) -> Option<(String, String)> {
    let rest = whole.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = &rest[..close];
    let src = rest[close + 2..].strip_suffix(')')?;
    Some((alt.to_string(), src.trim().to_string()))
}

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

/// 按单条上限切分长文本：按行装片，断点自然落在换行处；一行本身超长时才在空格处
/// 硬切。表格不会被切成「没有表头的半张」—— 见 `table_units`。
/// `max_len == 0` 视为不限长。
pub fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 || text.chars().count() <= max_len {
        return if text.is_empty() {
            vec![]
        } else {
            vec![text.to_string()]
        };
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_len = 0usize; // cur.join("\n") 的字符数

    for unit in table_units(text) {
        let mut text = unit.text;
        loop {
            let len = text.chars().count();
            let joined = if cur.is_empty() {
                len
            } else {
                cur_len + 1 + len
            };
            if joined <= max_len {
                break;
            }
            if !cur.is_empty() {
                // 收掉当前片，开新片；若切在表格中间，新片开头补回表头 + 分隔行，
                // 否则后半张表在钉钉那边只会渲染成一堆竖线。
                flush(&mut chunks, &mut cur, &mut cur_len);
                if let Some(head) = &unit.head {
                    let head_len = head.chars().count();
                    if head_len + 1 + len <= max_len {
                        cur.push(head.clone());
                        cur_len = head_len;
                    }
                }
                continue;
            }
            // 空片都装不下 → 这一行自己就超长，硬切（尽量断在空格）
            let (piece, rest) = hard_split(&text, max_len);
            if !piece.is_empty() {
                chunks.push(piece);
            }
            text = rest;
        }
        if !cur.is_empty() {
            cur_len += 1;
        }
        cur_len += text.chars().count();
        cur.push(text);
    }
    flush(&mut chunks, &mut cur, &mut cur_len);
    chunks
}

fn flush(chunks: &mut Vec<String>, cur: &mut Vec<String>, cur_len: &mut usize) {
    let piece = std::mem::take(cur).join("\n").trim_end().to_string();
    *cur_len = 0;
    if !piece.is_empty() {
        chunks.push(piece);
    }
}

/// 把一行超长文本切成 (前 max_len 内的一片, 余下)。断点优先落在后 40% 区域的空格。
fn hard_split(text: &str, max_len: usize) -> (String, String) {
    let b: Vec<char> = text.chars().collect();
    let floor = max_len * 6 / 10;
    // 断得太靠前会切出一堆碎片，所以够不着 floor 就宁可硬切
    let cut = b[..max_len]
        .iter()
        .rposition(|&c| c == ' ')
        .filter(|&p| p >= floor)
        .unwrap_or(max_len);
    let piece: String = b[..cut].iter().collect();
    let rest: String = b[cut..].iter().collect();
    (
        piece.trim_end().to_string(),
        rest.trim_start_matches(' ').to_string(),
    )
}

/// 切分的最小单位：一行普通文本，或**绑在一起的表头 + 分隔行**。
struct Unit {
    text: String,
    /// 本单位是表格数据行时，所属表格的「表头\n分隔行」—— 被切到新片时补在开头
    head: Option<String>,
}

/// 把文本拆成切分单位：识别 `表头 / |---| / 数据行…` 结构，让表头与分隔行不可分割，
/// 并给每个数据行记下自己的表头。不合法的表格（缺分隔行）按普通行处理 —— 反正也渲染不出来。
fn table_units(text: &str) -> Vec<Unit> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Unit> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if is_table_row(lines[i]) && i + 1 < lines.len() && is_separator_row(lines[i + 1]) {
            let head = format!("{}\n{}", lines[i], lines[i + 1]);
            out.push(Unit {
                text: head.clone(),
                head: None,
            });
            i += 2;
            while i < lines.len() && is_table_row(lines[i]) {
                out.push(Unit {
                    text: lines[i].to_string(),
                    head: Some(head.clone()),
                });
                i += 1;
            }
        } else {
            out.push(Unit {
                text: lines[i].to_string(),
                head: None,
            });
            i += 1;
        }
    }
    out
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.chars().count() > 1
}

/// `| --- | :--: |` 这类分隔行
fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    is_table_row(t) && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && t.contains('-')
}

#[cfg(test)]
mod img_tests {
    use super::*;

    #[test]
    fn takes_local_images_and_leaves_urls() {
        let (txt, imgs) = take_local_images(
            "结果见 ![交互提示](qa/evidence/x.png) 和 ![线上](https://a.com/b.png)",
        );
        // 本地的换成可读占位并挑出来；公网 URL 各渠道本来就认，原样留着
        assert_eq!(txt, "结果见 [图: 交互提示] 和 ![线上](https://a.com/b.png)");
        assert_eq!(
            imgs,
            vec![("交互提示".to_string(), "qa/evidence/x.png".to_string())]
        );
    }

    #[test]
    fn alt_falls_back_to_path() {
        let (txt, imgs) = take_local_images("![](shot.png)");
        assert_eq!(txt, "[图: shot.png]");
        assert_eq!(imgs.len(), 1);
    }

    #[test]
    fn plain_text_untouched() {
        let src = "普通正文，含感叹号！和方括号[不是图]";
        assert_eq!(take_local_images(src).0, src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表格原样保留：钉钉能渲染，转成列表反而丢了行列对照（这条曾断言相反，见模块注释）
    #[test]
    fn table_kept_as_is() {
        let src =
            "结果如下：\n| 文件 | 状态 |\n| --- | --- |\n| a.rs | 已改 |\n| b.rs | 跳过 |\n完毕";
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
        assert_eq!(
            derive_title("- [看这里](https://x.com) 改好了", "兜底", 24),
            "看这里 改好了"
        );
        assert_eq!(
            derive_title("![图](a.png) 标题在后面", "兜底", 24),
            "标题在后面"
        );
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

    fn long_table(rows: usize) -> String {
        let mut s = String::from("| 子模块 | 接口方法 |\n| --- | --- |\n");
        for i in 0..rows {
            s.push_str(&format!("| 模块{i} | postSomethingRatherLong{i} |\n"));
        }
        s.trim_end().to_string()
    }

    /// 病灶复现：表格被切成两片时，后一片必须自带表头 + 分隔行，否则渲染成一堆竖线
    #[test]
    fn chunk_repeats_table_header() {
        let out = chunk_text(&long_table(40), 400);
        assert!(out.len() >= 2, "这张表应该切成多片：{}", out.len());
        for (i, c) in out.iter().enumerate() {
            assert!(
                c.starts_with("| 子模块 | 接口方法 |\n| --- | --- |"),
                "第 {i} 片缺表头：{c}"
            );
            assert!(
                c.chars().count() <= 400,
                "第 {i} 片超长：{}",
                c.chars().count()
            );
        }
        // 数据行一行不丢、也不重复
        let rows: usize = out.iter().map(|c| c.matches("| 模块").count()).sum();
        assert_eq!(rows, 40, "数据行数对不上：{rows}");
    }

    /// 表头与分隔行绑在一起：不能一片以表头结尾、下一片以 `| --- |` 开头
    #[test]
    fn chunk_never_splits_header_from_separator() {
        // 让上文长度正好逼近上限，把断点顶到表头附近
        let text = format!("{}\n{}", "垫".repeat(180), long_table(6));
        for c in chunk_text(&text, 200) {
            assert!(
                !c.trim_start().starts_with("| ---"),
                "分隔行被切成了片首：{c}"
            );
            let last = c.lines().last().unwrap_or("");
            assert!(!last.starts_with("| 子模块"), "表头被留在了片尾：{c}");
        }
    }

    /// 表格外的正文不受影响，也不会平白多出表头
    #[test]
    fn chunk_plain_text_gets_no_header() {
        let text = format!("{}\n{}", "甲".repeat(80), "乙".repeat(80));
        let out = chunk_text(&text, 100);
        assert_eq!(out, vec!["甲".repeat(80), "乙".repeat(80)]);
    }

    /// 缺分隔行的「伪表格」按普通行处理，不补表头
    #[test]
    fn chunk_ignores_malformed_table() {
        let text = format!(
            "| 只有表头 | 没有分隔 |\n{}",
            "| a | b |\n".repeat(30).trim_end()
        );
        let out = chunk_text(&text, 120);
        assert!(out.len() > 1);
        assert!(
            !out[1].starts_with("| 只有表头"),
            "伪表格不该补表头：{}",
            out[1]
        );
    }
}
