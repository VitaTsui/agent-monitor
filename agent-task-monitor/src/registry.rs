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
}

/// 每台设备的元数据（key = machine_id）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceMeta {
    /// 归属用户名（认领者）
    pub owner: Option<String>,
    /// 是否信任（未信任的设备不允许监控其会话）
    pub trusted: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Persisted {
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    /// 全局 5h token 上限（0 = 不限制），达到后自动暂停对应终端任务
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
    quota_limit: u64,
    super_user: String,
}

impl Registry {
    /// 从数据目录加载；不存在则用默认（seed 超级管理员）初始化并落盘
    pub fn load(dir: PathBuf, seed_user: &str, seed_pass: &str) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("registry.json");
        let mut reg = if let Ok(txt) = std::fs::read_to_string(&path) {
            let p: Persisted = serde_json::from_str(&txt).unwrap_or_default();
            let super_user = if p.super_user.is_empty() {
                SUPER_USER.to_string()
            } else {
                p.super_user
            };
            Registry { dir, users: p.users, devices: p.devices, quota_limit: p.quota_limit, super_user }
        } else {
            Registry {
                dir,
                users: Vec::new(),
                devices: HashMap::new(),
                quota_limit: 0,
                super_user: seed_user.to_string(),
            }
        };
        if reg.users.is_empty() {
            reg.users.push(User {
                id: "1".into(),
                username: seed_user.to_string(),
                password: hash_password(seed_pass, &random_salt()),
                display: "超级管理员".into(),
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
            quota_limit: self.quota_limit,
            super_user: self.super_user.clone(),
        };
        if let Ok(txt) = serde_json::to_string_pretty(&p) {
            let _ = std::fs::write(self.dir.join("registry.json"), txt);
        }
    }

    pub fn quota_limit(&self) -> u64 {
        self.quota_limit
    }

    pub fn set_quota_limit(&mut self, limit: u64) {
        self.quota_limit = limit;
        self.save();
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

    pub fn user(&self, username: &str) -> Option<User> {
        self.users.iter().find(|u| u.username == username).cloned()
    }

    /// 全部用户（后管用户管理用）
    pub fn list_users(&self) -> Vec<User> {
        self.users.clone()
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

    /// 兼容旧调用：静态判断仅用于无 Registry 上下文处（默认超管名）
    pub fn is_super(username: &str) -> bool {
        username == SUPER_USER
    }

    pub fn device_meta(&self, machine_id: &str) -> DeviceMeta {
        self.devices.get(machine_id).cloned().unwrap_or_default()
    }

    /// 首次见到设备时登记；已存在则仅在其尚无 owner 时补认领者
    pub fn ensure_device(&mut self, machine_id: &str, claim_owner: Option<&str>, auto_trust: bool) {
        let entry = self.devices.entry(machine_id.to_string()).or_insert_with(|| DeviceMeta {
            owner: claim_owner.map(str::to_string),
            trusted: auto_trust,
        });
        if entry.owner.is_none() {
            if let Some(o) = claim_owner {
                entry.owner = Some(o.to_string());
            }
        }
        if auto_trust {
            entry.trusted = true;
        }
        self.save();
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

    pub fn set_owner(&mut self, machine_id: &str, owner: &str) -> bool {
        if let Some(d) = self.devices.get_mut(machine_id) {
            d.owner = Some(owner.to_string());
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
