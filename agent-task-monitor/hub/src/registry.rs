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
    /// 【已废弃】钉钉群机器人（webhook 单向推送）。群机器人与企业微信整体移除，
    /// 保留字段仅为让旧文件仍能反序列化，读进来即丢弃（故不读、也不写回）。
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    dingtalk: HashMap<String, serde_json::Value>,
    /// 【已废弃】企业微信自建应用，同上
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    wecom_apps: HashMap<String, serde_json::Value>,
    /// 钉钉企业应用（双向）：用户名 → 配置。**一个应用只服务配置它的那个账号** ——
    /// 谁配的机器人，收到的消息就归谁，不再有「一个机器人服务多个用户」那套。
    #[serde(default)]
    dingtalk_apps: HashMap<String, DingtalkApp>,
    /// 钉钉文件接收目录（按项目）：用户名 → (项目 cwd → 接收目录)。
    /// 未配置的项目默认落到 `<项目 cwd>/tmp`。
    #[serde(default)]
    dingtalk_recv_dirs: HashMap<String, HashMap<String, String>>,
    /// 钉钉 id 绑定：staffId → 账号。**只有走「管理员的全局机器人」时才需要** ——
    /// 那一个机器人服务所有人，只能靠发信人的 staffId 认出他是谁。
    /// 自己配了机器人的用户不必绑：谁配的机器人，消息就归谁。
    #[serde(default)]
    dingtalk_ids: HashMap<String, DingtalkIdBinding>,
    /// 配置同步的「配置源」：账号 → machine_id。没有条目 = 该账号没开配置同步，
    /// hub 既不收清单也不下发（默认关闭，用户必须显式指定以谁为准）。
    #[serde(default)]
    config_source: HashMap<String, String>,
    /// 微信机器人（iLink 扫码绑定）：账号 → 凭据 + 最近一次 context_token
    #[serde(default)]
    weixin_bots: HashMap<String, WeixinBot>,
}

/// 钉钉 id 绑定：一个 staffId 唯一归属一个账号；一个账号可绑多个钉钉号。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingtalkIdBinding {
    /// 归属的 agent-monitor 账号
    pub user: String,
    /// 钉钉昵称（绑定时捕获，供界面显示；为空则回退显示 staffId）
    #[serde(default)]
    pub nick: String,
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
    /// 跟这个机器人说过话的钉钉用户 staffId（收到消息时捕获），主动推送发给他。
    ///
    /// 一个应用只服务配置它的那个账号，所以这里只需记住「机器人对面是谁」即可 ——
    /// 不必再维护一张 staffId → 账号的绑定表，用户也不用去绑自己的钉钉 id。
    #[serde(default)]
    pub staff_id: String,
}

/// 微信（个人号）机器人：走腾讯官方 iLink Bot API，扫码绑定。
///
/// 与钉钉的关键差异：**发消息必须带 `context_token`**，而它来自用户发来的消息。
/// 实测这个 token 可长期复用（1.8 小时后仍可发出），所以把最近一次收到的存下来，
/// 任务完成时就能主动推送 —— 否则微信这条只能做「你问它答」。
/// 用户首次绑定后需要给 bot 发一句话来激活推送能力。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeixinBot {
    pub bot_token: String,
    /// 对面（用户本人）的 iLink id，形如 xxx@im.wechat
    #[serde(default)]
    pub ilink_user_id: String,
    #[serde(default)]
    pub ilink_bot_id: String,
    /// 最近一次收到消息时的 context_token —— 主动推送靠它
    #[serde(default)]
    pub context_token: String,
    #[serde(default)]
    pub bound_at: u64,
    /// 服务端判会话过期（-14）。不直接删绑定 —— 前端要能显示「需重新扫码」，
    /// 悄悄消失只会让人以为自己没配过。
    #[serde(default)]
    pub session_expired: bool,
}

pub struct Registry {
    dir: PathBuf,
    users: Vec<User>,
    devices: HashMap<String, DeviceMeta>,
    super_user: String,
    /// 用户名 → 他自己的钉钉应用。谁配的机器人，它收到的消息就归谁。
    /// super_user 名下那条同时充当「管理员的全局机器人」，供没配机器人的用户共用。
    dingtalk_apps: HashMap<String, DingtalkApp>,
    dingtalk_recv_dirs: HashMap<String, HashMap<String, String>>,
    /// staffId → 账号。只有走全局机器人时才需要（那一个机器人服务所有人）。
    dingtalk_ids: HashMap<String, DingtalkIdBinding>,
    /// 配置同步的源设备：账号 → machine_id。空 = 该账号未开启配置同步。
    config_source: HashMap<String, String>,
    /// 微信机器人：账号 → 扫码绑定的 iLink bot
    weixin_bots: HashMap<String, WeixinBot>,
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
            let dingtalk_apps = p.dingtalk_apps;
            Registry { dir, users: p.users, devices: p.devices, super_user, dingtalk_apps, dingtalk_recv_dirs: p.dingtalk_recv_dirs, dingtalk_ids: p.dingtalk_ids, config_source: p.config_source, weixin_bots: p.weixin_bots }
        } else {
            Registry {
                dir,
                users: Vec::new(),
                devices: HashMap::new(),
                super_user: seed_user.to_string(),
                dingtalk_apps: HashMap::new(),
                dingtalk_recv_dirs: HashMap::new(),
                dingtalk_ids: HashMap::new(),
                config_source: HashMap::new(),
                weixin_bots: HashMap::new(),
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
            dingtalk_apps: self.dingtalk_apps.clone(),
            dingtalk_recv_dirs: self.dingtalk_recv_dirs.clone(),
            dingtalk_ids: self.dingtalk_ids.clone(),
            config_source: self.config_source.clone(),
            weixin_bots: self.weixin_bots.clone(),
            // 已废弃字段（群机器人 / 企业微信）：写出时一律为空，
            // Persisted 上标了 skip_serializing，这里给默认值只为满足结构体字面量
            dingtalk: HashMap::new(),
            wecom_apps: HashMap::new(),
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

    // ---------- 用户自助集成（钉钉企业应用，双向） ----------


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
    /// 收到消息时记下「机器人自身编码」和「对面是谁」——主动推送两样都要用。
    ///
    /// 两者都按最新的存：robotCode 可能变；staffId 则跟着最后一个跟机器人说话的人走，
    /// 换个钉钉号来聊，推送自然跟过去（一个账号一个机器人，不存在争抢）。
    /// 仅在有变化时落盘，避免每条消息都写一次注册表。
    pub fn capture_dingtalk_peer(&mut self, app_user: &str, robot_code: &str, staff_id: &str) -> bool {
        let Some(app) = self.dingtalk_apps.get_mut(app_user) else { return false };
        let mut changed = false;
        if !robot_code.is_empty() && app.robot_code != robot_code {
            app.robot_code = robot_code.to_string();
            changed = true;
        }
        if !staff_id.is_empty() && app.staff_id != staff_id {
            app.staff_id = staff_id.to_string();
            changed = true;
        }
        if changed {
            self.save();
        }
        changed
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

    // ---------- 全局机器人（管理员配置，供没配机器人的用户共用） ----------

    /// 管理员的全局钉钉机器人 —— 就是 super_user 名下那个应用。
    /// 不额外开一份配置：超管本来就用它，再复制一份只会两处不同步。
    pub fn global_dingtalk_app(&self) -> Option<DingtalkApp> {
        self.dingtalk_apps.get(&self.super_user).cloned()
    }

    /// 后管保存全局机器人（存到 super_user 名下）
    pub fn set_global_dingtalk_app(&mut self, app_secret: &str, app_key: &str) -> Option<String> {
        let su = self.super_user.clone();
        self.set_dingtalk_app(&su, app_secret, app_key)
    }

    /// 这个应用是不是「全局机器人」（即超管配的那个）。
    /// 全局机器人服务所有人，必须靠 staffId 认人；个人机器人则谁配的归谁。
    pub fn is_global_dingtalk_app(&self, app_user: &str) -> bool {
        app_user == self.super_user
    }

    // ---------- 钉钉 id 绑定（仅全局机器人需要） ----------

    /// 按 staffId 找归属账号（全局机器人收到消息时用；找不到 → 引导绑定）
    pub fn dingtalk_user_of(&self, staff_id: &str) -> Option<String> {
        self.dingtalk_ids.get(staff_id).map(|b| b.user.clone())
    }

    /// 某账号绑定的钉钉号（staff_id, nick）；nick 空则界面回退显示 staff_id
    pub fn dingtalk_ids_of(&self, user: &str) -> Vec<(String, String)> {
        self.dingtalk_ids
            .iter()
            .filter(|(_, b)| b.user == user)
            .map(|(sid, b)| (sid.clone(), b.nick.clone()))
            .collect()
    }

    /// 绑定一个钉钉号到账号。一个 staffId 只能归一个账号，重复绑定即改绑。
    pub fn bind_dingtalk_id(&mut self, staff_id: &str, user: &str, nick: &str) {
        self.dingtalk_ids.insert(
            staff_id.to_string(),
            DingtalkIdBinding { user: user.to_string(), nick: nick.to_string() },
        );
        self.save();
    }

    /// 已绑定的钉钉号总数（后管用：看全局机器人到底有没有人在用）
    pub fn dingtalk_bound_count(&self) -> usize {
        self.dingtalk_ids.len()
    }

    /// 解绑（返回原本是否绑过）
    pub fn unbind_dingtalk_id(&mut self, staff_id: &str) -> bool {
        let existed = self.dingtalk_ids.remove(staff_id).is_some();
        if existed {
            self.save();
        }
        existed
    }

    /// 给某账号推送时该用哪个机器人、发给哪个钉钉号。
    ///
    /// **自己的机器人优先**：配了就用自己的（收件人 = 跟它说过话的人）；没配才回退到
    /// 管理员的全局机器人（收件人 = 他绑定的钉钉号）。两者都没有就推不了。
    pub fn dingtalk_push_target(&self, owner: &str) -> Option<(DingtalkApp, String)> {
        // 自己名下有应用、且已知道对面是谁 → 直接用。
        //
        // **超管也走这条**：他名下那个应用兼作全局机器人，但对他自己而言仍是私人机器人。
        // 早先把超管排除在外，单用户部署（唯一的用户就是超管）就没人能收到推送了 ——
        // 他的机器人被当成「服务所有人的公共机器人」，反倒要求他先去绑自己的钉钉号。
        if let Some(app) = self.dingtalk_apps.get(owner) {
            if !app.staff_id.is_empty() && !app.app_key.is_empty() {
                return Some((app.clone(), app.staff_id.clone()));
            }
        }
        // 全局机器人 + 该账号绑定的钉钉号（绑了多个就取其一：同一个人的不同钉钉号，
        // 推给哪个都算送到；全推反而会在多设备上重复响）
        let global = self.global_dingtalk_app()?;
        if global.app_key.is_empty() {
            return None;
        }
        let staff = self.dingtalk_ids.iter().find(|(_, b)| b.user == owner).map(|(s, _)| s.clone())?;
        Some((global, staff))
    }

    /// 某项目配置的钉钉文件接收目录（未配置返回 None → 调用方回落 `<cwd>/tmp`）。
    /// 按 `encode_path`（项目 key）匹配、而非裸 cwd 字符串：同一目录的不同形态
    /// （占位任务用进程 cwd vs 真实会话用 jsonl cwd、cursor/非 cursor，其分隔符/盘符/
    /// 标点常有细微差异）都能命中同一份配置——与前端分组、与配对 project_key 同规则。
    pub fn dingtalk_recv_dir(&self, username: &str, project_cwd: &str) -> Option<String> {
        let m = self.dingtalk_recv_dirs.get(username)?;
        let target = am_core::scanner::encode_path(project_cwd);
        m.iter()
            .find(|(k, _)| am_core::scanner::encode_path(k) == target)
            .map(|(_, v)| v.clone())
            .filter(|s| !s.trim().is_empty())
    }

    /// 该用户已配置的全部「项目 → 接收目录」。
    pub fn dingtalk_recv_dirs_of(&self, username: &str) -> HashMap<String, String> {
        self.dingtalk_recv_dirs.get(username).cloned().unwrap_or_default()
    }

    /// 设置某项目的接收目录；dir 为空则清除该项目的配置（回落默认 tmp）。
    /// 按 `encode_path` 归一：同一目录的不同 cwd 形态只保留一份，避免残留旧形态键
    /// 导致查目录时命中不到（与 [`dingtalk_recv_dir`](Self::dingtalk_recv_dir) 同规则）。
    pub fn set_dingtalk_recv_dir(&mut self, username: &str, project_cwd: &str, dir: &str) {
        let entry = self.dingtalk_recv_dirs.entry(username.to_string()).or_default();
        let target = am_core::scanner::encode_path(project_cwd);
        entry.retain(|k, _| am_core::scanner::encode_path(k) != target);
        if !dir.trim().is_empty() {
            entry.insert(project_cwd.to_string(), dir.trim().to_string());
        }
        if entry.is_empty() {
            self.dingtalk_recv_dirs.remove(username);
        }
        self.save();
    }

    /// 该账号的配置源设备（None = 未开启配置同步）
    pub fn config_source_of(&self, username: &str) -> Option<String> {
        self.config_source.get(username).cloned()
    }

    /// 指定配置源设备；`machine_id` 为空则关闭该账号的配置同步。
    ///
    /// 必须校验设备确实归该账号所有 —— 否则填一个别人的 machine_id 就能把对方的
    /// 配置拉进自己的基线（而基线随后会分发给自己名下的所有设备，等于隔账号窃取）。
    pub fn set_config_source(&mut self, username: &str, machine_id: &str) -> Result<(), String> {
        let id = machine_id.trim();
        if id.is_empty() {
            self.config_source.remove(username);
            self.save();
            return Ok(());
        }
        if !self.owned_by(id, username) {
            return Err("该设备不属于当前账号".into());
        }
        self.config_source.insert(username.to_string(), id.to_string());
        self.save();
        Ok(())
    }

    /// 设备被删除/换绑时清掉指向它的配置源，免得留下一个永远同步不动的悬空来源
    pub fn clear_config_source_of_device(&mut self, machine_id: &str) {
        self.config_source.retain(|_, v| v != machine_id);
    }

    /// 该账号绑定的微信机器人
    pub fn weixin_bot_of(&self, username: &str) -> Option<WeixinBot> {
        self.weixin_bots.get(username).cloned()
    }

    /// 扫码绑定完成时写入（context_token 留空，等用户发第一句话才有）
    pub fn set_weixin_bot(&mut self, username: &str, bot: WeixinBot) {
        self.weixin_bots.insert(username.to_string(), bot);
        self.save();
    }

    /// 刷新最近一次 context_token（收到用户消息时）。
    ///
    /// 只在**真的变了**时落盘：长轮询每收到一条消息都会调这里，
    /// 每次都 save 会把注册表写穿（那可是几十个账号 + 设备的全量 JSON）。
    pub fn touch_weixin_context(&mut self, username: &str, context_token: &str) {
        let Some(b) = self.weixin_bots.get_mut(username) else { return };
        if b.context_token == context_token {
            return;
        }
        b.context_token = context_token.to_string();
        self.save();
    }

    /// 标记会话过期与否（长轮询拿到 -14 时置位，重新收到消息时清掉）。
    /// 同样只在状态**真的翻转**时落盘。
    pub fn set_weixin_expired(&mut self, username: &str, expired: bool) {
        let Some(b) = self.weixin_bots.get_mut(username) else { return };
        if b.session_expired == expired {
            return;
        }
        b.session_expired = expired;
        self.save();
    }

    /// 解绑（用户主动解除，或 token 失效需要重扫）
    pub fn clear_weixin_bot(&mut self, username: &str) -> bool {
        let removed = self.weixin_bots.remove(username).is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// 所有已绑微信的账号（长轮询循环启动时用）
    pub fn weixin_users(&self) -> Vec<(String, WeixinBot)> {
        self.weixin_bots.iter().map(|(u, b)| (u.clone(), b.clone())).collect()
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
            // 它可能正是某账号的配置源：留着就是一个永远不再上报的来源，
            // 其余设备会一直显示「同步中」却等不到任何东西。
            self.clear_config_source_of_device(machine_id);
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

#[cfg(test)]
mod dingtalk_routing_tests {
    use super::*;

    fn reg(tag: &str) -> Registry {
        let dir = std::env::temp_dir().join(format!("am-ding-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::load(dir, "admin", "admin123")
    }

    /// 自己的机器人优先：配了就用自己的，不看有没有绑钉钉号、也不管全局机器人在不在
    #[test]
    fn personal_bot_wins_over_global() {
        let mut r = reg("personal");
        r.register("alice", "pw123456", "alice").unwrap();
        // 管理员的全局机器人
        r.set_global_dingtalk_app("gsecret", "gkey");
        // alice 自己的机器人，并已跟她聊过（捕获到收件人）
        r.set_dingtalk_app("alice", "asecret", "akey");
        r.capture_dingtalk_peer("alice", "arobot", "alice_staff");

        let (app, staff) = r.dingtalk_push_target("alice").expect("该有推送目标");
        assert_eq!(app.app_key, "akey", "配了自己的机器人就该用自己的");
        assert_eq!(staff, "alice_staff");
    }

    /// 没配自己的机器人 → 回退全局机器人，收件人取他绑定的钉钉号
    #[test]
    fn falls_back_to_global_with_bound_id() {
        let mut r = reg("global");
        r.register("bob", "pw123456", "bob").unwrap();
        r.set_global_dingtalk_app("gsecret", "gkey");
        assert!(r.dingtalk_push_target("bob").is_none(), "没绑钉钉号时认不出该发给谁");

        r.bind_dingtalk_id("bob_staff", "bob", "Bob");
        let (app, staff) = r.dingtalk_push_target("bob").expect("绑了就该能推");
        assert_eq!(app.app_key, "gkey", "没配个人机器人时走全局");
        assert_eq!(staff, "bob_staff");
    }

    /// 全局机器人靠 staffId 认人；个人机器人不需要绑定
    #[test]
    fn staff_id_only_matters_for_global_bot() {
        let mut r = reg("认人");
        r.register("carol", "pw123456", "carol").unwrap();
        r.bind_dingtalk_id("carol_staff", "carol", "Carol");

        assert_eq!(r.dingtalk_user_of("carol_staff").as_deref(), Some("carol"));
        assert_eq!(r.dingtalk_user_of("陌生人").as_deref(), None, "没绑过的认不出来");
        // 全局机器人 = 超管名下那个
        assert!(r.is_global_dingtalk_app("admin"));
        assert!(!r.is_global_dingtalk_app("carol"), "普通用户的应用不是全局的");
    }

    /// 超管名下那个机器人**同时**是他自己的私人机器人。
    /// 曾经把「全局」和「私人」互斥处理，单用户部署（唯一的用户就是超管）
    /// 就整个收不到推送了：他的机器人被判为公共的，反过来要求他先绑自己的钉钉号。
    #[test]
    fn super_user_can_use_own_global_bot() {
        let mut r = reg("超管自用");
        r.set_global_dingtalk_app("gsecret", "gkey");
        r.capture_dingtalk_peer("admin", "grobot", "admin_staff");
        assert!(r.dingtalk_ids_of("admin").is_empty(), "前提：他没给自己绑过钉钉号");

        let (app, staff) = r.dingtalk_push_target("admin").expect("超管自己也该收得到");
        assert_eq!(app.app_key, "gkey");
        assert_eq!(staff, "admin_staff");
    }

    /// 解绑只影响那一个钉钉号；一个账号可绑多个
    #[test]
    fn unbind_removes_only_that_id() {
        let mut r = reg("解绑");
        r.register("dave", "pw123456", "dave").unwrap();
        r.bind_dingtalk_id("s1", "dave", "手机");
        r.bind_dingtalk_id("s2", "dave", "电脑");
        assert_eq!(r.dingtalk_ids_of("dave").len(), 2);

        assert!(r.unbind_dingtalk_id("s1"));
        assert!(!r.unbind_dingtalk_id("s1"), "重复解绑应返回 false");
        let left = r.dingtalk_ids_of("dave");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].0, "s2");
    }
}
