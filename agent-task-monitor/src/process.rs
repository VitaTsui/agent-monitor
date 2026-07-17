use crate::model::{ControlAction, IdeKind, ProcessInfo};
use anyhow::{anyhow, Result};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

/// 扫描系统中所有 AI 代理进程（claude / codex …）
pub struct ProcessScanner {
    sys: System,
}

impl ProcessScanner {
    pub fn new() -> Self {
        Self {
            sys: System::new_with_specifics(
                RefreshKind::new().with_processes(
                    ProcessRefreshKind::new()
                        .with_cwd(UpdateKind::Always)
                        .with_cmd(UpdateKind::Always)
                        .with_cpu()
                        .with_memory(),
                ),
            ),
        }
    }

    /// 刷新并返回全部代理进程
    pub fn scan(&mut self) -> Vec<ProcessInfo> {
        self.sys.refresh_processes_specifics(
            ProcessRefreshKind::new()
                .with_cwd(UpdateKind::Always)
                .with_cmd(UpdateKind::Always)
                .with_cpu()
                .with_memory(),
        );

        let mut result = Vec::new();
        for (pid, proc_) in self.sys.processes() {
            let Some(agent) = agent_kind(proc_.name(), proc_.cmd()) else {
                continue;
            };
            let cwd = match proc_.cwd() {
                Some(p) => p.to_string_lossy().to_string(),
                None => continue,
            };
            let (ide, ide_name) = self.detect_ide(*pid);
            let tty = tty_path_of(pid.as_u32()).unwrap_or_default();
            result.push(ProcessInfo {
                pid: pid.as_u32(),
                agent: agent.to_string(),
                tty,
                cwd,
                ide,
                ide_name,
                start_time: proc_.start_time(),
                cpu_usage: proc_.cpu_usage(),
                memory: proc_.memory(),
                command: proc_.cmd().join(" "),
            });
        }
        result.sort_by_key(|p| p.start_time);
        result
    }

    /// 沿父进程链向上找宿主应用。
    /// 先走完整条链再判定：IDE（Cursor/VSCode）优先于终端宿主——
    /// Windows 上 IDE 内嵌终端的父链是 claude → powershell/cmd → Code.exe，
    /// 若遇到 shell 就提前返回会把 IDE 内嵌终端误判成独立终端。
    fn detect_ide(&self, pid: Pid) -> (IdeKind, String) {
        let mut chain: Vec<String> = Vec::new();
        let mut cur = pid.as_u32();
        for _ in 0..16 {
            if cur <= 1 {
                break;
            }
            let (name, parent) = match self.sys.process(Pid::from_u32(cur)) {
                Some(proc_) => (
                    proc_.name().to_string(),
                    proc_
                        .parent()
                        .map(|pp| pp.as_u32())
                        .or_else(|| ppid_via_ps(cur)),
                ),
                // sysinfo 读不到（如 root 拥有的 login）时用 ps 兜底
                None => (name_via_ps(cur).unwrap_or_default(), ppid_via_ps(cur)),
            };
            if !name.is_empty() {
                chain.push(name);
            }
            match parent {
                Some(pp) if pp != cur => cur = pp,
                _ => break,
            }
        }

        for name in &chain {
            let lower = name.to_lowercase();
            if lower.contains("cursor") {
                return (IdeKind::Cursor, "Cursor".into());
            }
            if lower.contains("code helper")
                || lower == "code"
                || lower == "code.exe"
                || lower.contains("code - ")
            {
                return (IdeKind::Vscode, "VSCode".into());
            }
        }

        // 终端模拟器（宿主应用）优先
        for name in &chain {
            let lower = name.to_lowercase();
            let host = [
                ("windowsterminal", "Windows Terminal"),
                ("iterm", "iTerm"),
                ("wezterm", "WezTerm"),
                ("alacritty", "Alacritty"),
                ("kitty", "kitty"),
                ("warp", "Warp"),
                ("ghostty", "Ghostty"),
                ("tmux", "tmux"),
                ("terminal", "Terminal"),
                ("conhost", "Windows Console"),
            ]
            .iter()
            .find(|(needle, _)| lower.contains(needle))
            .map(|(_, label)| label.to_string());
            if let Some(label) = host {
                return (IdeKind::Terminal, label);
            }
        }

        // 父链里没有终端宿主时，用 shell 本身兜底（Windows cmd/PowerShell 直开场景）
        for name in &chain {
            let lower = name.to_lowercase();
            if lower == "powershell.exe" || lower == "pwsh.exe" || lower == "powershell" || lower == "pwsh" {
                return (IdeKind::Terminal, "PowerShell".into());
            }
            if lower == "cmd.exe" || lower == "cmd" {
                return (IdeKind::Terminal, "CMD".into());
            }
        }

        (IdeKind::Other, "Unknown".into())
    }
}

/// macOS 上 sysinfo 拿不到 root 进程（如 login）的父子关系，用 ps 兜底
#[cfg(unix)]
fn ppid_via_ps(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(unix))]
fn ppid_via_ps(_pid: u32) -> Option<u32> {
    None
}

#[cfg(unix)]
fn name_via_ps(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // comm 可能是完整路径，取最后一段
    Some(s.rsplit('/').next().unwrap_or(&s).to_string()).filter(|x| !x.is_empty())
}

#[cfg(not(unix))]
fn name_via_ps(_pid: u32) -> Option<String> {
    None
}

/// 判断进程属于哪种 AI 编码代理；未来在此扩展新代理（如 gemini 等）
fn agent_kind(name: &str, cmd: &[String]) -> Option<&'static str> {
    for (agent, needle) in [("claude", "claude"), ("codex", "codex")] {
        if name == needle {
            return Some(agent);
        }
        if let Some(first) = cmd.first() {
            let is_node = name.starts_with("node") || first.ends_with("node") || first.ends_with("node.exe");
            let joined = cmd.join(" ");
            if is_node
                && (joined.contains(&format!("{needle}-code"))
                    || joined.contains(&format!("/{needle} "))
                    || joined.ends_with(&format!("/{needle}")))
            {
                return Some(agent);
            }
            if first.ends_with(&format!("/{needle}"))
                || first == needle
                || first.ends_with(&format!("\\{needle}.exe"))
            {
                return Some(agent);
            }
        }
    }
    None
}

/// 对指定 pid 执行控制动作。返回动作的中文描述。
pub fn control(pid: u32, action: ControlAction) -> Result<&'static str> {
    #[cfg(unix)]
    {
        let sig = match action {
            ControlAction::Pause => libc::SIGSTOP,
            ControlAction::Resume => libc::SIGCONT,
            ControlAction::Interrupt => libc::SIGINT,
            ControlAction::Stop => libc::SIGTERM,
            ControlAction::Kill => libc::SIGKILL,
            ControlAction::Input => return Err(anyhow!("Input 动作需走 send_input")),
        };
        let ret = unsafe { libc::kill(pid as i32, sig) };
        if ret != 0 {
            return Err(anyhow!("向进程 {} 发送信号失败（可能已退出或无权限）", pid));
        }
        Ok(action_label(action))
    }
    #[cfg(windows)]
    {
        match action {
            ControlAction::Stop | ControlAction::Kill | ControlAction::Interrupt => {
                let force = matches!(action, ControlAction::Kill);
                let mut cmd = std::process::Command::new("taskkill");
                cmd.arg("/PID").arg(pid.to_string());
                if force {
                    cmd.arg("/F");
                }
                let out = cmd.output().map_err(|e| anyhow!("taskkill 执行失败: {e}"))?;
                if !out.status.success() {
                    return Err(anyhow!(
                        "taskkill 失败: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                Ok(action_label(action))
            }
            ControlAction::Pause | ControlAction::Resume => {
                Err(anyhow!("Windows 平台暂不支持暂停/恢复"))
            }
            ControlAction::Input => Err(anyhow!("Input 动作需走 send_input")),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(anyhow!("当前平台不支持进程控制"))
    }
}

/// 向目标进程的控制终端注入一行输入（发布任务）。
/// macOS: 先按 tty 匹配 Terminal/iTerm2 会话用 AppleScript 写入（无需 root）；
///        失败再回退 TIOCSTI。其它 Unix: 直接 TIOCSTI。
pub fn send_input(pid: u32, text: &str) -> Result<&'static str> {
    #[cfg(unix)]
    {
        let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;

        #[cfg(target_os = "macos")]
        if let Ok(label) = applescript_write(&tty, text) {
            return Ok(label);
        }

        inject_tiocsti(&tty, text)
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, text);
        Err(anyhow!("当前平台暂不支持远程发布任务（仅 Unix 支持终端注入）"))
    }
}

/// TIOCSTI 逐字节注入（需能打开目标 tty；跨会话通常需 root）
#[cfg(unix)]
fn inject_tiocsti(tty: &str, text: &str) -> Result<&'static str> {
    use std::os::unix::io::AsRawFd;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tty)
        .map_err(|e| {
            anyhow!("注入失败：无法打开终端 {tty}（{e}）。跨会话注入通常需以 root 运行监控端，或使用 Terminal/iTerm。")
        })?;
    let fd = file.as_raw_fd();
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(b'\n');
    for b in bytes {
        let c = b as libc::c_char;
        let ret = unsafe { libc::ioctl(fd, libc::TIOCSTI, &c) };
        if ret != 0 {
            return Err(anyhow!("注入失败：TIOCSTI 被系统禁用或权限不足"));
        }
    }
    Ok("已发送")
}

/// macOS：按 tty 匹配 Terminal.app / iTerm2 的会话并写入文本（等价于键入并回车）
#[cfg(target_os = "macos")]
fn applescript_write(tty: &str, text: &str) -> Result<&'static str> {
    // 转义 AppleScript 字符串字面量
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let tty_e = esc(tty);
    let text_e = esc(text);

    // iTerm2：write text 会自动追加回车
    let iterm = format!(
        r#"tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        if (tty of s) is "{tty_e}" then
          tell s to write text "{text_e}"
          return "ok"
        end if
      end repeat
    end repeat
  end repeat
end tell
return "notfound""#
    );
    if run_osascript(&iterm).map(|o| o.contains("ok")).unwrap_or(false) {
        return Ok("已发送");
    }

    // Terminal.app：do script "..." in <tab> 会键入并回车
    let terminal = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if (tty of t) is "{tty_e}" then
        do script "{text_e}" in t
        return "ok"
      end if
    end repeat
  end repeat
end tell
return "notfound""#
    );
    if run_osascript(&terminal).map(|o| o.contains("ok")).unwrap_or(false) {
        return Ok("已发送");
    }

    Err(anyhow!("未匹配到 Terminal/iTerm 会话"))
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<String> {
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!("osascript: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

/// 跨平台：返回进程控制终端路径（windows 无 tty 返回 None）
pub fn tty_path_of(pid: u32) -> Option<String> {
    #[cfg(unix)]
    {
        tty_of(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// 解析进程的控制终端设备路径（如 /dev/ttys004）
#[cfg(unix)]
fn tty_of(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if t.is_empty() || t == "?" || t == "??" {
        return None;
    }
    if t.starts_with("/dev/") {
        Some(t)
    } else {
        Some(format!("/dev/{t}"))
    }
}

/// 查询进程是否处于停止（SIGSTOP）状态。仅 unix 有效，其余平台恒 false。
pub fn is_stopped(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        // ps -o stat= -p pid → 状态串首字母 T 表示 stopped
        if let Ok(out) = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            return s.trim_start().starts_with('T');
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            // 第三个字段是状态
            if let Some(state) = stat.split(") ").nth(1).and_then(|s| s.chars().next()) {
                return state == 'T';
            }
        }
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

fn action_label(action: ControlAction) -> &'static str {
    match action {
        ControlAction::Pause => "已暂停",
        ControlAction::Resume => "已恢复",
        ControlAction::Interrupt => "已发送中断",
        ControlAction::Stop => "已终止",
        ControlAction::Kill => "已强制终止",
        ControlAction::Input => "已发送",
    }
}
