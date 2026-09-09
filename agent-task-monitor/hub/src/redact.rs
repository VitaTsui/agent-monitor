//! 会话历史入库前的密钥脱敏：**只认结构，不认字面量**。
//!
//! 为什么要有这层：远程交互历史（见 crate::history）会把代理的最终报告原样存进
//! history.json，再经 `GET /monitor/history` 下发。报告里只要贴过一次私钥，
//! 这份私钥就落了盘、进了内存、还会发给所有能读该账号历史的端。**存下来就是风险**，
//! 所以脱敏必须发生在入库那一刻。
//!
//! 为什么不用标记词匹配：**谈论密钥的正常文本和真的密钥长得很像**。
//! 「核验：`BEGIN PRIVATE KEY` 0 命中」这句话里出现了标记词，却一个字节的密钥都没有；
//! 按 `contains("BEGIN PRIVATE KEY")` 判定就会把一句正常的话改花。本项目此前已经因为
//! 「字面量扫全文」误杀过运行中的子代理，同样的错不犯第二次。
//!
//! 判据只有一条：**形态**。
//! - PEM 私钥：`-----BEGIN <私钥标签>-----` ＋ **足量的 base64 主体**（≥48 字符）。
//!   只有标记、后面没有主体的，一个字节都不动。证书（CERTIFICATE / PUBLIC KEY）也不动 ——
//!   它们本来就是公开的。
//! - 令牌：只认**前缀 + 长度都固定**的那几家（GitHub / Anthropic / OpenAI / Slack / AWS）。
//!   像 `AM_AGENT_TOKEN` 那种纯 32 位随机串没有任何可辨形态，**认不出就不认**，
//!   宁可漏也不能把正常文本改花。

/// PEM 私钥主体的最小 base64 长度。
/// 下界由**最短的合法私钥**决定：PKCS#8 的 Ed25519 私钥只有 48 字节 → 64 个 base64 字符。
/// 取 48 既能覆盖它，又要求主体是「一大段连续 base64」，正常散文不可能撞上。
const MIN_PEM_BODY: usize = 48;

const BEGIN: &str = "-----BEGIN ";
const DASHES: &str = "-----";

const PEM_PLACEHOLDER: &str = "[REDACTED:private-key]";

/// 入库脱敏入口：先清 PEM 私钥块，再清有固定形态的令牌。
pub fn redact_secrets(text: &str) -> String {
    let pem = redact_pem(text);
    redact_tokens(&pem)
}

// ---------------------------------------------------------------- PEM 私钥

/// 私钥标签判定：全大写 ASCII（含空格/数字），且以 `PRIVATE KEY` 结尾
/// —— `PRIVATE KEY` / `RSA PRIVATE KEY` / `EC PRIVATE KEY` / `DSA PRIVATE KEY` /
/// `ENCRYPTED PRIVATE KEY` / `OPENSSH PRIVATE KEY` / `PGP PRIVATE KEY BLOCK` 都在内。
/// 靠**后缀**而不是穷举列表，将来出现新的私钥格式也自动覆盖；
/// `CERTIFICATE` / `PUBLIC KEY` 这些公开物天然落在外面。
fn is_private_key_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 64 {
        return false;
    }
    if !label
        .bytes()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b' ')
    {
        return false;
    }
    label.ends_with("PRIVATE KEY") || label.ends_with("PRIVATE KEY BLOCK")
}

fn is_b64(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

/// 分隔符宽度：ASCII 空白算 1；JSON 字符串里常见的**字面转义** `\n` / `\r` / `\t`
/// 算 2 —— 一份把 .env 或 JSON 原样贴进来的报告，整条私钥会挤在物理上的一行里，
/// 不认这种转义就等于漏掉一整类形态。
fn delim_len(b: &[u8], i: usize) -> usize {
    if b[i].is_ascii_whitespace() {
        1
    } else if b[i] == b'\\' && i + 1 < b.len() && matches!(b[i + 1], b'n' | b'r' | b't') {
        2
    } else {
        0
    }
}

/// `text[at..]` 是否正好是 `-----END <label>-----`；是则返回它的结束位置。
fn end_marker_end(text: &str, at: usize, label: &str) -> Option<usize> {
    let want_len = "-----END ".len() + label.len() + DASHES.len();
    let end = at + want_len;
    if end > text.len() || !text.is_char_boundary(end) {
        return None;
    }
    let seg = &text[at..end];
    (seg.starts_with("-----END ") && seg[9..].starts_with(label) && seg.ends_with(DASHES))
        .then_some(end)
}

fn redact_pem(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize; // 已经原样输出到哪儿
    let mut scan = 0usize; // 下一次找 BEGIN 的起点

    while let Some(rel) = text[scan..].find(BEGIN) {
        let head = scan + rel;
        let after_label = head + BEGIN.len();
        let Some(close_rel) = text[after_label..].find(DASHES) else {
            break; // 没有闭合的 `-----`，后面不可能再有完整的头
        };
        let label = &text[after_label..after_label + close_rel];
        let body_from = after_label + close_rel + DASHES.len();
        if !is_private_key_label(label) {
            scan = body_from;
            continue;
        }

        // 主体扫描：逐「词」前进，只收纯 base64 的词。
        // 开头允许跳过 RFC 1421 头（`Proc-Type: 4,ENCRYPTED` / `DEK-Info: AES-128-CBC,…`），
        // 它们出现在加密私钥里，不跳就会在第一行停下、把真钥漏出去。
        let mut k = body_from;
        let mut b64 = 0usize;
        let mut body_end = body_from;
        let mut hdr_skips = 0usize;
        let mut want_hdr_value = false;
        loop {
            while k < b.len() {
                let d = delim_len(b, k);
                if d == 0 {
                    break;
                }
                k += d;
            }
            if k >= b.len() {
                break;
            }
            let ts = k;
            while k < b.len() && delim_len(b, k) == 0 {
                k += 1;
            }
            let tok = &text[ts..k];
            if b64 == 0 && hdr_skips < 8 && (want_hdr_value || tok.ends_with(':')) {
                want_hdr_value = !want_hdr_value;
                hdr_skips += 1;
                continue;
            }
            if tok.len() >= 4 && tok.bytes().all(is_b64) {
                b64 += tok.len();
                body_end = k;
                continue;
            }
            // 不是主体了。若前面已收够主体，顺手把紧随的 `-----END …-----` 一并吃掉。
            if b64 >= MIN_PEM_BODY {
                if let Some(e) = end_marker_end(text, ts, label) {
                    body_end = e;
                }
            }
            break;
        }

        if b64 < MIN_PEM_BODY {
            // 只有标记、没有主体 —— 这是在**谈论**密钥，不是密钥本身。原样留着。
            scan = body_from;
            continue;
        }

        out.push_str(&text[cursor..head]);
        out.push_str(PEM_PLACEHOLDER);
        cursor = body_end;
        scan = body_end;
    }
    out.push_str(&text[cursor..]);
    out
}

// ------------------------------------------------------------------- 令牌

/// 一条令牌形态：固定前缀 ＋ 固定字符集 ＋ 固定长度区间。
/// 三样都对上才算，少一样就不认 —— 这是「宁可漏也不误伤」的具体落法。
struct TokenShape {
    /// 可能的前缀（同一家的多个变体）
    prefixes: &'static [&'static str],
    /// 前缀之后允许出现的字符
    body: fn(u8) -> bool,
    min: usize,
    max: usize,
    /// 主体里必须至少有一个数字（专治 AWS 那种「全大写字母也合法」的形态被英文词撞上）
    need_digit: bool,
    placeholder: &'static str,
}

fn alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}
fn alnum_us(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
fn alnum_dash_us(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}
fn alnum_dash(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}
fn upper_num(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

const SHAPES: &[TokenShape] = &[
    // GitHub 经典令牌：前缀 + 恰好 36 位 base62
    TokenShape {
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        body: alnum,
        min: 36,
        max: 36,
        need_digit: false,
        placeholder: "[REDACTED:github-token]",
    },
    // GitHub 细粒度 PAT：github_pat_ + 长串（官方 82 位，留出余量）
    TokenShape {
        prefixes: &["github_pat_"],
        body: alnum_us,
        min: 70,
        max: 200,
        need_digit: false,
        placeholder: "[REDACTED:github-token]",
    },
    TokenShape {
        prefixes: &["sk-ant-"],
        body: alnum_dash_us,
        min: 24,
        max: 200,
        need_digit: false,
        placeholder: "[REDACTED:anthropic-key]",
    },
    TokenShape {
        prefixes: &["sk-proj-"],
        body: alnum_dash_us,
        min: 24,
        max: 200,
        need_digit: false,
        placeholder: "[REDACTED:openai-key]",
    },
    TokenShape {
        prefixes: &["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxe-", "xoxr-"],
        body: alnum_dash,
        min: 20,
        max: 120,
        need_digit: false,
        placeholder: "[REDACTED:slack-token]",
    },
    // AWS Access Key ID：AKIA/ASIA + 恰好 16 位大写数字，且至少含一个数字
    TokenShape {
        prefixes: &["AKIA", "ASIA"],
        body: upper_num,
        min: 16,
        max: 16,
        need_digit: true,
        placeholder: "[REDACTED:aws-access-key-id]",
    },
];

/// 词边界：前一个字符不能是令牌自身可能包含的字符，否则 `xxghp_…` 这种半截也会中招。
fn at_boundary(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'-')
}

fn redact_tokens(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        // 非 ASCII 字节直接跳过：所有前缀都是 ASCII，不可能从多字节字符中间开始，
        // 而按字节切 &str 会在非字符边界上炸。
        if !b[i].is_ascii() || !at_boundary(b, i) {
            i += 1;
            continue;
        }
        let mut hit = None;
        'shapes: for s in SHAPES {
            for p in s.prefixes {
                if !text[i..].starts_with(p) {
                    continue;
                }
                let start = i + p.len();
                let mut end = start;
                while end < b.len() && (s.body)(b[end]) {
                    end += 1;
                }
                let len = end - start;
                if len < s.min || len > s.max {
                    continue;
                }
                if s.need_digit && !b[start..end].iter().any(u8::is_ascii_digit) {
                    continue;
                }
                hit = Some((end, s.placeholder));
                break 'shapes;
            }
        }
        match hit {
            Some((end, placeholder)) => {
                out.push_str(&text[cursor..i]);
                out.push_str(placeholder);
                cursor = end;
                i = end;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一段够长的 base64 主体（内容无意义，判据本来就只看形态）
    fn body(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC{i:015}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn pem(label: &str) -> String {
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----",
            body(3)
        )
    }

    /// 真私钥必须被替换，且整块（含头尾）一起消失
    #[test]
    fn redacts_real_pem_blocks() {
        for label in [
            "PRIVATE KEY",
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "DSA PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
            "OPENSSH PRIVATE KEY",
            "PGP PRIVATE KEY BLOCK",
        ] {
            let text = format!("报告正文：\n{}\n（以上）", pem(label));
            let got = redact_secrets(&text);
            assert_eq!(
                got, "报告正文：\n[REDACTED:private-key]\n（以上）",
                "{label} 没被完整脱敏"
            );
        }
    }

    /// **硬要求**：谈论密钥的正常文本一个字节都不许改。
    /// 第一条就是今天真实踩到的那句 —— 有人拿 `contains("BEGIN PRIVATE KEY")` 判定，
    /// 命中的正是这句话本身。
    #[test]
    fn never_touches_text_that_merely_talks_about_keys() {
        let cases = [
            "核验：`BEGIN PRIVATE KEY` 0 命中",
            "grep -c 'BEGIN PRIVATE KEY' history.json  # 结果 0",
            "报告里不许出现 -----BEGIN PRIVATE KEY----- 这种东西",
            "-----BEGIN PRIVATE KEY-----\n（这里本该有密钥，但我没贴）\n-----END PRIVATE KEY-----",
            "PEM 的头是 -----BEGIN RSA PRIVATE KEY-----，尾是 -----END RSA PRIVATE KEY-----。",
            "私钥格式说明：BEGIN/END 之间是 base64。",
            "已核对 sk-ant- 开头的 key 未泄露；ghp_ 前缀的令牌也没有。",
            "AKIAMOUNTAINPATHXXXX 是我随手编的大写词",
        ];
        for c in cases {
            assert_eq!(redact_secrets(c), c, "误伤了正常文本：{c}");
        }
    }

    /// 公开物不动：证书、公钥贴出来本就无害，改了反而丢信息
    #[test]
    fn keeps_public_pem() {
        for label in ["CERTIFICATE", "PUBLIC KEY", "RSA PUBLIC KEY"] {
            let text = pem(label);
            assert_eq!(redact_secrets(&text), text, "{label} 不该被改");
        }
    }

    /// 加密私钥带 RFC 1421 头，不能在第一行就停下
    #[test]
    fn redacts_encrypted_pem_with_headers() {
        let text = format!(
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,1F2E3D4C5B6A7988\n\n{}\n-----END RSA PRIVATE KEY-----",
            body(2)
        );
        assert_eq!(redact_secrets(&text), PEM_PLACEHOLDER);
    }

    /// JSON / .env 里整条挤成一行（`\n` 是字面转义）也要认
    #[test]
    fn redacts_escaped_single_line_pem() {
        let text = format!(
            "{{\"key\":\"-----BEGIN PRIVATE KEY-----\\n{}\\n-----END PRIVATE KEY-----\"}}",
            body(2).replace('\n', "\\n")
        );
        assert_eq!(
            redact_secrets(&text),
            "{\"key\":\"[REDACTED:private-key]\"}"
        );
    }

    /// 尾巴被截断的私钥（没有 END）同样要清 —— 半截主体照样是泄露
    #[test]
    fn redacts_truncated_pem() {
        let text = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}", body(2));
        assert_eq!(redact_secrets(&text), PEM_PLACEHOLDER);
    }

    /// 一段文本里多块私钥要全清，块与块之间的正文原样保留
    #[test]
    fn redacts_multiple_blocks_and_keeps_surroundings() {
        let text = format!("前{}中{}后", pem("PRIVATE KEY"), pem("EC PRIVATE KEY"));
        assert_eq!(
            redact_secrets(&text),
            "前[REDACTED:private-key]中[REDACTED:private-key]后"
        );
    }

    /// 形态明确的令牌要清
    #[test]
    fn redacts_shaped_tokens() {
        let cases = [
            (
                format!("token=ghp_{}", "a1B2c3D4e5".repeat(3) + "abcdef"),
                "token=[REDACTED:github-token]",
            ),
            (
                format!("Bearer sk-ant-api03-{}", "x9Y8z7W6".repeat(4)),
                "Bearer [REDACTED:anthropic-key]",
            ),
            (
                format!("key: sk-proj-{}", "Ab3Cd4Ef".repeat(4)),
                "key: [REDACTED:openai-key]",
            ),
            (
                format!("xoxb-{}", "12345678-".repeat(3) + "abcdefgh"),
                "[REDACTED:slack-token]",
            ),
            (
                "AKIAIOSFODNN7EXAMPLE".to_string(),
                "[REDACTED:aws-access-key-id]",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact_secrets(&input), want, "没清掉：{input}");
        }
    }

    /// 长度对不上的一律不认（宁可漏也不误伤）
    #[test]
    fn ignores_shape_mismatch() {
        let cases = [
            "ghp_tooshort",                               // 位数不够
            "ghp_",                                       // 只有前缀
            "sk-ant-x",                                   // 主体太短
            "xoxb-123",                                   // 主体太短
            "AKIAABCDEFGHIJKLMNOP",                       // 全字母、无数字
            "myghp_a1B2c3D4e5a1B2c3D4e5a1B2c3D4e5abcdef", // 不在词边界
        ];
        for c in cases {
            assert_eq!(redact_secrets(c), c, "误伤：{c}");
        }
    }

    /// 空串 / 无密钥文本走快路径不出错
    #[test]
    fn passthrough_plain_text() {
        for c in ["", "一切正常，没有任何密钥。", "a\nb\nc"] {
            assert_eq!(redact_secrets(c), c);
        }
    }
}
