//! Windows → WSL 采集端托管。
//!
//! Windows 进程无法读取 WSL 的 Linux 进程表、TTY 与信号。这里不把 WSL 会话文件硬塞进
//! Windows 扫描器，而是在每个真实 WSL 发行版内启动同版本的无界面 am-client：文件、进程、
//! 控制和输入都留在所属内核中处理，Windows 端只负责安装、签发运行端令牌与保活。

#![cfg(windows)]

use crate::state::SharedState;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const SUPERVISE_INTERVAL_SECS: u64 = 30;

/// 安装、更新令牌和宿主心跳，只判断采集端是否需要启动。这里故意不在 WSL 内后台启动：
/// 当启动它的 `wsl.exe` 退出且发行版没有别的前台进程时，WSL 会把后台进程一起回收。
const INSTALL_SCRIPT: &str = r#"
set -eu
src=$1
version=$2
data=${XDG_DATA_HOME:-$HOME/.local/share}/AgentMonitor
bin_dir=$data/bin
bin=$bin_dir/agent-monitor-wsl
pid_file=$data/wsl-agent.pid
version_file=$data/wsl-agent.version
parent_heartbeat=$data/windows-parent.heartbeat
mkdir -p "$bin_dir"
IFS= read -r token
printf '%s' "$token" > "$data/device-token"
chmod 600 "$data/device-token"
touch "$parent_heartbeat"

installed=''
if [ -r "$version_file" ]; then installed=$(cat "$version_file"); fi
running=0
pid=''
if [ -r "$pid_file" ]; then
  pid=$(cat "$pid_file" 2>/dev/null || true)
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    actual=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
    if [ "$actual" = "$bin" ]; then running=1; fi
  fi
fi

if [ "$installed" != "$version" ] || [ ! -x "$bin" ]; then
  if [ "$running" = 1 ]; then
    kill "$pid" 2>/dev/null || true
    i=0
    while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 20 ]; do
      sleep 0.1
      i=$((i + 1))
    done
    running=0
  fi
  tmp=$bin.tmp.$$
  cp "$src" "$tmp"
  chmod 700 "$tmp"
  mv -f "$tmp" "$bin"
  printf '%s' "$version" > "$version_file"
fi

if [ "$running" = 1 ]; then
  printf 'running\n'
else
  rm -f "$pid_file"
  printf 'start\n'
fi
"#;

/// 采集端作为 `wsl.exe` 的前台进程运行。Windows 客户端持有 Child，退出会被发现并重启。
const RUN_SCRIPT: &str = r#"
set -eu
hub=$1
machine=$2
display=$3
data=${XDG_DATA_HOME:-$HOME/.local/share}/AgentMonitor
bin=$data/bin/agent-monitor-wsl
pid_file=$data/wsl-agent.pid
parent_heartbeat=$data/windows-parent.heartbeat
mkdir -p "$data"
touch "$parent_heartbeat"
printf '%s' "$$" > "$pid_file"
exec env \
  AM_HEADLESS=1 \
  AM_NO_TRAY=1 \
  AM_HUB_URL="$hub" \
  AM_MACHINE_ID="$machine" \
  AM_DEVICE_NAME="$display" \
  AM_DATA_DIR="$data" \
  AM_PARENT_HEARTBEAT="$parent_heartbeat" \
  "$bin" >> "$data/wsl-agent.log" 2>&1 </dev/null
"#;

struct ManagedRuntime {
    child: Child,
}

impl Drop for ManagedRuntime {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn spawn_supervisor(state: SharedState, hub: String) {
    tokio::spawn(async move {
        let mut last_error: Option<String> = None;
        let mut runtimes: HashMap<String, ManagedRuntime> = HashMap::new();
        loop {
            runtimes.retain(|distro, runtime| match runtime.child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    tracing::warn!("WSL {distro} 采集运行端退出（{status}），稍后重启");
                    false
                }
                Err(e) => {
                    tracing::warn!("无法读取 WSL {distro} 采集运行端状态: {e}");
                    false
                }
            });
            match supervise_once(&state, &hub, &mut runtimes).await {
                Ok(()) => last_error = None,
                Err(e) => {
                    if last_error.as_deref() != Some(e.as_str()) {
                        tracing::warn!("WSL 采集端托管失败: {e}");
                        last_error = Some(e);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(SUPERVISE_INTERVAL_SECS)).await;
        }
    });
}

async fn supervise_once(
    state: &SharedState,
    hub: &str,
    runtimes: &mut HashMap<String, ManagedRuntime>,
) -> Result<(), String> {
    let source = helper_source()
        .ok_or_else(|| "安装目录缺少 agent-monitor-wsl；当前安装包不含 WSL 采集端".to_string())?;
    let host_token = state
        .device_token
        .read()
        .await
        .as_ref()
        .map(|d| d.value.clone())
        .ok_or_else(|| "Windows 设备尚未完成绑定".to_string())?;
    let distros = tokio::task::spawn_blocking(list_distros)
        .await
        .map_err(|e| e.to_string())??;
    if distros.is_empty() {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut errors = Vec::new();
    for distro in distros {
        if let Err(e) =
            supervise_distro(state, &client, hub, &source, &host_token, &distro, runtimes).await
        {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

async fn supervise_distro(
    state: &SharedState,
    client: &reqwest::Client,
    hub: &str,
    source: &Path,
    host_token: &str,
    distro: &str,
    runtimes: &mut HashMap<String, ManagedRuntime>,
) -> Result<(), String> {
    let runtime_id = runtime_id(&state.config.machine_id, distro);
    let display = format!("{} / WSL {distro}", state.config.hostname);
    let body = serde_json::json!({
        "hostMachineId": state.config.machine_id,
        "runtimeId": runtime_id,
        "hostname": display,
        "platform": "linux",
        "version": env!("CARGO_PKG_VERSION"),
    });
    let response = client
        .post(format!("{hub}/monitor/runtime/bind"))
        .header("x-device-token", host_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{distro}: 运行端绑定请求失败: {e}"))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("{distro}: 运行端绑定响应损坏: {e}"))?;
    if value.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
        return Err(format!(
            "{distro}: 运行端绑定被拒绝: {}",
            value
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("未知错误")
        ));
    }
    let token = value
        .pointer("/data/deviceToken")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{distro}: 运行端绑定响应缺少令牌"))?
        .to_string();
    let source = source.to_path_buf();
    let distro_for_install = distro.to_string();
    let needs_start = tokio::task::spawn_blocking(move || {
        install_and_prepare(&distro_for_install, &source, &token)
    })
    .await
    .map_err(|e| e.to_string())??;
    if needs_start {
        // 安装脚本可能刚因版本升级杀掉旧 Linux 进程；此时 map 里的 wsl.exe 句柄要到
        // 下一轮才会被 retain 发现已退出。现在就移除并重启，避免产生 30 秒离线窗口。
        runtimes.remove(distro);
        let child = start_runtime(distro, hub, &runtime_id, &display)?;
        runtimes.insert(distro.to_string(), ManagedRuntime { child });
    }
    Ok(())
}

fn helper_source() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AM_WSL_AGENT_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("agent-monitor-wsl");
    path.is_file().then_some(path)
}

fn list_distros() -> Result<Vec<String>, String> {
    let out = hidden(Command::new("wsl.exe"))
        .args(["--list", "--running", "--quiet"])
        .output()
        .map_err(|e| format!("无法执行 wsl.exe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "wsl.exe --list --running 失败: {}",
            decode_output(&out.stderr).trim()
        ));
    }
    Ok(decode_output(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !s.to_ascii_lowercase().starts_with("docker-desktop"))
        .map(str::to_string)
        .collect())
}

fn install_and_prepare(distro: &str, source: &Path, token: &str) -> Result<bool, String> {
    let source_wsl = windows_path_to_wsl(distro, source)?;
    let mut child = hidden(Command::new("wsl.exe"))
        .args([
            "--distribution",
            distro,
            "--exec",
            "sh",
            "-c",
            INSTALL_SCRIPT,
            "wsl-agent-install",
            &source_wsl,
            env!("CARGO_PKG_VERSION"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{distro}: 无法启动 WSL 安装脚本: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(format!("{token}\n").as_bytes())
            .map_err(|e| format!("{distro}: 写入安装脚本失败: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{distro}: 等待安装脚本失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{distro}: 安装脚本失败: {}",
            decode_output(&out.stderr).trim()
        ));
    }
    match decode_output(&out.stdout).trim() {
        "running" => Ok(false),
        "start" => Ok(true),
        other => Err(format!("{distro}: 安装脚本返回未知状态: {other}")),
    }
}

fn start_runtime(
    distro: &str,
    hub: &str,
    machine_id: &str,
    display: &str,
) -> Result<Child, String> {
    hidden(Command::new("wsl.exe"))
        .args([
            "--distribution",
            distro,
            "--exec",
            "sh",
            "-c",
            RUN_SCRIPT,
            "wsl-agent-run",
            hub,
            machine_id,
            display,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{distro}: 无法启动 WSL 采集运行端: {e}"))
}

fn windows_path_to_wsl(distro: &str, path: &Path) -> Result<String, String> {
    let raw = path
        .to_str()
        .ok_or_else(|| "WSL 采集端路径不是有效 Unicode".to_string())?;
    let out = hidden(Command::new("wsl.exe"))
        .args(["--distribution", distro, "--exec", "wslpath", "-u", raw])
        .output()
        .map_err(|e| format!("{distro}: 无法转换安装路径: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{distro}: wslpath 失败: {}",
            decode_output(&out.stderr).trim()
        ));
    }
    let path = decode_output(&out.stdout).trim().to_string();
    if path.is_empty() {
        Err(format!("{distro}: wslpath 返回空路径"))
    } else {
        Ok(path)
    }
}

fn runtime_id(host: &str, distro: &str) -> String {
    let suffix: String = distro
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let digest = Sha256::digest(distro.as_bytes());
    let short_hash = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{host}-wsl-{}-{short_hash:08x}", suffix.trim_matches('-'))
}

fn hidden(mut cmd: Command) -> Command {
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// wsl.exe 在不同 Windows/控制台配置下会输出 UTF-8 或 UTF-16LE，两种都接。
fn decode_output(bytes: &[u8]) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let looks_utf16 =
        bytes.len() >= 2 && pairs.iter().filter(|pair| pair[1] == 0).count() * 2 >= bytes.len() / 2;
    if looks_utf16 {
        let words: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf16_wsl_list() {
        let words: Vec<u16> = "Ubuntu-22.04\r\ndocker-desktop\r\n"
            .encode_utf16()
            .collect();
        let bytes: Vec<u8> = words.into_iter().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_output(&bytes), "Ubuntu-22.04\r\ndocker-desktop\r\n");
    }

    #[test]
    fn runtime_id_is_stable_and_safe() {
        let id = runtime_id("desktop-123", "Ubuntu 22.04");
        assert!(id.starts_with("desktop-123-wsl-ubuntu-22-04-"));
        assert_eq!(id, runtime_id("desktop-123", "Ubuntu 22.04"));
        assert_ne!(id, runtime_id("desktop-123", "Ubuntu-22.04"));
    }
}
