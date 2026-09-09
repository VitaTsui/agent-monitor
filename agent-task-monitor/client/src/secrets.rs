//! 设备令牌的安全存储：优先系统凭证设施，文件仅作回退。
//! - macOS：钥匙串（security CLI，随用户登录解锁）
//! - Windows：DPAPI 按用户加密后落文件（其他用户/拷走文件都解不开）
//! - 其它/失败回退：明文文件 + unix 0600
//!
//! **按机器隔离**：Windows/回退路径的令牌落在 `data_dir` 里，天然跟着实例走；
//! 而 macOS 钥匙串是**整个登录用户共用**的，旧版把账号名写死成 `device-token`，
//! 同一台 Mac 上并存的第二个实例（联调时换 `AM_DATA_DIR`/`AM_MACHINE_ID`）
//! 会读到已安装客户端的那张令牌，拿别人的身份去认自己的 machine_id，
//! hub 的 verify_device_token 必然不过。所以账号名带上 machine_id。
//!
//! 迁移：
//! - 旧版明文 `data_dir/device-token`：在自己的数据目录里，归属明确，读到即导入并删除。
//! - 旧版钥匙串条目 `AgentMonitor/device-token`：**本地无从判断它属于哪个 machine_id**
//!   （令牌是 hub 侧的 32 位随机串，不含机器信息；钥匙串条目也没记）。所以读时只作
//!   「暂用」（`provisional`）返回，等 hub 认了这张令牌（上报成功 = hub 确认它绑的就是
//!   本机 machine_id）再写进本机专属键并删掉旧条目 —— 见 `AppState::adopt_device_token`。
//!   认不过就原样留着不动，绝不删：那多半是别的实例的令牌，删了会让已安装客户端掉线。

use crate::state::DeviceToken;

#[cfg(target_os = "macos")]
const SERVICE: &str = "AgentMonitor";
/// 旧版（不区分机器）的钥匙串账号名。只读不写，见文件头注释。
#[cfg(target_os = "macos")]
const SHARED_LEGACY_ACCOUNT: &str = "device-token";

/// 本机专属的钥匙串账号名。
#[cfg(any(target_os = "macos", test))]
fn account(machine_id: &str) -> String {
    format!("device-token:{machine_id}")
}

fn legacy_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("device-token")
}

#[cfg(windows)]
fn dpapi_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("device-token.dpapi")
}

/// 读取本机设备令牌。返回的 `provisional` 为真时表示这张令牌来自旧版共用条目，
/// 尚未确认归属本机（调用方按 hub 的回答决定采纳还是丢弃）。
pub fn load(data_dir: &std::path::Path, machine_id: &str) -> Option<DeviceToken> {
    // 本机专属的安全存储
    if let Some(t) = load_secure(data_dir, machine_id) {
        return Some(DeviceToken::confirmed(t));
    }
    // 旧版明文文件：就在本实例的数据目录里，归属明确 —— 导入安全存储后删除
    let legacy = legacy_path(data_dir);
    if let Ok(t) = std::fs::read_to_string(&legacy) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            if save_secure(data_dir, machine_id, &t) {
                let _ = std::fs::remove_file(&legacy);
                tracing::info!("设备令牌已迁移到系统安全存储");
            }
            return Some(DeviceToken::confirmed(t));
        }
    }
    // 旧版共用钥匙串条目：归属未知，只作暂用
    load_shared_legacy().map(DeviceToken::provisional)
}

/// 写入本机设备令牌。**全局唯一的写入点**：只写本机专属键，不写任何旧键。
pub fn save(data_dir: &std::path::Path, machine_id: &str, token: &str) {
    if save_secure(data_dir, machine_id, token) {
        let _ = std::fs::remove_file(legacy_path(data_dir));
        return;
    }
    // 回退明文文件（unix 收紧权限）
    let path = legacy_path(data_dir);
    let _ = std::fs::write(&path, token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// hub 已确认这张暂用令牌绑的就是本机：写进本机专属键，并删掉旧版共用条目。
pub fn adopt_shared_legacy(data_dir: &std::path::Path, machine_id: &str, token: &str) {
    save(data_dir, machine_id, token);
    clear_shared_legacy();
}

/// 清除本机令牌。只动本机专属键与本实例数据目录里的回退文件；
/// 旧版共用条目不在这里删 —— 它可能属于另一个实例（删了会让人家掉线）。
pub fn clear(data_dir: &std::path::Path, machine_id: &str) {
    clear_secure(data_dir, machine_id);
    let _ = std::fs::remove_file(legacy_path(data_dir));
}

// ---------- macOS：钥匙串 ----------

// GUI（Finder/自更新）启动的 app PATH 可能被裁到不含常规目录，`security` 找不到会让
// 钥匙串读写清一律静默失败 → 令牌读不出/删不掉。一律用绝对路径。
#[cfg(target_os = "macos")]
const SECURITY_BIN: &str = "/usr/bin/security";

#[cfg(target_os = "macos")]
fn kc_load(service: &str, acct: &str) -> Option<String> {
    let out = std::process::Command::new(SECURITY_BIN)
        .args(["find-generic-password", "-s", service, "-a", acct, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(target_os = "macos")]
fn kc_save(service: &str, acct: &str, token: &str) -> bool {
    std::process::Command::new(SECURITY_BIN)
        .args([
            "add-generic-password",
            "-U",
            "-s",
            service,
            "-a",
            acct,
            "-w",
            token,
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 删除某账号下的**全部**同名条目并返回条数。
///
/// 钥匙串里可能存在多条同名条目（历史版本重复 add / 跨签名分裂造成）；单次 delete
/// 只删一条，会漏删导致陈旧令牌残留、下次启动又被读回。循环删到 find 不到为止。
#[cfg(target_os = "macos")]
fn kc_delete_all(service: &str, acct: &str) -> usize {
    let mut removed = 0;
    for _ in 0..20 {
        let ok = std::process::Command::new(SECURITY_BIN)
            .args(["delete-generic-password", "-s", service, "-a", acct])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            removed += 1;
        } else {
            break; // 没有更多可删（或 security 不可用）
        }
    }
    removed
}

#[cfg(target_os = "macos")]
fn load_secure(_data_dir: &std::path::Path, machine_id: &str) -> Option<String> {
    kc_load(SERVICE, &account(machine_id))
}

#[cfg(target_os = "macos")]
fn save_secure(_data_dir: &std::path::Path, machine_id: &str, token: &str) -> bool {
    kc_save(SERVICE, &account(machine_id), token)
}

#[cfg(target_os = "macos")]
fn clear_secure(_data_dir: &std::path::Path, machine_id: &str) {
    let removed = kc_delete_all(SERVICE, &account(machine_id));
    tracing::info!("[secrets] 清除钥匙串设备令牌：删除 {removed} 条");
}

#[cfg(target_os = "macos")]
fn load_shared_legacy() -> Option<String> {
    let t = kc_load(SERVICE, SHARED_LEGACY_ACCOUNT)?;
    tracing::info!("[secrets] 读到旧版共用设备令牌，暂用；待 hub 确认归属后再迁移");
    Some(t)
}

#[cfg(target_os = "macos")]
fn clear_shared_legacy() {
    let removed = kc_delete_all(SERVICE, SHARED_LEGACY_ACCOUNT);
    tracing::info!("[secrets] 旧版共用设备令牌已迁至本机专属键，删除 {removed} 条旧条目");
}

// ---------- Windows：DPAPI ----------
//
// 加密后的令牌落在 data_dir 里，本就跟着实例走（第二个实例有自己的数据目录），
// 不存在 mac 钥匙串那种跨实例串味，因此文件名不必带 machine_id。

#[cfg(windows)]
fn load_secure(data_dir: &std::path::Path, _machine_id: &str) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let enc = std::fs::read(dpapi_path(data_dir)).ok()?;
    if enc.is_empty() {
        return None;
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: enc.len() as u32,
        pbData: enc.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pbData as _) };
    let t = String::from_utf8_lossy(&bytes).trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(windows)]
fn save_secure(data_dir: &std::path::Path, _machine_id: &str, token: &str) -> bool {
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let data = token.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return false;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pbData as _) };
    std::fs::write(dpapi_path(data_dir), bytes).is_ok()
}

#[cfg(windows)]
fn clear_secure(data_dir: &std::path::Path, _machine_id: &str) {
    let p = dpapi_path(data_dir);
    match std::fs::remove_file(&p) {
        Ok(_) => tracing::info!("[secrets] 已删除 device-token.dpapi"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("[secrets] 删除 device-token.dpapi 失败: {e}"),
    }
}

// ---------- 其它平台：无安全设施，直接回退文件 ----------

#[cfg(all(unix, not(target_os = "macos")))]
fn load_secure(_data_dir: &std::path::Path, _machine_id: &str) -> Option<String> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn save_secure(_data_dir: &std::path::Path, _machine_id: &str, _token: &str) -> bool {
    false
}

#[cfg(all(unix, not(target_os = "macos")))]
fn clear_secure(_data_dir: &std::path::Path, _machine_id: &str) {}

// ---------- 非 macOS：没有跨实例共用的凭证库，也就没有旧版共用条目 ----------

#[cfg(not(target_os = "macos"))]
fn load_shared_legacy() -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn clear_shared_legacy() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 键名必须按机器分开 —— 这正是「第二个实例捞到已安装客户端令牌」的根因。
    #[test]
    fn account_scoped_by_machine() {
        assert_eq!(account("mac-abc123"), "device-token:mac-abc123");
        assert_ne!(account("a"), account("b"));
        // 旧版共用条目的账号名不能和任何机器的新键撞上
        assert!(!account("x").eq("device-token"));
    }

    /// 数据目录里的旧版明文令牌：归属明确，读到即当作本机的（非暂用）并删除文件。
    #[test]
    fn legacy_plain_file_is_owned_not_provisional() {
        let dir = std::env::temp_dir().join(format!("am-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mid = format!("test-{}", uuid::Uuid::new_v4().simple());
        std::fs::write(legacy_path(&dir), "tok-from-file").unwrap();

        let got = load(&dir, &mid).expect("应读出旧版明文令牌");
        assert_eq!(got.value, "tok-from-file");
        assert!(!got.provisional, "自家数据目录里的令牌归属明确，不该是暂用");

        clear(&dir, &mid);
        assert!(load(&dir, &mid).is_none() || load(&dir, &mid).unwrap().provisional);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写入后能按同一 machine_id 读回，换一个 machine_id 读不到（回退文件路径下
    /// 由数据目录隔离，钥匙串路径下由账号名隔离）。
    #[test]
    fn save_load_clear_roundtrip() {
        let dir = std::env::temp_dir().join(format!("am-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mid = format!("test-{}", uuid::Uuid::new_v4().simple());

        save(&dir, &mid, "tok-1");
        let got = load(&dir, &mid).expect("应读回刚写入的令牌");
        assert_eq!(got.value, "tok-1");
        assert!(!got.provisional);

        clear(&dir, &mid);
        // 本机键清干净后，只可能读到旧版共用条目（暂用），绝不会再读到自己那张
        match load(&dir, &mid) {
            None => {}
            Some(t) => {
                assert!(t.provisional);
                assert_ne!(t.value, "tok-1");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 钥匙串隔离的正题：旧版共用条目对任何 machine_id 都只是「暂用」；
    /// 各机器写自己的键，互不串味；采纳后旧条目消失。
    /// 用独立的 service 名，绝不碰真实的 `AgentMonitor` 条目。
    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_isolated_per_machine() {
        let svc = format!("AgentMonitorTest-{}", uuid::Uuid::new_v4().simple());
        if !kc_save(&svc, SHARED_LEGACY_ACCOUNT, "shared-old") {
            // 无可用钥匙串（无头 CI/钥匙串锁定）：这条用例的前提不成立，直接跳过
            eprintln!("跳过：本环境 security 不可用");
            return;
        }
        assert_eq!(
            kc_load(&svc, SHARED_LEGACY_ACCOUNT).as_deref(),
            Some("shared-old")
        );

        // A 机写自己的键后，B 机读不到 A 的令牌
        assert!(kc_save(&svc, &account("mach-a"), "tok-a"));
        assert_eq!(kc_load(&svc, &account("mach-a")).as_deref(), Some("tok-a"));
        assert_eq!(kc_load(&svc, &account("mach-b")), None);

        // 采纳旧条目 = 删掉它
        assert_eq!(kc_delete_all(&svc, SHARED_LEGACY_ACCOUNT), 1);
        assert_eq!(kc_load(&svc, SHARED_LEGACY_ACCOUNT), None);

        kc_delete_all(&svc, &account("mach-a"));
    }
}
