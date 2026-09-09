//! 设备令牌的安全存储：优先系统凭证设施，文件仅作回退。
//! - macOS：钥匙串（security CLI，随用户登录解锁）
//! - Windows：DPAPI 按用户加密后落文件（其他用户/拷走文件都解不开）
//! - 其它/失败回退：明文文件 + unix 0600
//!
//! 迁移：旧版明文 device-token 首次读取时自动导入安全存储并删除。

#[cfg(target_os = "macos")]
const SERVICE: &str = "AgentMonitor";
#[cfg(target_os = "macos")]
const ACCOUNT: &str = "device-token";

fn legacy_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("device-token")
}

#[cfg(windows)]
fn dpapi_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("device-token.dpapi")
}

pub fn load(data_dir: &std::path::Path) -> Option<String> {
    // 先读安全存储
    if let Some(t) = load_secure(data_dir) {
        return Some(t);
    }
    // 旧版明文文件：导入安全存储后删除
    let legacy = legacy_path(data_dir);
    if let Ok(t) = std::fs::read_to_string(&legacy) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            if save_secure(data_dir, &t) {
                let _ = std::fs::remove_file(&legacy);
                tracing::info!("设备令牌已迁移到系统安全存储");
            }
            return Some(t);
        }
    }
    None
}

pub fn save(data_dir: &std::path::Path, token: &str) {
    if save_secure(data_dir, token) {
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

pub fn clear(data_dir: &std::path::Path) {
    clear_secure(data_dir);
    let _ = std::fs::remove_file(legacy_path(data_dir));
}

// ---------- macOS：钥匙串 ----------

// GUI（Finder/自更新）启动的 app PATH 可能被裁到不含常规目录，`security` 找不到会让
// 钥匙串读写清一律静默失败 → 令牌读不出/删不掉。一律用绝对路径。
#[cfg(target_os = "macos")]
const SECURITY_BIN: &str = "/usr/bin/security";

#[cfg(target_os = "macos")]
fn load_secure(_data_dir: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new(SECURITY_BIN)
        .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(target_os = "macos")]
fn save_secure(_data_dir: &std::path::Path, token: &str) -> bool {
    std::process::Command::new(SECURITY_BIN)
        .args([
            "add-generic-password",
            "-U",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-w",
            token,
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn clear_secure(_data_dir: &std::path::Path) {
    // 钥匙串里可能存在**多条同名条目**（历史版本重复 add / 跨签名分裂造成）；单次 delete
    // 只删一条，会漏删导致陈旧令牌残留、下次启动又被读回。循环删到 find 不到为止。
    let mut removed = 0;
    for _ in 0..20 {
        let ok = std::process::Command::new(SECURITY_BIN)
            .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            removed += 1;
        } else {
            break; // 没有更多可删（或 security 不可用）
        }
    }
    tracing::info!("[secrets] 清除钥匙串设备令牌：删除 {removed} 条");
}

// ---------- Windows：DPAPI ----------

#[cfg(windows)]
fn load_secure(data_dir: &std::path::Path) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let enc = std::fs::read(dpapi_path(data_dir)).ok()?;
    if enc.is_empty() {
        return None;
    }
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: enc.len() as u32,
        pbData: enc.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
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
fn save_secure(data_dir: &std::path::Path, token: &str) -> bool {
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let data = token.as_bytes();
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &mut input,
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
fn clear_secure(data_dir: &std::path::Path) {
    let p = dpapi_path(data_dir);
    match std::fs::remove_file(&p) {
        Ok(_) => tracing::info!("[secrets] 已删除 device-token.dpapi"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("[secrets] 删除 device-token.dpapi 失败: {e}"),
    }
}

// ---------- 其它平台：无安全设施，直接回退文件 ----------

#[cfg(all(unix, not(target_os = "macos")))]
fn load_secure(_data_dir: &std::path::Path) -> Option<String> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn save_secure(_data_dir: &std::path::Path, _token: &str) -> bool {
    false
}

#[cfg(all(unix, not(target_os = "macos")))]
fn clear_secure(_data_dir: &std::path::Path) {}
