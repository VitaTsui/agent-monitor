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

use am_core::configpath::{is_allowed, DIRS, MAX_FILE_BYTES, SINGLE_FILES};
use am_core::model::{ConfigFileBody, ConfigFileMeta, ConfigManifest, ConfigPush};
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
    for (root, prefix) in [
        (home.join(".claude"), "claude"),
        (home.join(".codex"), "codex"),
    ] {
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
            let Some(dir) = dir_abs(home, d) else {
                continue;
            };
            collect_md(&dir, 0, &mut paths);
        }
        paths.sort();
        paths.truncate(MAX_FILES);

        let mut files = Vec::with_capacity(paths.len());
        let mut next_cache = HashMap::with_capacity(paths.len());
        for p in paths {
            let Some(rel) = rel_of(home, &p) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&p) else {
                continue;
            };
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
            files.push(ConfigFileMeta {
                path: rel,
                sha256: sha,
                size,
                mtime,
            });
        }
        self.cache = next_cache;
        ConfigManifest {
            files,
            scanned_at: now_secs(),
        }
    }
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

/// 递归收集目录下的 .md。跟随符号链接会成环，靠深度上限兜住。
fn collect_md(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_md(&p, depth + 1, out),
            Ok(_) if name.ends_with(".md") => out.push(p),
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
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&abs) else {
            continue;
        };
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
            tracing::warn!(
                "配置内容哈希不符，跳过: {}（期望 {}）",
                push.path,
                push.sha256
            );
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
        tracing::info!("已同步配置: {}", push.path);
        done += 1;
    }
    done
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
