//! WSL 的终端输入注入。
//!
//! 新版 Linux 默认拒绝普通用户调用 TIOCSTI（实测 WSL 返回 EPERM），而 Windows 登录用户
//! 本来就拥有自己 WSL 发行版的 root 权限。这里通过 wsl.exe 在同一发行版内启动一次短命
//! root 子命令，只把文本/按键写入已由本客户端验证过 pid 的 TTY；采集主进程仍以普通用户运行。

#![cfg(target_os = "linux")]

use anyhow::{anyhow, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

pub fn is_wsl() -> bool {
    std::env::var("WSL_DISTRO_NAME")
        .ok()
        .is_some_and(|s| !s.trim().is_empty())
}

pub fn send_input(pid: u32, text: &str, submit: bool) -> Result<&'static str> {
    if !is_wsl() {
        return am_core::process::send_input_ex(pid, text, submit);
    }
    root_call("wsl-root-input", pid, submit.then_some("1"), text)?;
    Ok("已通过 WSL TTY 注入")
}

pub fn send_keys(pid: u32, spec: &str) -> Result<&'static str> {
    if !is_wsl() {
        return am_core::process::send_terminal_keys(pid, spec);
    }
    root_call("wsl-root-keys", pid, None, spec)?;
    Ok("已通过 WSL TTY 注入按键")
}

fn root_call(mode: &str, pid: u32, extra: Option<&str>, body: &str) -> Result<()> {
    let distro = std::env::var("WSL_DISTRO_NAME")?;
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new("wsl.exe");
    cmd.args(["--distribution", distro.trim(), "--user", "root", "--exec"])
        .arg(exe)
        .arg(mode)
        .arg(pid.to_string());
    if let Some(value) = extra {
        cmd.arg(value);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("无法启动 WSL root 输入代理: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(body.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "WSL root 输入代理失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// root 短命子命令入口。返回 Some 表示当前进程就是输入代理，不应继续启动采集服务。
pub fn run_root_cli() -> Option<Result<()>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next()?;
    if mode != "wsl-root-input" && mode != "wsl-root-keys" {
        return None;
    }
    let result = (|| {
        let pid = args
            .next()
            .ok_or_else(|| anyhow!("缺少 pid"))?
            .parse::<u32>()?;
        let mut body = String::new();
        std::io::stdin().read_to_string(&mut body)?;
        if mode == "wsl-root-input" {
            let submit = args.next().as_deref() == Some("1");
            am_core::process::send_input_ex(pid, &body, submit)?;
        } else {
            am_core::process::send_terminal_keys(pid, &body)?;
        }
        Ok(())
    })();
    Some(result)
}
