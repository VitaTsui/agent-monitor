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

/// 回调路由用的不透明 id（24 位十六进制，够抗枚举）
fn new_channel() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..24).map(|_| format!("{:x}", rng.gen_range(0..16))).collect()
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
    /// 以下为展示信息（随上报刷新并持久化）：设备离线或 hub 重启后，
    /// 设备管理列表仍能显示这台机器，而不是从列表里凭空消失。
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub version: String,
    /// 最后一次上报（unix 秒，粗粒度节流写入）
    #[serde(default)]
    pub last_seen: u64,
    /// 协助共享：设备主人生成的连接码 + 密码，供其他用户接入（类似远程控制）
    #[serde(default)]
    pub share: Option<ShareEntry>,
    /// 已通过协助码接入本设备的用户名（可查看+控制其会话）；主人可随时撤销
    #[serde(default)]
    pub shared_with: Vec<String>,
}

/// 协助共享条目：连接码 + 密码哈希 + 有效期
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareEntry {
    /// 连接码（其他用户输入它 + 密码即可接入）
    pub code: String,
    /// 密码哈希（加盐 SHA-256，同账号口令）
    pub password_hash: String,
    /// 临时密码（true）到期自动失效；固定密码（false）长期有效
    pub temporary: bool,
    /// 到期时间（unix 秒，仅临时密码有值；0 = 不过期）
    #[serde(default)]
    pub expires_at: u64,
}

impl ShareEntry {
    fn expired(&self) -> bool {
        self.temporary && self.expires_at > 0 && crate::state::now_secs() >= self.expires_at
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Persisted {
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    /// 历史字段（旧版全局额度）；额度功能已整体移除，仅为兼容旧文件保留反序列化位
    #[serde(default)]
    quota_limit: u64,
    /// 超级管理员用户名（首次启动由 AM_USERNAME 种子决定）
    #[serde(default)]
    super_user: String,
    /// 钉钉群机器人推送配置：用户名 → webhook + 事件开关
    #[serde(default)]
    dingtalk: HashMap<String, crate::dingtalk::DingtalkNotify>,
    /// 企业微信自建应用（双向）：用户名 → 配置
    #[serde(default)]
    wecom_apps: HashMap<String, WecomApp>,
    /// 钉钉企业应用（双向）：用户名 → 配置
    #[serde(default)]
    dingtalk_apps: HashMap<String, DingtalkApp>,
}

/// 企业微信自建应用（用户自助接入，双向遥控）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WecomApp {
    /// 回调 URL 里的不透明路由 id（用户专属）
    pub channel: String,
    pub corp_id: String,
    pub token: String,
    /// EncodingAESKey 原文（43 位）
    pub aes_key: String,
}

/// 钉钉企业应用（用户自助接入，双向遥控）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingtalkApp {
    pub channel: String,
    pub app_secret: String,
    /// Stream 模式的 clientId（钉钉应用 AppKey / ClientID）。填了它就走 Stream
    /// 长连接（hub 主动连钉钉，免公网入站回调），绕开中国→海外服务器可达性问题。
    #[serde(default)]
    pub app_key: String,
    /// 机器人 robotCode（收到消息时捕获；主动 OTO 推送用）。Stream 机器人一般 == app_key，
    /// 但以消息里带的为准。
    #[serde(default)]
    pub robot_code: String,
    /// 用户本人的 staffId（收到消息时捕获）。主动推送就发给这个人。
    #[serde(default)]
    pub staff_id: String,
}

pub struct Registry {
    dir: PathBuf,
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    super_user: String,
    dingtalk: HashMap<String, crate::dingtalk::DingtalkNotify>,
    wecom_apps: HashMap<String, WecomApp>,
    dingtalk_apps: HashMap<String, DingtalkApp>,
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
            Registry { dir, users: p.users, devices: p.devices, super_user, dingtalk: p.dingtalk, wecom_apps: p.wecom_apps, dingtalk_apps: p.dingtalk_apps }
        } else {
            Registry {
                dir,
                users: Vec::new(),
                devices: HashMap::new(),
                super_user: seed_user.to_string(),
                dingtalk: HashMap::new(),
                wecom_apps: HashMap::new(),
                dingtalk_apps: HashMap::new(),
            }
        };
        if reg.users.is_empty() {
            reg.users.push(User {
                id: "1".into(),
                username: seed_user.to_string(),
                password: hash_password(seed_pass, &random_salt()),
                display: "超级管理员".into(),
                oauth_provider: None,
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
            dingtalk: self.dingtalk.clone(),
            wecom_apps: self.wecom_apps.clone(),
            dingtalk_apps: self.dingtalk_apps.clone(),
            quota_limit: 0,
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

    /// 按用户名取用户（客户端静默续登等需要完整用户信息的场景）
    pub fn user_by_name(&self, username: &str) -> Option<&User> {
        self.users.iter().find(|u| u.username == username)
    }

    // ---------- 用户自助集成（企业微信 / 钉钉应用，双向） ----------

    /// 保存企业微信自建应用配置；corp_id 为空则删除。返回该用户的回调 channel。
    pub fn set_wecom_app(&mut self, user: &str, corp_id: &str, token: &str, aes_key: &str) -> Option<String> {
        if corp_id.trim().is_empty() {
            self.wecom_apps.remove(user);
            self.save();
            return None;
        }
        let channel = self.wecom_apps.get(user).map(|a| a.channel.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(new_channel);
        self.wecom_apps.insert(user.to_string(), WecomApp {
            channel: channel.clone(),
            corp_id: corp_id.trim().to_string(),
            token: token.trim().to_string(),
            aes_key: aes_key.trim().to_string(),
        });
        self.save();
        Some(channel)
    }

    pub fn wecom_app_of(&self, user: &str) -> Option<WecomApp> {
        self.wecom_apps.get(user).cloned()
    }

    /// 按回调 channel 反查 (用户名, 配置)
    pub fn wecom_app_by_channel(&self, channel: &str) -> Option<(String, WecomApp)> {
        self.wecom_apps.iter().find(|(_, a)| a.channel == channel).map(|(u, a)| (u.clone(), a.clone()))
    }

    /// 保存钉钉企业应用配置；app_secret 为空则删除。app_key 填了则走 Stream 长连接。
    /// 返回回调 channel（HTTP 回调模式用；Stream 模式用不到但保留兼容）。
    pub fn set_dingtalk_app(&mut self, user: &str, app_secret: &str, app_key: &str) -> Option<String> {
        if app_secret.trim().is_empty() {
            self.dingtalk_apps.remove(user);
            self.save();
            return None;
        }
        // 保留已捕获的身份（robot_code/staff_id）与 channel：重存凭据不该清掉它们
        let prev = self.dingtalk_apps.get(user).cloned().unwrap_or_default();
        let channel = if prev.channel.is_empty() { new_channel() } else { prev.channel.clone() };
        self.dingtalk_apps.insert(user.to_string(), DingtalkApp {
            channel: channel.clone(),
            app_secret: app_secret.trim().to_string(),
            app_key: app_key.trim().to_string(),
            robot_code: prev.robot_code,
            staff_id: prev.staff_id,
        });
        self.save();
        Some(channel)
    }

    /// 收到 Stream 机器人消息时，捕获发信人 staffId 与 robotCode（主动 OTO 推送要用）。
    /// staffId「首次捕获即绑定」，之后不因别的发信人自动改绑（防止他人私聊机器人把推送劫持走）；
    /// 要改绑用 `bind_dingtalk_staff` 显式覆盖。robotCode 是机器人自身编码、与发信人无关，可随时更新。
    /// 仅在有变化时落盘。返回是否发生了变更。
    pub fn set_dingtalk_identity(&mut self, user: &str, staff_id: &str, robot_code: &str) -> bool {
        let Some(app) = self.dingtalk_apps.get_mut(user) else { return false };
        let mut changed = false;
        if !staff_id.is_empty() && app.staff_id.is_empty() {
            app.staff_id = staff_id.to_string();
            changed = true;
        }
        if !robot_code.is_empty() && app.robot_code != robot_code {
            app.robot_code = robot_code.to_string();
            changed = true;
        }
        if changed {
            self.save();
        }
        changed
    }

    /// 显式（重新）绑定推送接收人：把发信人 staffId 强制绑到该账号（「绑定」指令用）。
    pub fn bind_dingtalk_staff(&mut self, user: &str, staff_id: &str, robot_code: &str) -> bool {
        let Some(app) = self.dingtalk_apps.get_mut(user) else { return false };
        if staff_id.is_empty() {
            return false;
        }
        app.staff_id = staff_id.to_string();
        if !robot_code.is_empty() {
            app.robot_code = robot_code.to_string();
        }
        self.save();
        true
    }

    /// 解绑推送接收人（「解绑」指令用）。返回原本是否有绑定。
    pub fn unbind_dingtalk_staff(&mut self, user: &str) -> bool {
        let Some(app) = self.dingtalk_apps.get_mut(user) else { return false };
        if app.staff_id.is_empty() {
            return false;
        }
        app.staff_id.clear();
        self.save();
        true
    }

    /// 所有配了 Stream（app_key+app_secret 都非空）的钉钉应用：(user, app_key, app_secret)
    pub fn dingtalk_stream_apps(&self) -> Vec<(String, String, String)> {
        self.dingtalk_apps
            .iter()
            .filter(|(_, a)| !a.app_key.is_empty() && !a.app_secret.is_empty())
            .map(|(u, a)| (u.clone(), a.app_key.clone(), a.app_secret.clone()))
            .collect()
    }

    pub fn dingtalk_app_of(&self, user: &str) -> Option<DingtalkApp> {
        self.dingtalk_apps.get(user).cloned()
    }

    pub fn dingtalk_app_by_channel(&self, channel: &str) -> Option<(String, DingtalkApp)> {
        self.dingtalk_apps.iter().find(|(_, a)| a.channel == channel).map(|(u, a)| (u.clone(), a.clone()))
    }

    // ---------- 钉钉推送配置 ----------

    pub fn set_dingtalk(&mut self, username: &str, cfg: crate::dingtalk::DingtalkNotify) {
        if cfg.webhook.trim().is_empty() {
            self.dingtalk.remove(username);
        } else {
            self.dingtalk.insert(username.to_string(), cfg);
        }
        self.save();
    }

    pub fn dingtalk_of(&self, username: &str) -> Option<crate::dingtalk::DingtalkNotify> {
        self.dingtalk.get(username).cloned()
    }

    /// 该用户名下全部设备（含离线；设备管理列表用）
    pub fn devices_of(&self, username: &str) -> Vec<(String, DeviceMeta)> {
        self.devices
            .iter()
            .filter(|(_, m)| m.owner.as_deref() == Some(username))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 随上报刷新设备展示信息。hostname/platform/version 变了才落盘；
    /// last_seen 十分钟一档节流（它只服务于离线设备的「最后在线」展示，
    /// 不能让每 1.5s 的上报把整个注册表写穿）。
    pub fn update_device_info(
        &mut self,
        machine_id: &str,
        hostname: &str,
        platform: &str,
        version: &str,
    ) {
        let now = crate::state::now_secs();
        let Some(m) = self.devices.get_mut(machine_id) else {
            return;
        };
        let changed = m.hostname != hostname || m.platform != platform || m.version != version;
        if changed {
            m.hostname = hostname.to_string();
            m.platform = platform.to_string();
            m.version = version.to_string();
        }
        let seen_stale = now.saturating_sub(m.last_seen) >= 600;
        if seen_stale {
            m.last_seen = now;
        }
        if changed || seen_stale {
            self.save();
        }
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
                ..DeviceMeta::default()
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

    /// 该设备的会话是否允许被指定用户监控：
    /// - 主人：已信任 且 归属本人；
    /// - 协助访客：通过协助码接入（主人显式共享，绕过信任判定）。
    pub fn can_view(&self, machine_id: &str, username: &str) -> bool {
        let m = self.device_meta(machine_id);
        (m.trusted && m.owner.as_deref() == Some(username))
            || m.shared_with.iter().any(|u| u == username)
    }

    /// 该设备是否归属指定用户（用于设备管理列表，含未信任的 pending）
    pub fn owned_by(&self, machine_id: &str, username: &str) -> bool {
        self.device_meta(machine_id).owner.as_deref() == Some(username)
    }

    // ---------- 协助共享（跨用户设备接入） ----------

    /// 主人为自己的设备生成/刷新协助码。temporary=true 时用生成的随机
    /// 临时密码（30 分钟过期）；否则用调用方给的固定密码。返回 (连接码, 明文密码)。
    pub fn create_share(
        &mut self,
        machine_id: &str,
        temporary: bool,
        fixed_password: Option<&str>,
    ) -> Result<(String, String), String> {
        if !self.devices.contains_key(machine_id) {
            return Err("设备不存在".into());
        }
        let password = if temporary {
            // 8 位数字临时密码，好念好输
            use rand::Rng;
            let mut rng = rand::thread_rng();
            (0..8).map(|_| char::from(b'0' + rng.gen_range(0..10))).collect::<String>()
        } else {
            let p = fixed_password.unwrap_or("").trim().to_string();
            if p.len() < 4 {
                return Err("固定密码至少 4 位".into());
            }
            p
        };
        // 连接码沿用设备已有的（同一设备连接码稳定），首次生成新的
        let code = self
            .devices
            .get(machine_id)
            .and_then(|d| d.share.as_ref())
            .map(|s| s.code.clone())
            .unwrap_or_else(new_share_code);
        let expires_at = if temporary { crate::state::now_secs() + 30 * 60 } else { 0 };
        let entry = ShareEntry {
            code: code.clone(),
            password_hash: hash_password(&password, &random_salt()),
            temporary,
            expires_at,
        };
        if let Some(d) = self.devices.get_mut(machine_id) {
            d.share = Some(entry);
        }
        self.save();
        Ok((code, password))
    }

    /// 当前协助码信息（供主人查看）：返回 (连接码, 是否临时, 到期秒)
    pub fn share_info(&self, machine_id: &str) -> Option<(String, bool, u64)> {
        self.devices
            .get(machine_id)
            .and_then(|d| d.share.as_ref())
            .filter(|s| !s.expired())
            .map(|s| (s.code.clone(), s.temporary, s.expires_at))
    }

    /// 撤销协助码：清连接码 + 踢出所有已接入访客
    pub fn revoke_share(&mut self, machine_id: &str) {
        if let Some(d) = self.devices.get_mut(machine_id) {
            d.share = None;
            d.shared_with.clear();
            self.save();
        }
    }

    /// 访客用连接码 + 密码接入。成功返回 machine_id。
    pub fn connect_share(&mut self, code: &str, password: &str, user: &str) -> Result<String, String> {
        let code = code.trim();
        let hit = self.devices.iter().find_map(|(id, d)| {
            d.share.as_ref().filter(|s| s.code == code).map(|s| (id.clone(), s.clone()))
        });
        let Some((machine_id, share)) = hit else {
            return Err("连接码无效".into());
        };
        if share.expired() {
            return Err("临时密码已过期，请向设备主人索取新密码".into());
        }
        if self.owned_by(&machine_id, user) {
            return Err("这是你自己的设备，无需接入".into());
        }
        if !verify_password(&share.password_hash, password) {
            return Err("密码错误".into());
        }
        if let Some(d) = self.devices.get_mut(&machine_id) {
            if !d.shared_with.iter().any(|u| u == user) {
                d.shared_with.push(user.to_string());
            }
        }
        self.save();
        Ok(machine_id)
    }

    /// 访客主动断开自己对某设备的接入
    pub fn disconnect_share(&mut self, machine_id: &str, user: &str) {
        if let Some(d) = self.devices.get_mut(machine_id) {
            let before = d.shared_with.len();
            d.shared_with.retain(|u| u != user);
            if d.shared_with.len() != before {
                self.save();
            }
        }
    }

    /// 主人踢掉某个访客
    pub fn kick_share_user(&mut self, machine_id: &str, user: &str) {
        self.disconnect_share(machine_id, user);
    }

    /// 某设备当前已接入的访客列表（供主人查看）
    pub fn share_guests(&self, machine_id: &str) -> Vec<String> {
        self.devices.get(machine_id).map(|d| d.shared_with.clone()).unwrap_or_default()
    }

    /// 该用户通过协助码可访问的（他人）设备
    pub fn shared_to(&self, username: &str) -> Vec<(String, DeviceMeta)> {
        self.devices
            .iter()
            .filter(|(_, m)| m.shared_with.iter().any(|u| u == username))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// 协助连接码：9 位数字，分三段好念（其他用户手输）
fn new_share_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..9).map(|_| char::from(b'0' + rng.gen_range(0..10))).collect()
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

#[cfg(test)]
mod offline_visibility_tests {
    use super::*;

    /// 设备离线（未上报）也必须出现在名下设备里，且展示信息随上报持久化
    #[test]
    fn offline_devices_stay_listed() {
        let dir = std::env::temp_dir().join(format!("am-ov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut r = Registry::load(dir.clone(), "admin", "admin123");
        r.register("hank", "pw123456", "").unwrap();
        r.ensure_device("pc-9", Some("hank"), true);
        r.update_device_info("pc-9", "Hank-PC", "windows", "0.2.0");
        // 模拟 hub 重启：重新加载注册表（实时表为空的场景）
        let r2 = Registry::load(dir, "admin", "admin123");
        let devs = r2.devices_of("hank");
        assert_eq!(devs.len(), 1);
        let (id, meta) = &devs[0];
        assert_eq!(id, "pc-9");
        assert_eq!(meta.hostname, "Hank-PC");
        assert_eq!(meta.platform, "windows");
        assert!(meta.trusted);
        assert!(meta.last_seen > 0, "last_seen 应已记录");
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;

    fn reg(tag: &str) -> Registry {
        let dir = std::env::temp_dir().join(format!("am-share-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::load(dir, "admin", "admin123")
    }

    /// 固定密码：访客用连接码+密码接入 → can_view 放行；密码错拒绝；撤销后失效
    #[test]
    fn fixed_password_flow() {
        let mut r = reg("fixed");
        r.register("alice", "pw123456", "").unwrap();
        r.register("bob", "pw123456", "").unwrap();
        r.ensure_device("pc-a", Some("alice"), true);
        let (code, pw) = r.create_share("pc-a", false, Some("secret1")).unwrap();
        assert_eq!(pw, "secret1");
        // 接入前 bob 看不到
        assert!(!r.can_view("pc-a", "bob"));
        // 密码错误
        assert!(r.connect_share(&code, "wrong", "bob").is_err());
        // 正确接入
        assert_eq!(r.connect_share(&code, "secret1", "bob").unwrap(), "pc-a");
        assert!(r.can_view("pc-a", "bob"), "接入后可查看");
        assert!(r.shared_to("bob").iter().any(|(id, _)| id == "pc-a"));
        // 主人自己不能接入自己
        assert!(r.connect_share(&code, "secret1", "alice").is_err());
        // 撤销 → 踢出
        r.revoke_share("pc-a");
        assert!(!r.can_view("pc-a", "bob"), "撤销后失效");
        assert!(r.share_info("pc-a").is_none());
    }

    /// 临时密码：过期后拒绝接入
    #[test]
    fn temp_password_expires() {
        let mut r = reg("temp");
        r.register("carol", "pw123456", "").unwrap();
        r.register("dave", "pw123456", "").unwrap();
        r.ensure_device("pc-c", Some("carol"), true);
        let (code, pw) = r.create_share("pc-c", true, None).unwrap();
        assert_eq!(pw.len(), 8);
        assert!(pw.chars().all(|c| c.is_ascii_digit()));
        // 手动把到期时间设到过去
        if let Some(d) = r.devices.get_mut("pc-c") {
            if let Some(s) = d.share.as_mut() {
                s.expires_at = 1;
            }
        }
        assert!(r.connect_share(&code, &pw, "dave").is_err(), "过期临时密码应拒绝");
    }

    /// 访客自断 & 主人踢人
    #[test]
    fn disconnect_and_kick() {
        let mut r = reg("disc");
        r.register("erin", "pw123456", "").unwrap();
        r.register("frank", "pw123456", "").unwrap();
        r.ensure_device("pc-e", Some("erin"), true);
        let (code, pw) = r.create_share("pc-e", false, Some("pass12")).unwrap();
        r.connect_share(&code, &pw, "frank").unwrap();
        assert!(r.share_guests("pc-e").contains(&"frank".to_string()));
        r.disconnect_share("pc-e", "frank");
        assert!(!r.can_view("pc-e", "frank"), "自断后失效");
        // 再接入后主人踢
        r.connect_share(&code, &pw, "frank").unwrap();
        r.kick_share_user("pc-e", "frank");
        assert!(!r.can_view("pc-e", "frank"), "被踢后失效");
    }
}
