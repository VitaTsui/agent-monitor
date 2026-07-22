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
            // 只监控「终端会话」：
            // - unix：没有控制终端（TTY）的代理进程是 IDE 插件/后台服务 ——
            //   典型如 Cursor 的 Codex 插件常驻进程，用户并没有开任何 codex
            //   终端会话，却会被采集成一条「codex 终端」。
            #[cfg(unix)]
            if tty.is_empty() {
                continue;
            }
            // - Windows 拿不到 tty，改用父链启发式：终端里跑的代理其父链必有
            //   shell（powershell/cmd/bash…）；IDE 插件进程由扩展宿主直接拉起，
            //   父链没有 shell（实测 Cursor 的 Codex 插件即如此，cwd 还是
            //   Cursor 安装目录）。
            #[cfg(windows)]
            if !self.has_shell_ancestor(*pid) {
                continue;
            }
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

    /// Windows：父链里是否存在 shell（终端会话的标志）。
    /// IDE 插件/后台服务由扩展宿主直接拉起，父链没有 shell。
    #[cfg(windows)]
    fn has_shell_ancestor(&self, pid: Pid) -> bool {
        let mut cur = pid.as_u32();
        for _ in 0..16 {
            if cur <= 1 {
                break;
            }
            let Some(proc_) = self.sys.process(Pid::from_u32(cur)) else {
                break;
            };
            let name = proc_.name().to_lowercase();
            if matches!(
                name.as_str(),
                "powershell.exe" | "pwsh.exe" | "cmd.exe" | "bash.exe" | "sh.exe"
                    | "wsl.exe" | "nu.exe" | "powershell" | "pwsh" | "cmd" | "bash"
            ) {
                return true;
            }
            match proc_.parent().map(|p| p.as_u32()) {
                Some(pp) if pp != cur => cur = pp,
                _ => break,
            }
        }
        false
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
    // 只认「精确命中」：进程名、可执行文件基名、node 包装脚本的路径分量/基名。
    // 绝不能在整串命令行里 contains 子串 —— MCP 配置路径、扩展目录等参数里
    // 带个 "codex"/"claude" 字样，就会把无关进程识别成代理
    // （用户实际遇到：只开了 Claude Code，列表里却多出两个 codex）。
    fn base(s: &str) -> &str {
        s.rsplit(['/', '\\']).next().unwrap_or(s)
    }
    for (agent, needle) in [
        ("claude", "claude"),
        ("codex", "codex"),
        ("gemini", "gemini"),
        ("aider", "aider"),
        ("opencode", "opencode"),
    ] {
        let exe = format!("{needle}.exe");
        if name == needle || name == exe {
            return Some(agent);
        }
        let Some(first) = cmd.first() else { continue };
        let fb = base(first);
        if fb == needle || fb == exe {
            return Some(agent);
        }
        // node 包装（npm 全局安装形态，如 node …/@anthropic-ai/claude-code/cli.js）：
        // 只看被执行脚本（第二个参数）的路径分量与基名，不看后续业务参数
        let is_node = name.starts_with("node") || fb == "node" || fb == "node.exe";
        if is_node {
            if let Some(script) = cmd.get(1) {
                let pkg = format!("{needle}-code");
                let sb = base(script);
                let sb_noext = sb
                    .strip_suffix(".js")
                    .or_else(|| sb.strip_suffix(".mjs"))
                    .or_else(|| sb.strip_suffix(".cjs"))
                    .unwrap_or(sb);
                if sb_noext == needle
                    || sb_noext == pkg
                    || script.split(['/', '\\']).any(|c| c == needle || c == pkg)
                {
                    return Some(agent);
                }
            }
        }
    }
    None
}

/// 对指定 pid 执行控制动作。返回动作的中文描述。
pub fn control(pid: u32, action: ControlAction) -> Result<&'static str> {
    // 安全下界：pid=0 会把信号发给整个进程组；pid>i32::MAX 转成 i32 会变负数，
    // kill(-1, SIGKILL) 将杀光当前用户的所有进程。这里一律拒绝。
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(anyhow!("非法 pid: {pid}"));
    }
    #[cfg(unix)]
    {
        let sig = match action {
            ControlAction::Pause => libc::SIGSTOP,
            ControlAction::Resume => libc::SIGCONT,
            ControlAction::Interrupt => libc::SIGINT,
            ControlAction::Stop => libc::SIGTERM,
            ControlAction::Kill => libc::SIGKILL,
            ControlAction::Input => return Err(anyhow!("Input 动作需走 send_input")),
            ControlAction::TermKey => {
                return Err(anyhow!("TermKey 动作需走 send_terminal_keys"))
            }
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
            ControlAction::TermKey => Err(anyhow!("TermKey 动作需走 send_terminal_keys")),
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
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(anyhow!("非法 pid: {pid}"));
    }
    #[cfg(unix)]
    {
        let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;

        #[cfg(target_os = "macos")]
        if let Ok(label) = applescript_write(&tty, text) {
            return Ok(label);
        }

        inject_tiocsti(&tty, text)
    }
    #[cfg(windows)]
    {
        windows_send_input(pid, text)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, text);
        Err(anyhow!("当前平台暂不支持远程发布任务"))
    }
}

/// 向终端注入按键（不提交），用于「撤回排队(↑)」「插入排队(Esc)」。
/// spec："up:3" = 按 3 次上键；"esc" = 按 1 次 Esc。仅 iTerm2(mac) 与 Windows 控制台
/// 可干净注入；Terminal.app 无法在不切前台的前提下注入方向键 → 返回错误（前端走提示）。
pub fn send_terminal_keys(pid: u32, spec: &str) -> Result<&'static str> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(anyhow!("非法 pid: {pid}"));
    }
    let (key, count) = parse_key_spec(spec);
    if count == 0 {
        return Ok("无按键");
    }
    #[cfg(target_os = "macos")]
    {
        mac_send_key(pid, key, count)
    }
    #[cfg(windows)]
    {
        windows_send_key(pid, key, count)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;
        tiocsti_send_key(&tty, key, count)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (key, count);
        Err(anyhow!("当前平台不支持按键注入"))
    }
}

/// 解析 "up:3" / "esc" → (键名, 次数)。次数封顶 50 防误触发风暴。
fn parse_key_spec(spec: &str) -> (&str, usize) {
    let mut it = spec.splitn(2, ':');
    let key = it.next().unwrap_or("").trim();
    let count = it
        .next()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .unwrap_or(1);
    (key, count.min(50))
}

/// 键名 → 终端转义字节序列。↑ 用普通光标模式 ESC[A；Esc 单字节。
#[cfg(all(unix, not(target_os = "macos")))]
fn key_seq(key: &str) -> Option<&'static [u8]> {
    match key {
        "up" => Some(b"\x1b[A"),
        "esc" => Some(b"\x1b"),
        _ => None,
    }
}

/// macOS 终端按键注入：先试 iTerm2（write text 转义序列，无需切前台），
/// 匹配不到再试 Terminal.app（System Events key code，需切前台 + 辅助功能权限）。
#[cfg(target_os = "macos")]
fn mac_send_key(pid: u32, key: &str, count: usize) -> Result<&'static str> {
    let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;
    let tty_e = tty.replace('\\', "\\\\").replace('"', "\\\"");

    // 1) iTerm2：write text 直接把转义序列写进会话，不切前台
    let seq_expr = match key {
        "up" => "(character id 27) & \"[A\"",
        "esc" => "(character id 27)",
        _ => return Err(anyhow!("未知按键: {key}")),
    };
    let iterm = format!(
        r#"tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        if (tty of s) is "{tty_e}" then
          repeat {count} times
            tell s to write text ({seq_expr}) newline no
          end repeat
          return "ok"
        end if
      end repeat
    end repeat
  end repeat
end tell
return "notfound""#
    );
    if run_osascript(&iterm).map(|o| o.contains("ok")).unwrap_or(false) {
        return Ok("已注入按键");
    }

    // 2) Terminal.app：do script 送不了方向键，只能把目标标签页切到前台，再用
    // System Events 发键码（key code 126=↑，53=Esc）。切前台不可避免；且需在
    // 系统设置→隐私与安全性→辅助功能里允许「终端任务监控」，否则 System Events 被拒。
    let keycode = match key {
        "up" => 126,
        "esc" => 53,
        _ => return Err(anyhow!("未知按键: {key}")),
    };
    let terminal = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if (tty of t) is "{tty_e}" then
        set selected of t to true
        set index of w to 1
        activate
        delay 0.2
        tell application "System Events"
          repeat {count} times
            key code {keycode}
            delay 0.04
          end repeat
        end tell
        return "ok"
      end if
    end repeat
  end repeat
end tell
return "notfound""#
    );
    match run_osascript(&terminal) {
        Ok(o) if o.contains("ok") => Ok("已注入按键"),
        Ok(o) if o.contains("notfound") => Err(anyhow!("未匹配到 iTerm2/Terminal.app 会话")),
        Ok(_) => Ok("已注入按键"),
        // System Events 被辅助功能权限拦截时 osascript 报错，给出可操作提示
        Err(e) => Err(anyhow!(
            "Terminal.app 按键注入失败（{e}）。若提示无权限，请到「系统设置→隐私与安全性→辅助功能」允许「终端任务监控」。"
        )),
    }
}

/// Linux：TIOCSTI 把转义序列逐字节注入 count 次。
#[cfg(all(unix, not(target_os = "macos")))]
fn tiocsti_send_key(tty: &str, key: &str, count: usize) -> Result<&'static str> {
    use std::os::unix::io::AsRawFd;
    let seq = key_seq(key).ok_or_else(|| anyhow!("未知按键: {key}"))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tty)
        .map_err(|e| anyhow!("注入失败：无法打开终端 {tty}（{e}）"))?;
    let fd = file.as_raw_fd();
    for _ in 0..count {
        for &b in seq {
            let c = b as libc::c_char;
            let ret = unsafe { libc::ioctl(fd, libc::TIOCSTI, &c) };
            if ret != 0 {
                return Err(anyhow!("注入失败：TIOCSTI 被系统禁用或权限不足"));
            }
        }
    }
    Ok("已注入按键")
}

/// Windows：AttachConsole + WriteConsoleInput 发虚拟键（按下+抬起）count 次。
#[cfg(windows)]
fn windows_send_key(pid: u32, key: &str, count: usize) -> Result<&'static str> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (vk, uch): (u16, u16) = match key {
        "up" => (0x26, 0),   // VK_UP，非可打印字符 → UnicodeChar 0
        "esc" => (0x1B, 27), // VK_ESCAPE，UnicodeChar = ESC
        _ => return Err(anyhow!("未知按键: {key}")),
    };
    let dir = std::env::temp_dir();
    let ps_path = dir.join(format!("am-key-{pid}-{}.ps1", std::process::id()));
    let script = format!(
        r#"param([int]$TargetPid,[int]$Vk,[int]$Uch,[int]$Count)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class AmKey {{
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool AttachConsole(uint pid);
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool FreeConsole();
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct KEY_EVENT_RECORD {{ public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode; public ushort wVirtualScanCode; public char UnicodeChar; public uint dwControlKeyState; }}
  [StructLayout(LayoutKind.Explicit)] public struct INPUT_RECORD {{ [FieldOffset(0)] public ushort EventType; [FieldOffset(4)] public KEY_EVENT_RECORD Key; }}
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode,EntryPoint="WriteConsoleInputW")] public static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
  static INPUT_RECORD Mk(ushort vk, char uc, bool down){{ var r=new INPUT_RECORD(); r.EventType=1; var k=new KEY_EVENT_RECORD(); k.bKeyDown=down?1:0; k.wRepeatCount=1; k.wVirtualKeyCode=vk; k.wVirtualScanCode=0; k.UnicodeChar=uc; k.dwControlKeyState=0; r.Key=k; return r; }}
  static bool WriteAll(IntPtr h, List<INPUT_RECORD> recs){{
    int i=0; var arr=recs.ToArray();
    while(i<arr.Length){{ int n=Math.Min(8, arr.Length-i); var chunk=new INPUT_RECORD[n]; Array.Copy(arr,i,chunk,0,n); uint w; if(!WriteConsoleInput(h, chunk, (uint)n, out w) || w==0) return false; i+=(int)w; }}
    return true;
  }}
  public static bool Send(uint pid, ushort vk, char uc, int count){{
    FreeConsole();
    if(!AttachConsole(pid)) return false;
    try {{
      IntPtr h=CreateFileW("CONIN$",0xC0000000u,3u,IntPtr.Zero,3u,0u,IntPtr.Zero);
      if(h==(IntPtr)(-1)) return false;
      var recs=new List<INPUT_RECORD>();
      for(int i=0;i<count;i++){{ recs.Add(Mk(vk,uc,true)); recs.Add(Mk(vk,uc,false)); }}
      return WriteAll(h, recs);
    }} finally {{ FreeConsole(); }}
  }}
}}
'@
Add-Type -TypeDefinition $code -Language CSharp
if([AmKey]::Send([uint32]$TargetPid,[uint16]$Vk,[char]$Uch,$Count)){{ exit 0 }} else {{ exit 2 }}
"#
    );
    std::fs::write(&ps_path, script).map_err(|e| anyhow!("写入临时脚本失败: {e}"))?;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(&ps_path)
        .arg("-TargetPid")
        .arg(pid.to_string())
        .arg("-Vk")
        .arg(vk.to_string())
        .arg("-Uch")
        .arg(uch.to_string())
        .arg("-Count")
        .arg(count.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = std::fs::remove_file(&ps_path);
    match out {
        Ok(o) if o.status.success() => Ok("已注入按键"),
        Ok(o) => Err(anyhow!(
            "按键注入失败（进程可能非控制台程序或权限不足）。{}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(anyhow!("powershell 执行失败: {e}")),
    }
}

/// Windows：把文本注入目标控制台进程的输入缓冲。
/// 机制：AttachConsole(pid) 挂到 claude 所在控制台（含 Windows Terminal/VS Code 的
/// ConPTY 伪控制台）→ WriteConsoleInput 写入按键事件 → FreeConsole 复原。经由一段临时
/// PowerShell 脚本（Add-Type P/Invoke）执行，文本走临时文件传递以彻底避开转义问题。
/// 多行用 bracketed paste 包裹，内部换行只当文本、不提前提交（与 Unix 路径一致）。
#[cfg(windows)]
fn windows_send_input(pid: u32, text: &str) -> Result<&'static str> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Windows Terminal（ConPTY）：WriteConsoleInput 对伪控制台不可靠（可能报错、也可能
    // 「成功却没送达」），故先判定——父链里有 WindowsTerminal.exe 就直接走聚焦粘贴，不再
    // 尝试 WriteConsoleInput。传统 conhost 控制台父链里没有它，继续走下面的 WriteConsoleInput。
    if let Some(wt_pid) = windows_wt_pid(pid) {
        return windows_paste_send(wt_pid, text);
    }

    // 多行包 bracketed paste：ESC[200~ … ESC[201~，末尾 Enter 在包裹外提交整块
    let payload = if text.contains('\n') {
        format!("\u{1b}[200~{text}\u{1b}[201~")
    } else {
        text.to_string()
    };

    let dir = std::env::temp_dir();
    let stamp = std::process::id();
    let txt_path = dir.join(format!("am-send-{pid}-{stamp}.txt"));
    let ps_path = dir.join(format!("am-send-{pid}-{stamp}.ps1"));
    std::fs::write(&txt_path, payload.as_bytes())
        .map_err(|e| anyhow!("写入临时文本失败: {e}"))?;

    // 脚本：读文本 → 逐字符写 KEY_EVENT_RECORD → 末尾补一个回车提交
    let script = r#"param([int]$TargetPid,[string]$TextFile)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class AmConIn {
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool AttachConsole(uint pid);
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool FreeConsole();
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct KEY_EVENT_RECORD { public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode; public ushort wVirtualScanCode; public char UnicodeChar; public uint dwControlKeyState; }
  [StructLayout(LayoutKind.Explicit)] public struct INPUT_RECORD { [FieldOffset(0)] public ushort EventType; [FieldOffset(4)] public KEY_EVENT_RECORD Key; }
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode,EntryPoint="WriteConsoleInputW")] public static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
  static INPUT_RECORD Mk(char c, ushort vk, bool down){ var r=new INPUT_RECORD(); r.EventType=1; var k=new KEY_EVENT_RECORD(); k.bKeyDown=down?1:0; k.wRepeatCount=1; k.wVirtualKeyCode=vk; k.wVirtualScanCode=0; k.UnicodeChar=c; k.dwControlKeyState=0; r.Key=k; return r; }
  // 一次写太多记录会撑爆控制台输入缓冲、报 ERROR_INSUFFICIENT_BUFFER(0x8007007A) —— 长
  // 输入注入失败正因如此。改成每 8 条一批分次写，每批失败即回退。
  static bool WriteAll(IntPtr h, System.Collections.Generic.List<INPUT_RECORD> recs){
    int i=0; var arr=recs.ToArray();
    while(i<arr.Length){
      int n=Math.Min(8, arr.Length-i);
      var chunk=new INPUT_RECORD[n];
      Array.Copy(arr,i,chunk,0,n);
      uint w;
      if(!WriteConsoleInput(h, chunk, (uint)n, out w) || w==0) return false;
      i+=(int)w;
    }
    return true;
  }
  public static bool Send(uint pid, string text){
    FreeConsole();
    if(!AttachConsole(pid)) return false;
    try {
      IntPtr h=CreateFileW("CONIN$",0xC0000000u,3u,IntPtr.Zero,3u,0u,IntPtr.Zero);
      if(h==(IntPtr)(-1)) return false;
      var recs=new System.Collections.Generic.List<INPUT_RECORD>();
      foreach(char c in text){ recs.Add(Mk(c,0,true)); recs.Add(Mk(c,0,false)); }
      recs.Add(Mk('\r',0x0D,true)); recs.Add(Mk('\r',0x0D,false));
      return WriteAll(h, recs);
    } finally { FreeConsole(); }
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
$t=[System.IO.File]::ReadAllText($TextFile,[System.Text.Encoding]::UTF8)
if([AmConIn]::Send([uint32]$TargetPid,$t)){ exit 0 } else { exit 2 }
"#;
    std::fs::write(&ps_path, script).map_err(|e| anyhow!("写入临时脚本失败: {e}"))?;

    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&ps_path)
        .arg("-TargetPid")
        .arg(pid.to_string())
        .arg("-TextFile")
        .arg(&txt_path)
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    // 尽力清理临时文件（失败无妨，系统临时目录会被回收）
    let _ = std::fs::remove_file(&txt_path);
    let _ = std::fs::remove_file(&ps_path);
    let _ = std::io::stdout().flush();

    match out {
        Ok(o) if o.status.success() => Ok("已发送（WriteConsoleInput）"),
        Ok(o) => {
            // WriteConsoleInput 失败：传统 conhost 控制台一般能成功，失败多半是 Windows
            // Terminal（ConPTY）——伪控制台的输入管道由宿主持有，外部写不进去。退回
            // 「聚焦该 Windows Terminal 窗口 + 剪贴板粘贴 + 回车」模拟输入。
            let primary = String::from_utf8_lossy(&o.stderr).trim().to_string();
            match windows_wt_pid(pid) {
                Some(wt_pid) => windows_paste_send(wt_pid, text).map_err(|e| {
                    anyhow!("WriteConsoleInput 失败；Windows Terminal 粘贴回退也失败：{e}（原始：{primary}）")
                }),
                None => Err(anyhow!(
                    "注入失败：AttachConsole/WriteConsoleInput 未成功（进程可能非控制台程序或权限不足）。{primary}"
                )),
            }
        }
        Err(e) => Err(anyhow!("powershell 执行失败: {e}")),
    }
}

/// 沿父进程链找 Cursor/VSCode 内嵌终端：若父链里出现 Cursor.exe/Code.exe，返回其「之下最近
/// 的 shell pid」——即该内嵌终端在扩展里 `terminal.processId` 的值，客户端据此把任务经文件桥
/// 交给扩展用 `terminal.sendText` 送达（ConPTY 内嵌终端无法用 WriteConsoleInput 注入）。
/// 不是 IDE 内嵌终端则返回 None。
pub fn ide_shell_pid(claude_pid: u32) -> Option<u32> {
    use sysinfo::{Pid, System};
    let is_shell = |n: &str| {
        matches!(
            n,
            "powershell.exe" | "pwsh.exe" | "cmd.exe" | "bash.exe" | "nu.exe" | "wsl.exe"
                | "bash" | "zsh" | "sh" | "fish" | "nu" | "pwsh" | "powershell" | "-zsh" | "-bash"
        )
    };
    // 命中 Cursor/VSCode 宿主（含 mac 的 helper/electron 命名，与 detect_ide 一致）
    let is_ide = |n: &str| {
        n.contains("cursor")
            || n == "code.exe"
            || n == "code"
            || n.contains("code helper")
            || n.contains("code - ")
    };
    let mut sys = System::new();
    sys.refresh_processes();
    let mut cur = claude_pid;
    let mut last_shell: Option<u32> = None;
    for _ in 0..24 {
        let p = sys.process(Pid::from_u32(cur))?;
        let name = p.name().to_lowercase();
        if is_shell(&name) {
            last_shell = Some(cur);
        }
        if is_ide(&name) {
            return last_shell;
        }
        match p.parent().map(|pp| pp.as_u32()) {
            Some(pp) if pp != cur && pp > 1 => cur = pp,
            _ => return None,
        }
    }
    None
}

/// 沿父进程链找 WindowsTerminal.exe（Windows Terminal 宿主），找到返回其 pid（用于定位
/// 它的窗口做聚焦粘贴）；传统 conhost 控制台的父链里没有它 → 返回 None，仍走 WriteConsoleInput。
#[cfg(windows)]
fn windows_wt_pid(claude_pid: u32) -> Option<u32> {
    use sysinfo::{Pid, System};
    let mut sys = System::new();
    sys.refresh_processes();
    let mut cur = claude_pid;
    for _ in 0..24 {
        let p = sys.process(Pid::from_u32(cur))?;
        if p.name().to_lowercase().contains("windowsterminal") {
            return Some(cur);
        }
        match p.parent().map(|pp| pp.as_u32()) {
            Some(pp) if pp != cur && pp > 4 => cur = pp,
            _ => return None,
        }
    }
    None
}

/// Windows Terminal 回退：把文本放剪贴板 → 聚焦 wt_pid 的可见窗口 → 发 Ctrl+V + 回车。
/// 局限：会抢前台焦点；多标签页时粘到「当前活动标签」，claude 不在活动标签则会送错——
/// 这是已有 WT 标签页对外注入的固有限制（见 CreatePseudoConsole 文档）。
#[cfg(windows)]
fn windows_paste_send(wt_pid: u32, text: &str) -> Result<&'static str> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let dir = std::env::temp_dir();
    let stamp = std::process::id();
    let txt_path = dir.join(format!("am-paste-{wt_pid}-{stamp}.txt"));
    let ps_path = dir.join(format!("am-paste-{wt_pid}-{stamp}.ps1"));
    std::fs::write(&txt_path, text.as_bytes()).map_err(|e| anyhow!("写入临时文本失败: {e}"))?;

    // 找到该进程的可见顶层窗口 → SetForegroundWindow → keybd_event 发 Ctrl+V、回车。
    // 剪贴板用 PowerShell 的 Set-Clipboard/Get-Clipboard 存取并复原，省去 C# 剪贴板 P/Invoke。
    let script = r#"param([int]$WtPid,[string]$TextFile)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
public class AmPaste {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  const uint KEYUP=2; const byte VK_CTRL=0x11, VK_V=0x56, VK_RET=0x0D;
  static IntPtr FindWin(uint target){
    IntPtr found=IntPtr.Zero;
    EnumWindows((h,p)=>{ uint wp; GetWindowThreadProcessId(h,out wp); if(wp==target && IsWindowVisible(h)){ found=h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static bool Run(uint pid){
    IntPtr h=FindWin(pid);
    if(h==IntPtr.Zero) return false;
    ShowWindow(h,9); SetForegroundWindow(h);
    System.Threading.Thread.Sleep(180);
    keybd_event(VK_CTRL,0,0,IntPtr.Zero); keybd_event(VK_V,0,0,IntPtr.Zero);
    keybd_event(VK_V,0,KEYUP,IntPtr.Zero); keybd_event(VK_CTRL,0,KEYUP,IntPtr.Zero);
    System.Threading.Thread.Sleep(140);
    keybd_event(VK_RET,0,0,IntPtr.Zero); keybd_event(VK_RET,0,KEYUP,IntPtr.Zero);
    return true;
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
$t=[System.IO.File]::ReadAllText($TextFile,[System.Text.Encoding]::UTF8)
$old=''
try { $old=Get-Clipboard -Raw } catch {}
Set-Clipboard -Value $t
$ok=[AmPaste]::Run([uint32]$WtPid)
Start-Sleep -Milliseconds 250
try { if($old -ne $null){ Set-Clipboard -Value $old } } catch {}
if($ok){ exit 0 } else { exit 4 }
"#;
    std::fs::write(&ps_path, script).map_err(|e| anyhow!("写入临时脚本失败: {e}"))?;

    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&ps_path)
        .arg("-WtPid")
        .arg(wt_pid.to_string())
        .arg("-TextFile")
        .arg(&txt_path)
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = std::fs::remove_file(&txt_path);
    let _ = std::fs::remove_file(&ps_path);
    let _ = std::io::stdout().flush();
    match out {
        Ok(o) if o.status.success() => Ok("已发送（Windows Terminal 聚焦粘贴）"),
        Ok(o) => Err(anyhow!(
            "找不到 Windows Terminal 窗口或粘贴失败。{}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(anyhow!("powershell 执行失败: {e}")),
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
    // 多行内容用 bracketed paste 包裹：TUI（Claude Code 等）会把块内换行当
    // 文本而非提交键，否则第一个 \n 就提交了前半句、剩余卡在输入框里出不去
    let mut bytes = Vec::with_capacity(text.len() + 16);
    if text.contains('\n') {
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }
    // 提交键必须是回车 CR(\r=0x0D)，不能用换行 LF(\n)：TUI（Claude Code 等）把 CR 当
    // 「提交」、把 LF 当「输入里换一行」。之前推 \n 导致文字进了输入框却只换行、不提交。
    bytes.push(b'\r');
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
    // 转义 AppleScript 字符串字面量。
    // 换行必须一起转：AppleScript 的字符串字面量不能跨行，文本里一个裸换行
    // 就会把字面量提前闭合，后面的内容被当成脚本解析（＝任意 AppleScript 注入）。
    let esc = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    };
    let tty_e = esc(tty);
    // 多行：包 bracketed paste（ESC [200~ … ESC [201~），换行只当文本；
    // write text/do script 末尾自带的回车在包裹外，负责提交整块
    let text_e = if text.contains('\n') {
        format!(
            "\" & (character id 27) & \"[200~{}\" & (character id 27) & \"[201~\" & \"",
            esc(text)
        )
    } else {
        esc(text)
    };

    // iTerm2：先只键入文本（newline no，不让它自动补换行——那有时是 LF、只换行不提交），
    // 停一下再单独送回车 CR(character id 13) 作提交。停顿关键在长/多行内容：claude 会进
    // 粘贴态，紧跟的回车会被并进粘贴而不提交（表现为「只换行」）；等它把粘贴吃完再回车才稳。
    // 停顿按内容长度递增：0.12s 起步、封顶 1s。
    let submit_delay = (0.12 + text.chars().count() as f64 / 3000.0).min(1.0);
    let iterm = format!(
        r#"tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        if (tty of s) is "{tty_e}" then
          tell s to write text "{text_e}" newline no
          delay {submit_delay:.2}
          tell s to write text (character id 13) newline no
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
        ControlAction::TermKey => "已注入按键",
    }
}

#[cfg(test)]
mod agent_kind_tests {
    use super::agent_kind;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn exact_matches() {
        assert_eq!(agent_kind("claude", &s(&["claude", "--flag"])), Some("claude"));
        assert_eq!(agent_kind("codex.exe", &s(&["C:\\bin\\codex.exe"])), Some("codex"));
        assert_eq!(agent_kind("zsh", &s(&["/usr/local/bin/claude"])), Some("claude"));
        // npm 全局安装形态：node + 包目录 claude-code
        assert_eq!(
            agent_kind("node", &s(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"])),
            Some("claude")
        );
        assert_eq!(agent_kind("node", &s(&["node", "/opt/bin/codex"])), Some("codex"));
    }

    /// 用户实际踩过：只开了 Claude Code，列表却多出 codex ——
    /// 业务参数（MCP 配置路径、扩展目录等）里的子串绝不能触发识别
    #[test]
    fn args_substrings_do_not_match() {
        assert_eq!(
            agent_kind("node", &s(&["node", "/x/mcp-server.js", "--config", "/Users/a/.claude/codex mcp.json"])),
            None,
            "第三个参数里的 codex/claude 字样不该命中"
        );
        assert_eq!(
            agent_kind("node", &s(&["node", "/app/extensions/vendor-codex-helper/main.js"])),
            None,
            "路径分量是 vendor-codex-helper 而非 codex，不该命中"
        );
        assert_eq!(agent_kind("Cursor Helper", &s(&["/Applications/Cursor.app/x"])), None);
        assert_eq!(agent_kind("claude-backup-tool", &s(&["claude-backup-tool"])), None);
    }
}
