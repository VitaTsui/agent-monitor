//! 用户与设备信任注册表，持久化到 ~/.agent-monitor/。
//! - 用户：登录账号（口令加盐哈希存储，历史明文自动迁移）。
//! - 设备：每台上报的机器一条元数据（归属用户 + 是否信任）。
//!   非信任设备的会话不对外暴露；用户只能看到自己名下且已信任的设备。
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

pub const SUPER_USER: &str = "admin";

/// 是否邮箱形态（用于隔离「口令账号」与「第三方账号」的命名空间）
pub fn looks_like_email(s: &str) -> bool {
    match s.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

/// 口令加盐哈希（迭代 SHA-256），存储格式 `sha256$<salt>$<hex>`
fn hash_password(password: &str, salt: &str) -> String {
    let mut acc = format!("{salt}{password}").into_bytes();
    for _ in 0..10_000 {
        let mut hasher = Sha256::new();
        hasher.update(&acc);
        acc = hasher.finalize().to_vec();
    }
    let hex: String = acc.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256${salt}${hex}")
}

/// 32 位随机令牌（设备上报令牌用）
fn random_token32() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn random_salt() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

/// 校验口令：支持哈希存储与（迁移前的）明文存储
fn verify_password(stored: &str, password: &str) -> bool {
    if let Some(rest) = stored.strip_prefix("sha256$") {
        match rest.split_once('$') {
            Some((salt, _)) => hash_password(password, salt) == stored,
            None => false,
        }
    } else {
        stored == password
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub display: String,
    /// 第三方账号来源（google/apple）。为空表示本地口令账号。
    /// 用于隔离两类账号：口令账号不可被 OAuth 顶掉，反之亦然。
    #[serde(default)]
    pub oauth_provider: Option<String>,
    /// 该用户的 5h token 额度上限（0 = 不限制）。用户自行设置，
    /// 随上报响应下发到其名下设备，由客户端本地执行自动暂停/恢复。
    #[serde(default)]
    pub quota_limit: u64,
}

/// 每台设备的元数据（key = machine_id）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceMeta {
    /// 归属用户名（认领者）
    pub owner: Option<String>,
    /// 是否信任（未信任的设备不允许监控其会话）
    pub trusted: bool,
    /// 每设备上报令牌：配对绑定时签发，客户端持有后凭它上报。
    /// 有它就不需要管理员发放全局令牌 —— 注册 + 安装即可用。
    #[serde(default)]
    pub device_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Persisted {
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    /// 旧版全局 5h token 上限；已迁移为每用户 User.quota_limit，
    /// 字段保留仅为兼容读取旧文件（load 时一次性搬到各用户）。
    #[serde(default)]
    quota_limit: u64,
    /// 超级管理员用户名（首次启动由 AM_USERNAME 种子决定）
    #[serde(default)]
    super_user: String,
}

pub struct Registry {
    dir: PathBuf,
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    /// 旧全局额度，仅作迁移中转（load 后恒为 0）
    legacy_quota: u64,
    super_user: String,
}

impl Registry {
    /// 从数据目录加载；不存在则用默认（seed 超级管理员）初始化并落盘
    pub fn load(dir: PathBuf, seed_user: &str, seed_pass: &str) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("registry.json");
        let mut reg = if let Ok(txt) = std::fs::read_to_string(&path) {
            // 解析失败绝不能 unwrap_or_default()：那会把注册表当成空的，
            // 紧接着 seed 逻辑又会 save() 覆盖原文件——一次解析失败＝所有用户永久丢失。
            // 这里选择带着原文件退出，让人工介入。
            let p: Persisted = match serde_json::from_str(&txt) {
                Ok(p) => p,
                Err(e) => {
                    let backup = dir.join("registry.json.corrupt");
                    let _ = std::fs::copy(&path, &backup);
                    panic!(
                        "注册表 {} 解析失败: {e}\n已备份为 {}。\n\
                         为避免覆盖丢失全部用户，服务拒绝启动。请修复或删除该文件后重启。",
                        path.display(),
                        backup.display()
                    );
                }
            };
            let super_user = if p.super_user.is_empty() {
                SUPER_USER.to_string()
            } else {
                p.super_user
            };
            Registry { dir, users: p.users, devices: p.devices, legacy_quota: p.quota_limit, super_user }
        } else {
            Registry {
                dir,
                users: Vec::new(),
                devices: HashMap::new(),
                legacy_quota: 0,
                super_user: seed_user.to_string(),
            }
        };
        // 迁移：旧版全局额度一次性搬到每个尚未自设额度的用户名下
        if reg.legacy_quota > 0 {
            let legacy = reg.legacy_quota;
            for u in reg.users.iter_mut() {
                if u.quota_limit == 0 {
                    u.quota_limit = legacy;
                }
            }
            reg.legacy_quota = 0;
            reg.save();
        }
        if reg.users.is_empty() {
            reg.users.push(User {
                id: "1".into(),
                username: seed_user.to_string(),
                password: hash_password(seed_pass, &random_salt()),
                display: "超级管理员".into(),
                oauth_provider: None,
                quota_limit: 0,
            });
            reg.save();
        }
        // 迁移：明文口令一律转加盐哈希
        let mut migrated = false;
        for u in reg.users.iter_mut() {
            if !u.password.starts_with("sha256$") {
                u.password = hash_password(&u.password.clone(), &random_salt());
                migrated = true;
            }
        }
        if migrated {
            reg.save();
        }
        reg
    }

    fn save(&self) {
        let p = Persisted {
            users: self.users.clone(),
            devices: self.devices.clone(),
            quota_limit: self.legacy_quota,
            super_user: self.super_user.clone(),
        };
        let Ok(txt) = serde_json::to_string_pretty(&p) else {
            tracing::error!("注册表序列化失败，本次未落盘");
            return;
        };
        // 原子写：先写同目录临时文件再 rename。
        // 直接 write 会就地截断，进程在写一半时挂掉/磁盘写满，
        // 留下的就是半截 JSON —— 下次启动解析失败。
        let path = self.dir.join("registry.json");
        let tmp = self.dir.join("registry.json.tmp");
        if let Err(e) = std::fs::write(&tmp, &txt) {
            tracing::error!("注册表写入临时文件失败: {e}");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // 内含口令哈希，仅属主可读
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::error!("注册表落盘失败: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// 该用户设置的 5h token 上限（0 = 不限制；用户不存在也视为不限制）
    pub fn quota_limit_of(&self, username: &str) -> u64 {
        self.users
            .iter()
            .find(|u| u.username == username)
            .map(|u| u.quota_limit)
            .unwrap_or(0)
    }

    /// 用户给自己设置额度上限
    pub fn set_quota_limit_for(&mut self, username: &str, limit: u64) -> bool {
        let Some(u) = self.users.iter_mut().find(|u| u.username == username) else {
            return false;
        };
        u.quota_limit = limit;
        self.save();
        true
    }

    /// 设备归属者的额度上限（随上报响应下发给该设备；无归属 = 不限制）
    pub fn quota_limit_for_device(&self, machine_id: &str) -> u64 {
        self.devices
            .get(machine_id)
            .and_then(|d| d.owner.as_deref())
            .map(|o| self.quota_limit_of(o))
            .unwrap_or(0)
    }

    /// 注册新用户（用户名已存在则失败）。返回创建的用户。
    pub fn register(&mut self, username: &str, password: &str, display: &str) -> Result<User, String> {
        let username = username.trim();
        if username.is_empty() || password.len() < 6 {
            return Err("用户名不能为空且密码至少 6 位".into());
        }
        if username.chars().any(char::is_whitespace) {
            return Err("用户名不能包含空白字符".into());
        }
        if username.chars().count() > 64 {
            return Err("用户名过长（最多 64 字符）".into());
        }
        // 防「账号预劫持」：第三方登录以邮箱作为用户名，若允许自助注册邮箱形态的
        // 用户名，攻击者可抢注 victim@x.com，受害者用 Google/Apple 登录时会直接
        // 落进攻击者已知口令的账号里。故口令注册一律不接受邮箱形态用户名。
        if looks_like_email(username) {
            return Err("用户名不能是邮箱格式（邮箱账号请使用第三方登录）".into());
        }
        if self.users.iter().any(|u| u.username == username) {
            return Err("用户名已存在".into());
        }
        // id 取现存最大值 +1，避免删除用户后复用旧 id（前端 rowKey 依赖唯一）
        let id = (self
            .users
            .iter()
            .filter_map(|u| u.id.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1)
            .to_string();
        let user = User {
            id,
            username: username.to_string(),
            password: hash_password(password, &random_salt()),
            display: if display.is_empty() { username.to_string() } else { display.to_string() },
            oauth_provider: None,
            quota_limit: 0,
        };
        self.users.push(user.clone());
        self.save();
        Ok(user)
    }

    pub fn authenticate(&self, username: &str, password: &str) -> Option<User> {
        self.users
            .iter()
            .find(|u| u.username == username && verify_password(&u.password, password))
            .cloned()
    }

    /// 全部用户（后管用户管理用）
    pub fn list_users(&self) -> Vec<User> {
        self.users.clone()
    }

    /// 配对认领：把设备绑定到用户名下并签发每设备令牌（覆盖旧令牌）。
    /// 返回签发的令牌。设备可被重新配对（换账号），但信任状态重置。
    pub fn bind_device(&mut self, machine_id: &str, owner: &str) -> String {
        let token = random_token32();
        let entry = self.devices.entry(machine_id.to_string()).or_default();
        entry.owner = Some(owner.to_string());
        // 绑定即信任：用户是在那台电脑的客户端里亲自登录的，这是对「监控本机」
        // 最直接的授权 —— 登录完成，页面/移动端立刻能看到该设备的终端会话。
        // 设备管理里的「撤销信任」保留，用于事后关停某台设备的监控。
        entry.trusted = true;
        entry.device_token = Some(token.clone());
        self.save();
        token
    }

    /// 校验设备上报令牌（常量时间比较）
    pub fn verify_device_token(&self, machine_id: &str, token: &str) -> bool {
        self.devices
            .get(machine_id)
            .and_then(|d| d.device_token.as_deref())
            .map(|t| crate::state::token_eq(t, token))
            .unwrap_or(false)
    }

    /// 用户是否存在（agent 上报时校验 AM_USER 用）
    pub fn user_exists(&self, username: &str) -> bool {
        self.users.iter().any(|u| u.username == username)
    }

    /// 删除用户（超级管理员不可删）；其名下设备释放归属并撤销信任
    pub fn delete_user(&mut self, username: &str) -> Result<(), String> {
        if username == self.super_user {
            return Err("不能删除超级管理员".into());
        }
        let before = self.users.len();
        self.users.retain(|u| u.username != username);
        if self.users.len() == before {
            return Err("用户不存在".into());
        }
        for meta in self.devices.values_mut() {
            if meta.owner.as_deref() == Some(username) {
                meta.owner = None;
                meta.trusted = false;
            }
        }
        self.save();
        Ok(())
    }

    /// 重置密码
    pub fn reset_password(&mut self, username: &str, password: &str) -> Result<(), String> {
        if password.len() < 6 {
            return Err("密码至少 6 位".into());
        }
        match self.users.iter_mut().find(|u| u.username == username) {
            Some(u) => {
                u.password = hash_password(password, &random_salt());
                self.save();
                Ok(())
            }
            None => Err("用户不存在".into()),
        }
    }

    /// 第三方登录：按用户名（邮箱）找用户，不存在则创建（随机口令占位）。
    /// 命中的若是本地口令账号（非本渠道创建），拒绝——防账号预劫持。
    pub fn find_or_create_oauth(
        &mut self,
        username: &str,
        display: &str,
        provider: &str,
    ) -> Result<User, String> {
        if let Some(u) = self.users.iter().find(|u| u.username == username) {
            return match u.oauth_provider.as_deref() {
                // 同渠道的既有第三方账号：正常登录
                Some(p) if p == provider => Ok(u.clone()),
                // 该邮箱是本地口令账号或别的渠道创建：不可被本渠道顶掉
                _ => Err("该邮箱已被一个已有账号占用，无法用此方式登录".into()),
            };
        }
        let id = (self
            .users
            .iter()
            .filter_map(|u| u.id.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1)
            .to_string();
        let user = User {
            id,
            username: username.to_string(),
            // 第三方账号本地无密码，放一段随机哈希占位（无法用于口令登录）
            password: hash_password(&random_salt(), &random_salt()),
            display: if display.trim().is_empty() {
                username.to_string()
            } else {
                display.trim().to_string()
            },
            oauth_provider: Some(provider.to_string()),
            quota_limit: 0,
        };
        self.users.push(user.clone());
        self.save();
        Ok(user)
    }

    /// 修改昵称
    pub fn update_display(&mut self, username: &str, display: &str) -> Result<(), String> {
        match self.users.iter_mut().find(|u| u.username == username) {
            Some(u) => {
                u.display = if display.trim().is_empty() {
                    u.username.clone()
                } else {
                    display.trim().to_string()
                };
                self.save();
                Ok(())
            }
            None => Err("用户不存在".into()),
        }
    }

    /// 某用户名下的设备数
    pub fn device_count_of(&self, username: &str) -> usize {
        self.devices
            .values()
            .filter(|d| d.owner.as_deref() == Some(username))
            .count()
    }

    pub fn is_super_user(&self, username: &str) -> bool {
        username == self.super_user
    }

    pub fn device_meta(&self, machine_id: &str) -> DeviceMeta {
        self.devices.get(machine_id).cloned().unwrap_or_default()
    }

    /// 首次见到设备时登记；已存在则仅在其尚无 owner 时补认领者
    /// default_trust 只在「设备创建」或「首次被认领」时生效 —— 设备接入默认信任，
    /// 但用户手动撤销信任后，后续上报绝不能把它又打开（撤销要有粘性，
    /// 否则信任开关形同虚设）。
    pub fn ensure_device(&mut self, machine_id: &str, claim_owner: Option<&str>, default_trust: bool) {
        // 该函数在每次 agent 上报（1.5s 一次）时都会被调用，绝大多数情况下
        // 什么都没变。只有真的改了才落盘，否则等于把整个注册表按 1.5s × 设备数
        // 的频率反复重写。
        let mut dirty = false;
        let entry = self.devices.entry(machine_id.to_string()).or_insert_with(|| {
            dirty = true;
            DeviceMeta {
                owner: claim_owner.map(str::to_string),
                trusted: default_trust,
                device_token: None,
            }
        });
        if entry.owner.is_none() {
            if let Some(o) = claim_owner {
                entry.owner = Some(o.to_string());
                // 首次认领视同新接入：按默认信任策略处理
                if default_trust && !entry.trusted {
                    entry.trusted = true;
                }
                dirty = true;
            }
        }
        if dirty {
            self.save();
        }
    }

    pub fn set_trust(&mut self, machine_id: &str, trusted: bool) -> bool {
        if let Some(d) = self.devices.get_mut(machine_id) {
            d.trusted = trusted;
            self.save();
            true
        } else {
            false
        }
    }

    pub fn delete_device(&mut self, machine_id: &str) -> bool {
        let removed = self.devices.remove(machine_id).is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// 该设备的会话是否允许被指定用户监控：已信任 且 归属本人。
    /// 严格按归属隔离——超级管理员也不能看别人的设备会话。
    pub fn can_view(&self, machine_id: &str, username: &str) -> bool {
        let m = self.device_meta(machine_id);
        m.trusted && m.owner.as_deref() == Some(username)
    }

    /// 该设备是否归属指定用户（用于设备管理列表，含未信任的 pending）
    pub fn owned_by(&self, machine_id: &str, username: &str) -> bool {
        self.device_meta(machine_id).owner.as_deref() == Some(username)
    }
}

#[cfg(test)]
mod user_exists_tests {
    use super::*;

    fn reg() -> Registry {
        let dir = std::env::temp_dir().join(format!("am-ue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::load(dir, "admin", "admin123")
    }

    /// agent 上报时用它校验 AM_USER：拼错一个字母就该被拒，
    /// 而不是登记成谁都看不见的孤儿设备
    #[test]
    fn distinguishes_existing_from_typo() {
        let r = reg();
        assert!(r.user_exists("admin"));
        assert!(!r.user_exists("admln"), "拼错的用户名不该被当成存在");
        assert!(!r.user_exists(""), "空用户名不存在");
        assert!(!r.user_exists("Admin"), "用户名区分大小写（owned_by 也是严格相等）");
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    fn reg(tag: &str) -> Registry {
        let dir = std::env::temp_dir().join(format!("am-q-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::load(dir, "admin", "admin123")
    }

    /// 额度是每用户的：自己设自己的，互不影响；未知用户 = 不限制
    #[test]
    fn per_user_quota() {
        let mut r = reg("per-user");
        r.register("alice", "pw123456", "").unwrap();
        assert!(r.set_quota_limit_for("alice", 500_000));
        assert_eq!(r.quota_limit_of("alice"), 500_000);
        assert_eq!(r.quota_limit_of("admin"), 0, "别的用户不受影响");
        assert_eq!(r.quota_limit_of("ghost"), 0, "未知用户视为不限制");
        assert!(!r.set_quota_limit_for("ghost", 1), "未知用户设置应失败");
    }

    /// 上报响应下发的是设备归属者的额度；无归属设备不限制
    #[test]
    fn device_owner_quota() {
        let mut r = reg("dev-owner");
        r.register("bob", "pw123456", "").unwrap();
        r.set_quota_limit_for("bob", 300_000);
        r.ensure_device("mac-1", Some("bob"), true);
        r.ensure_device("mac-orphan", None, false);
        assert_eq!(r.quota_limit_for_device("mac-1"), 300_000);
        assert_eq!(r.quota_limit_for_device("mac-orphan"), 0);
        assert_eq!(r.quota_limit_for_device("mac-unknown"), 0);
    }

    /// 旧版全局额度：load 时一次性搬到每个未自设额度的用户，之后不再有全局值
    #[test]
    fn migrates_legacy_global_quota() {
        let dir = std::env::temp_dir().join(format!("am-q-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut r = Registry::load(dir.clone(), "admin", "admin123");
            r.register("carol", "pw123456", "").unwrap();
            // 手工写入旧格式：全局 quota_limit > 0
            let path = dir.join("registry.json");
            let mut v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            v["quota_limit"] = serde_json::json!(800_000u64);
            std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        }
        let r = Registry::load(dir, "admin", "admin123");
        assert_eq!(r.quota_limit_of("admin"), 800_000);
        assert_eq!(r.quota_limit_of("carol"), 800_000);
    }
}

#[cfg(test)]
mod default_trust_tests {
    use super::*;

    fn reg(tag: &str) -> Registry {
        let dir = std::env::temp_dir().join(format!("am-dt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::load(dir, "admin", "admin123")
    }

    /// 新设备接入默认信任；用户撤销后，后续上报不得把信任又打开（粘性）
    #[test]
    fn default_trust_and_sticky_revoke() {
        let mut r = reg("sticky");
        r.register("dave", "pw123456", "").unwrap();
        // 首次上报登记 → 默认信任
        r.ensure_device("pc-1", Some("dave"), true);
        assert!(r.device_meta("pc-1").trusted, "新设备应默认信任");
        // 用户撤销信任（断开链接）
        assert!(r.set_trust("pc-1", false));
        // 该设备继续上报（每 1.5s 一次）—— 不得重新信任
        r.ensure_device("pc-1", Some("dave"), true);
        r.ensure_device("pc-1", None, true);
        assert!(!r.device_meta("pc-1").trusted, "撤销必须有粘性，上报不能重新打开信任");
        // 用户手动恢复信任
        assert!(r.set_trust("pc-1", true));
        assert!(r.device_meta("pc-1").trusted);
    }

    /// 无归属设备被首次认领时，同样按默认信任处理
    #[test]
    fn claim_applies_default_trust() {
        let mut r = reg("claim");
        r.register("erin", "pw123456", "").unwrap();
        // 匿名先上报（无 owner），后被认领
        r.ensure_device("pc-2", None, true);
        r.ensure_device("pc-2", Some("erin"), true);
        assert_eq!(r.device_meta("pc-2").owner.as_deref(), Some("erin"));
        assert!(r.device_meta("pc-2").trusted, "首次认领视同新接入，默认信任");
    }
}
