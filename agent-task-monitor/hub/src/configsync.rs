//! 配置同步的服务端基线（hub 侧）。
//!
//! 模型是**单向镜像**：每个账号指定一台设备作为「配置源」，hub 把源机的在管配置存成一份
//! 基线，其余设备与基线比对、缺什么补什么。
//!
//! 之所以不做双向自动合并：两台机器同时改同一个 CLAUDE.md 时，任何自动合并都会在用户
//! 毫不知情的情况下丢掉一边的内容。宁可让用户显式选一台为准。
//!
//! 落盘布局：
//! ```text
//! <data_dir>/configs/<safe_user>/manifest.json   基线清单（含账号名，目录名是哈希化的）
//! <data_dir>/configs/<safe_user>/files/<rel>     文件内容，<rel> 同 ConfigFileMeta::path
//! ```

use am_core::configpath::{
    is_allowed, is_syncable_field, looks_machine_specific, portable_hooks, portable_mcp,
    MAX_FILE_BYTES,
};
use am_core::model::{
    ConfigChange, ConfigFileBody, ConfigManifest, ConfigPatch, ConfigProbe, ConfigPush,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 配置项改动记录的保留条数（给人看近期动态，不是审计日志）
const MAX_CHANGES: usize = 50;

/// 单轮向源机索要的文件数上限
pub const MAX_PULLS_PER_ROUND: usize = 3;
/// 单轮向镜像机下发的文件数上限。上报响应与心跳同频（1.5s），一次塞太多会把响应撑爆；
/// 首同步分几十轮完成也就一分钟左右，不值得为此冒风险。
pub const MAX_PUSHES_PER_ROUND: usize = 3;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 账号名 → 目录名。
///
/// 账号名可能是邮箱（含 `@`、`.`），理论上也可能含 `/` 或 `..` —— 直接拿来当目录名
/// 就是一个路径穿越。这里只留字母数字与 `-_`，再拼上原名哈希的前 12 位，
/// 既不越界，也不会让 `a.b` 与 `a_b` 归一化后撞进同一个目录。
fn safe_user(user: &str) -> String {
    let mut s: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(32)
        .collect();
    if s.is_empty() {
        s.push('u');
    }
    format!("{s}-{}", &sha256_hex(user.as_bytes())[..12])
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedBaseline {
    /// 账号名。目录名是哈希化的，反查不回来，所以要在文件里带一份。
    user: String,
    manifest: ConfigManifest,
}

/// 各账号的配置基线。内存里持有清单，文件内容留在磁盘（按需读）。
pub struct ConfigStore {
    dir: PathBuf,
    /// 账号 → 基线清单
    manifests: HashMap<String, ConfigManifest>,
}

impl ConfigStore {
    /// 从 `<data_dir>/configs` 加载全部账号的基线清单。
    ///
    /// 单个账号的 manifest.json 坏掉只丢那个账号的基线（下次源机上报会重建），
    /// 不像注册表那样必须拒绝启动 —— 配置基线是可重建的派生数据。
    pub fn load(data_dir: &Path) -> Self {
        let dir = data_dir.join("configs");
        let _ = std::fs::create_dir_all(&dir);
        let mut manifests = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let path = e.path().join("manifest.json");
                let Ok(txt) = std::fs::read_to_string(&path) else { continue };
                match serde_json::from_str::<PersistedBaseline>(&txt) {
                    Ok(p) => {
                        manifests.insert(p.user, p.manifest);
                    }
                    Err(e) => tracing::warn!("配置基线 {} 解析失败，将由源机重建: {e}", path.display()),
                }
            }
        }
        if !manifests.is_empty() {
            tracing::info!("已加载 {} 个账号的配置基线", manifests.len());
        }
        Self { dir, manifests }
    }

    fn user_dir(&self, user: &str) -> PathBuf {
        self.dir.join(safe_user(user))
    }

    /// 基线里某文件的绝对路径。路径先过白名单，`rel` 不可信（来自客户端上报）。
    fn file_path(&self, user: &str, rel: &str) -> Option<PathBuf> {
        if !is_allowed(rel) {
            return None;
        }
        let mut p = self.user_dir(user).join("files");
        for seg in rel.split('/') {
            p.push(seg);
        }
        Some(p)
    }

    /// 某账号的基线清单（没有则空清单）
    pub fn manifest_of(&self, user: &str) -> ConfigManifest {
        self.manifests.get(user).cloned().unwrap_or_default()
    }

    /// 基线里有多少份文件
    pub fn file_count(&self, user: &str) -> usize {
        self.manifests.get(user).map(|m| m.files.len()).unwrap_or(0)
    }

    /// 把源机回传的一份文件写进基线。返回是否写成功。
    ///
    /// 三关：路径在白名单内、体积没超限、内容哈希与声明一致。第三关拦的是传输截断——
    /// 半截的 markdown 照样能解析，静默分发给所有设备之后没人会发现。
    pub fn put(&mut self, user: &str, body: &ConfigFileBody) -> bool {
        let Some(path) = self.file_path(user, &body.path) else {
            tracing::warn!("拒绝写入非同步集配置到基线: {}", body.path);
            return false;
        };
        let Ok(bytes) = B64.decode(body.content_b64.as_bytes()) else {
            tracing::warn!("配置内容解码失败: {}", body.path);
            return false;
        };
        if bytes.len() as u64 > MAX_FILE_BYTES {
            tracing::warn!("配置文件超限，拒绝入基线: {}", body.path);
            return false;
        }
        let actual = sha256_hex(&bytes);
        if actual != body.sha256 {
            tracing::warn!("配置哈希不符，拒绝入基线: {}", body.path);
            return false;
        }
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建基线目录失败 {}: {e}", parent.display());
                return false;
            }
        }
        // 原子写：半截文件会被后续 get() 当成完好内容分发出去
        let tmp = path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            tracing::warn!("写入基线临时文件失败 {}: {e}", body.path);
            return false;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("替换基线文件失败 {}: {e}", body.path);
            return false;
        }

        let m = self.manifests.entry(user.to_string()).or_default();
        let meta = am_core::model::ConfigFileMeta {
            path: body.path.clone(),
            sha256: actual,
            size: bytes.len() as u64,
            mtime: crate::state::now_secs(),
        };
        match m.files.iter_mut().find(|f| f.path == body.path) {
            Some(slot) => *slot = meta,
            None => m.files.push(meta),
        }
        m.scanned_at = crate::state::now_secs();
        self.save_user(user);
        true
    }

    /// 读基线里的一份文件，装成可直接下发的结构。
    ///
    /// 内容哈希当场重算而不是照抄清单：清单与文件是两次写，中间 hub 被杀就会对不上，
    /// 此时宁可不发（下一轮源机上报会把基线补齐），也不能发一份带着错误哈希的内容——
    /// 客户端会拒收，然后每一轮都重试同一个文件，卡死在这里。
    pub fn get(&self, user: &str, rel: &str) -> Option<ConfigPush> {
        let path = self.file_path(user, rel)?;
        let bytes = std::fs::read(&path).ok()?;
        Some(ConfigPush {
            path: rel.to_string(),
            sha256: sha256_hex(&bytes),
            content_b64: B64.encode(&bytes),
        })
    }

    /// 把配置源的字段值写进基线（字段级同步）。
    ///
    /// 入库前**再过一遍白名单**：上报来自客户端，不能因为「它说这是可同步字段」就照收——
    /// 一台被控设备否则就能把 `hooks` 塞进基线，再由 hub 分发到该账号的所有机器上。
    pub fn put_patches(&mut self, user: &str, patches: &[ConfigPatch]) -> bool {
        let mut clean = Vec::new();
        for p in patches {
            let mut fields = std::collections::BTreeMap::new();
            for (k, v) in &p.fields {
                if !is_syncable_field(&p.file, k) {
                    tracing::warn!("拒绝把字段 {}:{k} 收进基线", p.file);
                    continue;
                }
                // hooks 只收「通用」条目：客户端可能被控，不能让它把带 _source 标记的
                // 条目或指向本机路径的 hook 塞进基线，再由 hub 分发到该账号的所有机器
                if k == "hooks" {
                    match portable_hooks(v) {
                        Some(p) => {
                            fields.insert(k.clone(), p);
                        }
                        None => tracing::warn!("hooks 无可同步条目，不入基线"),
                    }
                    continue;
                }
                // mcpServers 同样走专门通道：兜底剥一次 env（客户端可能被控，
                // 不能让密钥落进基线再分发）。路径归一化只有客户端做得了——
                // hub 不知道对端 home，传空串即跳过归一化。
                // 也因此不能对它跑 looks_machine_specific：MCP 命令合法地含
                // /opt/homebrew 这类安装路径，一刀切会把整份配置滤没。
                if k == "mcpServers" {
                    match portable_mcp(v, "") {
                        Some(p) => {
                            fields.insert(k.clone(), p);
                        }
                        None => tracing::warn!("mcpServers 为空，不入基线"),
                    }
                    continue;
                }
                if looks_machine_specific(v) {
                    tracing::warn!("拒绝把字段 {}:{k} 收进基线", p.file);
                    continue;
                }
                fields.insert(k.clone(), v.clone());
            }
            if !fields.is_empty() {
                clean.push(ConfigPatch { file: p.file.clone(), fields });
            }
        }
        if clean == self.patches_of(user) {
            return false;
        }
        let dir = self.user_dir(user);
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let Ok(txt) = serde_json::to_string_pretty(&clean) else { return false };
        let path = dir.join("patches.json");
        let tmp = dir.join("patches.json.tmp");
        if std::fs::write(&tmp, &txt).is_err() {
            return false;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        true
    }

    /// 基线里的字段值（没有则空）
    pub fn patches_of(&self, user: &str) -> Vec<ConfigPatch> {
        let path = self.user_dir(user).join("patches.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// 记一条配置项改动。**同一台设备的同一字段、目标值没变时不重复记** ——
    /// 下发后客户端要到下一次扫描（30s）才报回新值，中间每一轮都会重新算出同样的差异，
    /// 不去重的话历史里会瞬间堆满几十条一模一样的记录。
    pub fn append_change(&mut self, user: &str, change: ConfigChange) {
        let mut list = self.changes_of(user);
        if let Some(last) = list
            .iter()
            .rev()
            .find(|c| c.machine_id == change.machine_id && c.file == change.file && c.field == change.field)
        {
            if last.to == change.to {
                return;
            }
        }
        list.push(change);
        // 只留最近这些条：这是给人看的近期动态，不是审计日志
        let len = list.len();
        if len > MAX_CHANGES {
            list.drain(..len - MAX_CHANGES);
        }
        let dir = self.user_dir(user);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(txt) = serde_json::to_string_pretty(&list) else { return };
        let path = dir.join("changes.json");
        let tmp = dir.join("changes.json.tmp");
        if std::fs::write(&tmp, &txt).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// 该账号的配置项改动记录（旧→新）
    pub fn changes_of(&self, user: &str) -> Vec<ConfigChange> {
        std::fs::read_to_string(self.user_dir(user).join("changes.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// 存下某设备的结构化配置字段普查（二期定白名单用）。
    ///
    /// 按设备分开存而不是合并：不同机器上同一个字段可能一台是机器相关、另一台不是
    /// （例如 `statusLine` 在 A 上填的是绝对路径、在 B 上是命令名），合并会把这个信息抹平，
    /// 而它恰恰是「该字段能不能跨机同步」的判据。
    pub fn put_probe(&self, user: &str, machine_id: &str, probes: &[ConfigProbe]) {
        if probes.is_empty() {
            return;
        }
        let dir = self.user_dir(user).join("probes");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        // machine_id 来自客户端上报，同样不能直接当文件名
        let name: String = machine_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .take(64)
            .collect();
        let Ok(txt) = serde_json::to_string_pretty(probes) else { return };
        let path = dir.join(format!("{name}.json"));
        let tmp = dir.join(format!("{name}.json.tmp"));
        if std::fs::write(&tmp, &txt).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// 读回某账号全部设备的普查结果：(machine_id 安全名, 普查)
    pub fn probes_of(&self, user: &str) -> Vec<(String, Vec<ConfigProbe>)> {
        let dir = self.user_dir(user).join("probes");
        let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(&path) else { continue };
            let Ok(probes) = serde_json::from_str::<Vec<ConfigProbe>>(&txt) else { continue };
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            out.push((name, probes));
        }
        out
    }

    /// 只保留 `keep` 里的路径，其余从基线清单与磁盘上移除，返回移除份数。
    ///
    /// 用于「源机删掉了某份配置」：基线跟着删，镜像机就不会再被推这份文件。
    /// 注意这只影响基线——镜像机上已有的那份**不删**（第一期不同步删除，
    /// 误删别人机器上的文件是不可逆的，而多留一份最多是碍眼）。
    pub fn retain(&mut self, user: &str, keep: &std::collections::HashSet<&str>) -> usize {
        let Some(m) = self.manifests.get_mut(user) else { return 0 };
        let before = m.files.len();
        let mut dropped = Vec::new();
        m.files.retain(|f| {
            let hit = keep.contains(f.path.as_str());
            if !hit {
                dropped.push(f.path.clone());
            }
            hit
        });
        if dropped.is_empty() {
            return 0;
        }
        for rel in &dropped {
            if let Some(p) = self.file_path(user, rel) {
                let _ = std::fs::remove_file(p);
            }
        }
        self.save_user(user);
        before - self.manifests.get(user).map(|m| m.files.len()).unwrap_or(0)
    }

    fn save_user(&self, user: &str) {
        let Some(manifest) = self.manifests.get(user) else { return };
        let dir = self.user_dir(user);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("创建基线目录失败 {}: {e}", dir.display());
            return;
        }
        let p = PersistedBaseline { user: user.to_string(), manifest: manifest.clone() };
        let Ok(txt) = serde_json::to_string_pretty(&p) else { return };
        let path = dir.join("manifest.json");
        let tmp = dir.join("manifest.json.tmp");
        if std::fs::write(&tmp, &txt).is_err() {
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("基线清单落盘失败: {e}");
        }
    }
}

/// `from` 里有、而 `to` 里缺失或内容不同的路径。
///
/// 只比哈希，不比 mtime：各机器时钟不保证同步，按 mtime 判新旧会让「时钟慢的那台」
/// 永远被判成落后，于是每一轮都被推同一批文件。
pub fn diff(from: &ConfigManifest, to: &ConfigManifest) -> Vec<String> {
    let have: HashMap<&str, &str> =
        to.files.iter().map(|f| (f.path.as_str(), f.sha256.as_str())).collect();
    from.files
        .iter()
        .filter(|f| have.get(f.path.as_str()) != Some(&f.sha256.as_str()))
        .map(|f| f.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use am_core::model::ConfigFileMeta;

    fn meta(path: &str, sha: &str) -> ConfigFileMeta {
        ConfigFileMeta { path: path.into(), sha256: sha.into(), size: 1, mtime: 0 }
    }

    fn body(path: &str, content: &[u8]) -> ConfigFileBody {
        ConfigFileBody {
            path: path.into(),
            content_b64: B64.encode(content),
            sha256: sha256_hex(content),
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("am-hub-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn diff_reports_missing_and_changed_only() {
        let base = ConfigManifest {
            files: vec![meta("claude/CLAUDE.md", "aa"), meta("claude/agents/x.md", "bb")],
            scanned_at: 0,
        };
        let dev = ConfigManifest {
            files: vec![meta("claude/CLAUDE.md", "aa"), meta("claude/agents/x.md", "OLD")],
            scanned_at: 0,
        };
        assert_eq!(diff(&base, &dev), vec!["claude/agents/x.md".to_string()]);
        // 完全一致 → 无差异
        assert!(diff(&base, &base).is_empty());
        // 设备侧多出来的文件不算差异（第一期不同步删除）
        assert!(diff(&dev, &base).len() <= 1);
    }

    #[test]
    fn put_then_get_roundtrip() {
        let d = tmp_dir("rt");
        let mut store = ConfigStore::load(&d);
        assert!(store.put("a@b.com", &body("claude/CLAUDE.md", b"hello")));
        assert_eq!(store.file_count("a@b.com"), 1);

        let push = store.get("a@b.com", "claude/CLAUDE.md").expect("应能读回");
        assert_eq!(B64.decode(push.content_b64).unwrap(), b"hello");
        assert_eq!(push.sha256, sha256_hex(b"hello"));

        // 重启后基线还在
        let reloaded = ConfigStore::load(&d);
        assert_eq!(reloaded.file_count("a@b.com"), 1);
        assert_eq!(reloaded.manifest_of("a@b.com").files[0].path, "claude/CLAUDE.md");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn put_rejects_bad_path_hash_and_oversize() {
        let d = tmp_dir("rej");
        let mut store = ConfigStore::load(&d);

        // 非同步集
        assert!(!store.put("u", &body("claude/settings.json", b"{}")));
        // 路径穿越
        assert!(!store.put("u", &body("claude/agents/../../x.md", b"x")));
        // 哈希不符
        let mut bad = body("claude/CLAUDE.md", b"hello");
        bad.sha256 = "deadbeef".into();
        assert!(!store.put("u", &bad));
        // 超限
        let big = vec![b'x'; (MAX_FILE_BYTES + 1) as usize];
        assert!(!store.put("u", &body("claude/CLAUDE.md", &big)));

        assert_eq!(store.file_count("u"), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn put_patches_filters_blacklisted_fields() {
        let d = tmp_dir("patch");
        let mut store = ConfigStore::load(&d);
        let patch = ConfigPatch {
            file: "claude/settings.json".into(),
            fields: [
                ("model".to_string(), serde_json::json!("opus")),
                // 客户端可能被控 —— hub 必须自己再过一遍白名单
                ("hooks".to_string(), serde_json::json!({"evil": "x"})),
                ("apiKeyHelper".to_string(), serde_json::json!("/Users/a/k.sh")),
                // 白名单字段但值是本机路径
                ("model2".to_string(), serde_json::json!("/abs/path")),
            ]
            .into_iter()
            .collect(),
        };
        assert!(store.put_patches("u", &[patch]));

        let saved = store.patches_of("u");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].fields.keys().collect::<Vec<_>>(), vec!["model"]);
        assert_eq!(saved[0].fields["model"], serde_json::json!("opus"));

        // 重启后还在
        assert_eq!(ConfigStore::load(&d).patches_of("u"), saved);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn user_dir_is_not_traversable() {
        // 账号名里的 ../ 不能把基线写到 configs 之外
        let evil = "../../etc";
        let name = safe_user(evil);
        assert!(!name.contains('/'), "目录名 {name} 不该含分隔符");
        assert!(!name.contains(".."), "目录名 {name} 不该含 ..");
        // 归一化后同形的两个账号仍分到不同目录
        assert_ne!(safe_user("a.b"), safe_user("a_b"));
    }
}
