use crate::model::{ControlAction, IdeKind, ProcessInfo};
use anyhow::{anyhow, Result};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

/// 扫描系统中所有 AI 代理进程（claude / codex …）
pub struct ProcessScanner {
    sys: System,
}

impl Default for ProcessScanner {
    fn default() -> Self {
        Self::new()
    }
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
            // 桌面客户端（ChatGPT.app / Claude.app）拉起的代理：不是终端会话，但确实是
            // 用户开的真会话，下面两条「只留终端会话」的判据都要给它让路。
            let desktop = ide == IdeKind::Desktop;
            // 服务模式（`codex app-server` 等）**在终端里**不是会话：它由某个真会话拉起、
            // tty 是继承来的，下面的 tty 判据挡不住，见 [`is_service_mode`]。
            // 但同一个子命令由桌面客户端拉起时，它就是那些桌面会话的宿主进程本身 ——
            // 判据是父链（桌面客户端 vs 终端/IDE），不是命令行长什么样。
            let shared_host = is_service_mode(agent, proc_.cmd());
            if shared_host && !desktop {
                continue;
            }
            let tty = tty_path_of(pid.as_u32()).unwrap_or_default();
            // 只监控「终端会话」与「桌面客户端会话」：
            // - unix：没有控制终端（TTY）、又不是桌面客户端拉起的代理进程 = IDE 插件/
            //   后台服务 —— 典型如 Cursor 的 Codex 插件常驻进程，用户并没有开任何 codex
            //   终端会话，却会被采集成一条「codex 终端」。它的父链里有 IDE，
            //   [`detect_ide`] 认得出来，于是 desktop 为假、照旧挡掉。
            #[cfg(unix)]
            if tty.is_empty() && !desktop {
                continue;
            }
            // - Windows 拿不到 tty，改用父链启发式：终端里跑的代理其父链必有
            //   shell（powershell/cmd/bash…）；IDE 插件进程由扩展宿主直接拉起，
            //   父链没有 shell（实测 Cursor 的 Codex 插件即如此，cwd 还是
            //   Cursor 安装目录）。桌面客户端识别目前只在 macOS 上成立（靠 .app 包），
            //   Windows 上 desktop 恒为假 —— 行为与本次改动前完全一致。
            #[cfg(windows)]
            if !self.has_shell_ancestor(*pid) && !desktop {
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
                shell_pid: None,
                shell_start: None,
                shared_host,
            });
        }
        // 终端锚：各 agent 最近的 shell 祖先 (pid, start)（复用本轮 self.sys，不额外扫描）
        for r in &mut result {
            match self.nearest_shell(r.pid) {
                Some((sp, ss)) => {
                    r.shell_pid = Some(sp);
                    r.shell_start = Some(ss);
                }
                None => {
                    r.shell_pid = None;
                    r.shell_start = None;
                }
            }
        }
        result.sort_by_key(|p| p.start_time);
        result
    }

    /// 该进程最近的 shell 祖先 (pid, 启动时间)（powershell/bash…），即它所在终端的 shell。
    /// 终端 shell 的 pid 跨 claude 的 /clear/--resume/重启都不变，用作配对稳定锚；start 一并
    /// 返回，供配对恢复时区分「同一个 shell」与「pid 被重用的新 shell」（Windows 会重用 pid）。
    fn nearest_shell(&self, pid: u32) -> Option<(u32, u64)> {
        let mut cur = pid;
        for _ in 0..24 {
            let p = self.sys.process(Pid::from_u32(cur))?;
            if is_shell_name(&p.name().to_lowercase()) {
                return Some((cur, p.start_time()));
            }
            match p.parent().map(|pp| pp.as_u32()) {
                Some(pp) if pp != cur && pp > 1 => cur = pp,
                _ => return None,
            }
        }
        None
    }

    /// 权威配对：claude 进程 pid → 它正在跑的会话 id。
    ///
    /// Claude Code 给它派生的每个子进程注入两个环境变量 `CLAUDE_PID`（拥有该子进程的
    /// claude 进程 pid）与 `CLAUDE_CODE_SESSION_ID`（= 会话 jsonl 文件名）。据此可得
    /// 「会话 ↔ claude 进程」的**权威链**，不依赖文件句柄（Windows 上 claude 写一行开一次
    /// 就关，句柄扫描抓不到）或时间戳启发式（并发同项目会话本质歧义）。
    ///
    /// 覆盖面：仅当会话**当前有活着的子进程**（正在跑工具/命令）时能取到；空闲会话没有
    /// 子进程 → 取不到，交回 build_tasks 的配对缓存（tier⑤）与 mtime 兜底。一旦活跃时
    /// 拿到过一次，缓存就把它粘住。环境块须单独刷新，开销较大，调用方应节流。
    ///
    /// 平台：environ 由 sysinfo 提供（Windows 读 PEB、Linux 读 /proc/<pid>/environ、
    /// macOS 读自有进程），无需自写 unsafe。取不到 environ 的进程被安全跳过 → 空表。
    pub fn session_pins(&mut self) -> std::collections::HashMap<u32, String> {
        self.sys.refresh_processes_specifics(
            ProcessRefreshKind::new().with_environ(UpdateKind::Always),
        );
        // 采集候选：谁（reporter）在 env 里声称「CLAUDE_PID → session_id」，同时建进程图
        // （父链 + 存活 claude 集合），供 resolve_session_pins 做「存活祖先」校验。
        let mut candidates: Vec<(u32, u32, String)> = Vec::new();
        let mut parent_of: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut alive_claude: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (pid, proc_) in self.sys.processes() {
            let pid = pid.as_u32();
            if let Some(pp) = proc_.parent().map(|p| p.as_u32()) {
                parent_of.insert(pid, pp);
            }
            // 精确识别 claude（agent_kind 会挡掉 claude-backup-tool 之类子串误判），
            // 不用松散的 name.contains("claude")
            if agent_kind(proc_.name(), proc_.cmd()) == Some("claude") {
                alive_claude.insert(pid);
            }
            let (mut claude_pid, mut session_id) = (None, None);
            for kv in proc_.environ() {
                if let Some(v) = kv.strip_prefix("CLAUDE_PID=") {
                    claude_pid = v.trim().parse::<u32>().ok();
                } else if let Some(v) = kv.strip_prefix("CLAUDE_CODE_SESSION_ID=") {
                    if !v.is_empty() {
                        session_id = Some(v.to_string());
                    }
                }
            }
            if let (Some(cp), Some(sid)) = (claude_pid, session_id) {
                candidates.push((pid, cp, sid));
            }
        }
        resolve_session_pins(&candidates, &parent_of, &alive_claude)
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
                "powershell.exe"
                    | "pwsh.exe"
                    | "cmd.exe"
                    | "bash.exe"
                    | "sh.exe"
                    | "wsl.exe"
                    | "nu.exe"
                    | "powershell"
                    | "pwsh"
                    | "cmd"
                    | "bash"
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
        // (进程名, 可执行文件路径)。路径只有桌面客户端判据用得上（要认 .app 包），
        // 名字判据照旧只看名字。
        let mut chain: Vec<(String, String)> = Vec::new();
        let mut cur = pid.as_u32();
        for _ in 0..16 {
            if cur <= 1 {
                break;
            }
            let (name, exe, parent) = match self.sys.process(Pid::from_u32(cur)) {
                Some(proc_) => (
                    proc_.name().to_string(),
                    proc_.cmd().first().cloned().unwrap_or_default(),
                    proc_
                        .parent()
                        .map(|pp| pp.as_u32())
                        .or_else(|| ppid_via_ps(cur)),
                ),
                // sysinfo 读不到（如 root 拥有的 login）时用 ps 兜底
                None => (
                    name_via_ps(cur).unwrap_or_default(),
                    String::new(),
                    ppid_via_ps(cur),
                ),
            };
            if !name.is_empty() || !exe.is_empty() {
                chain.push((name, exe));
            }
            match parent {
                Some(pp) if pp != cur => cur = pp,
                _ => break,
            }
        }
        classify_chain(&chain)
    }
}

/// 由父进程链（链首 = 进程自己，往后是祖先）判定宿主类型。
///
/// 单独拆出来是为了能直接喂造好的链做测试 —— 判据全在链上，不需要真去起一个
/// Cursor 插件宿主或 ChatGPT.app 才能验。
fn classify_chain(chain: &[(String, String)]) -> (IdeKind, String) {
    let names: Vec<&String> = chain.iter().map(|(n, _)| n).collect();

    for name in &names {
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
    for name in &names {
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

    // 桌面客户端：父链里既没有 IDE 也没有终端模拟器，却有一个 GUI 应用包**直接**托着
    // 它 —— 这就是 ChatGPT.app / Claude.app 从自己进程里拉起代理的形态（实测
    // `…/ChatGPT.app/Contents/Resources/codex … app-server` 的父进程正是
    // `/Applications/ChatGPT.app/Contents/MacOS/ChatGPT`）。
    //
    // 两条边界，缺一条就会把终端会话认成桌面会话：
    // - 跳过链首（进程自己）：代理二进制本身也可能装在 .app 里（实测 Claude 桌面版
    //   本地代理跑的是 `…/claude-code/<ver>/claude.app/Contents/MacOS/claude`），
    //   拿它当宿主就成了自己托自己。
    // - 遇到 shell 就停：终端里跑的代理，父链一定是 代理 → shell → 终端应用，而终端
    //   应用同样是个 `.app`。上面那份终端名单必然漏（Alacritty / kitty / WezTerm /
    //   Hyper…），漏掉的就会在这里被认成「桌面客户端」，进而绕过 tty 判据 ——
    //   在这种终端里跑 `codex mcp` 就又会冒出假会话卡。桌面客户端拉起的代理中间
    //   没有 shell，这条判据不认名字、只认形态。
    //
    // 只在 macOS 成立：判据是 .app 包结构。别的平台走不到这里，返回 Unknown，
    // 与本次改动前一致。
    for (_, exe) in chain
        .iter()
        .skip(1)
        .take_while(|(name, _)| !is_shell_name(&name.to_lowercase()))
    {
        if let Some(app) = app_bundle_name(exe) {
            return (IdeKind::Desktop, app.to_string());
        }
    }

    // 父链里没有终端宿主时，用 shell 本身兜底（Windows cmd/PowerShell 直开场景）
    for name in &names {
        let lower = name.to_lowercase();
        if lower == "powershell.exe"
            || lower == "pwsh.exe"
            || lower == "powershell"
            || lower == "pwsh"
        {
            return (IdeKind::Terminal, "PowerShell".into());
        }
        if lower == "cmd.exe" || lower == "cmd" {
            return (IdeKind::Terminal, "CMD".into());
        }
    }

    (IdeKind::Other, "Unknown".into())
}

/// 从 env 候选里挑出**可信**的「claude pid → session id」权威配对。
///
/// 病灶：`run_in_background` 派生的后台任务（及其它子进程）会**继承** `CLAUDE_PID` /
/// `CLAUDE_CODE_SESSION_ID`。当拥有它的 claude 退出（用户 `/clear`、会话重启）后，这些子进程
/// 常常**孤儿化并继续存活**，其 env 里仍带着**已死的旧 CLAUDE_PID**。若照单全收，就会把会话
/// 配到一个不存在的 pid（build_tasks 里被丢弃 → 该会话退回 mtime 启发式，串终端），更危险的是
/// Windows 会**重用 PID**：旧 pid 一旦被无关新进程占用，就会把该会话错配过去（= cursor 终端
/// 会话获取错误）。
///
/// 过滤规则（两条都要满足才采信）：
/// 1. `CLAUDE_PID` 必须是**当前存活的 claude 进程**（排除已死孤儿 / 被非 claude 重用的 pid）；
/// 2. 该 `CLAUDE_PID` 必须是上报进程的**祖先**（顺父链上溯能走到）——真正的子孙进程满足，
///    父链已断的孤儿走不到，进一步防 PID 重用后的张冠李戴。
///
/// 同一 claude 的多个子孙给出同一 (pid, session)，去重取其一即可。
fn resolve_session_pins(
    candidates: &[(u32, u32, String)],
    parent_of: &std::collections::HashMap<u32, u32>,
    alive_claude: &std::collections::HashSet<u32>,
) -> std::collections::HashMap<u32, String> {
    let mut out = std::collections::HashMap::new();
    for (reporter, claude_pid, sid) in candidates {
        if !alive_claude.contains(claude_pid) {
            continue; // CLAUDE_PID 已死（孤儿）或被非 claude 进程重用
        }
        if !is_ancestor(*claude_pid, *reporter, parent_of) {
            continue; // 父链走不到该 claude → 不是它的子孙，多半是孤儿/重用
        }
        out.entry(*claude_pid).or_insert_with(|| sid.clone());
    }
    out
}

/// 顺 `parent_of` 父链从 `from` 上溯（最多 24 跳），判断 `ancestor` 是否为其祖先。
fn is_ancestor(ancestor: u32, from: u32, parent_of: &std::collections::HashMap<u32, u32>) -> bool {
    let mut cur = from;
    for _ in 0..24 {
        if cur == ancestor {
            return true;
        }
        match parent_of.get(&cur) {
            Some(&pp) if pp != cur && pp > 1 => cur = pp,
            _ => return false,
        }
    }
    false
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

/// 进程名（已转小写）是不是一个交互式 shell。
///
/// 「父链里有没有 shell」是区分「终端里跑的会话」与「宿主程序直接拉起的代理」的判据，
/// [`ProcessScanner::nearest_shell`] 与 [`ProcessScanner::detect_ide`] 共用这一份名单，
/// 免得两处各写一份、哪天只改了一处。
fn is_shell_name(lower: &str) -> bool {
    matches!(
        lower,
        "powershell.exe"
            | "pwsh.exe"
            | "cmd.exe"
            | "bash.exe"
            | "nu.exe"
            | "wsl.exe"
            | "bash"
            | "zsh"
            | "sh"
            | "fish"
            | "nu"
            | "pwsh"
            | "powershell"
            | "-zsh"
            | "-bash"
    )
}

/// 可执行文件路径若是某个 macOS 应用包的主程序，返回**最外层** `.app` 的名字。
///
/// 判据是包结构本身，不认任何具体应用名：路径里得有 `Contents/MacOS/` 这一段
/// （GUI 应用主程序的固定落点），再取从左数第一个 `.app` 组件。取最外层是因为
/// Electron 应用的子进程住在嵌套包里 —— `/Applications/ChatGPT.app/Contents/
/// Frameworks/…/Codex (Renderer).app/Contents/MacOS/…` 该报「ChatGPT」，
/// 不是「Codex (Renderer)」。
///
/// 非 macOS 的路径拿不到 `.app` + `Contents/MacOS`，一律 None。
fn app_bundle_name(exe: &str) -> Option<&str> {
    if !exe.contains("/Contents/MacOS/") {
        return None;
    }
    exe.split('/')
        .find_map(|c| c.strip_suffix(".app"))
        .filter(|n| !n.is_empty())
}

/// 顺父链找出**托着这个代理进程的 macOS GUI 应用**：`(宿主进程 pid, 应用名)`。
///
/// 判据与 [`classify_chain`] 的桌面客户端分支逐条对齐（跳过链首、遇 shell 即停、
/// 取最外层 `.app`），共用同一个 [`app_bundle_name`] —— 两处判据必须永远一致，
/// 否则会出现「扫描认成桌面会话、注入却找不到宿主」这种自相矛盾。
/// 区别只在这里额外把**宿主的 pid** 带出来：AX 注入要按 pid 打开应用元素。
///
/// 只在 macOS 成立（判据是 `.app` 包结构），其它平台恒返回 None。
pub fn desktop_host(agent_pid: u32) -> Option<(u32, String)> {
    let mut cur = agent_pid;
    // 跳过链首（进程自己）：代理二进制本身也可能装在 .app 里，拿它当宿主就成了自己托自己。
    for _ in 0..16 {
        cur = ppid_via_ps(cur)?;
        if cur <= 1 {
            return None;
        }
        let exe = exe_via_ps(cur)?;
        // 遇到 shell 就停：终端里跑的代理父链一定是 代理 → shell → 终端应用，
        // 而终端应用同样是个 .app，再往上找就会把终端会话认成桌面会话。
        if is_shell_name(&base(&exe).to_lowercase()) {
            return None;
        }
        if let Some(app) = app_bundle_name(&exe) {
            return Some((cur, app.to_string()));
        }
    }
    None
}

/// 进程的可执行文件路径（macOS 的 `ps -o comm=` 给的就是完整路径）
#[cfg(unix)]
fn exe_via_ps(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(unix))]
fn exe_via_ps(_pid: u32) -> Option<String> {
    None
}

/// 路径的基名（同时认 / 与 \，兼顾 Windows）
fn base(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

/// 判断进程属于哪种 AI 编码代理；未来在此扩展新代理（如 gemini 等）
fn agent_kind(name: &str, cmd: &[String]) -> Option<&'static str> {
    // 只认「精确命中」：进程名、可执行文件基名、node 包装脚本的路径分量/基名。
    // 绝不能在整串命令行里 contains 子串 —— MCP 配置路径、扩展目录等参数里
    // 带个 "codex"/"claude" 字样，就会把无关进程识别成代理
    // （用户实际遇到：只开了 Claude Code，列表里却多出两个 codex）。
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

/// 这个代理进程是不是「服务模式」——虽然叫 codex/claude，却不是一个交互式终端会话。
///
/// 实际踩到的：终端里跑着 `codex --yolo`，它会拉起 ChatGPT 桌面版的
/// `…/ChatGPT.app/Contents/Resources/codex app-server --listen stdio://` 作孙进程。
/// 这个孙进程可执行文件基名正好是 `codex`、tty 又是从父进程继承来的，于是既过不了
/// [`agent_kind`] 也过不了 tty 判据，最终变成一条「（会话尚未产生记录）」的占位会话；
/// 更糟的是它的最近 shell 祖先与真会话同一个，号位锚一致 —— 同一个终端号下挂出两条会话。
///
/// 判据只看**第一个子命令**，不在整串命令行里找关键字：`codex "帮我看下 mcp"` 这种
/// 提示词里带同名字样的绝不能被误挡。
fn is_service_mode(agent: &str, cmd: &[String]) -> bool {
    // 未来别的代理有同类服务模式（如 `xxx serve`）在这里加一行即可。
    //
    // `value_opts` 是该代理**带独立取值**的全局选项。必须列全，否则选项的值会被当成
    // 子命令：实测 ChatGPT 桌面版的命令行是
    // `…/codex -c features.code_mode_host=true app-server --analytics-default-enabled`，
    // 不跳过 `-c` 的值就会取到 `features.code_mode_host=true`，服务模式判不出来。
    // 漏列一个只会漏挡、不会误挡，方向是安全的。
    let (subs, value_opts): (&[&str], &[&str]) = match agent {
        "codex" => (
            &["app-server", "mcp", "mcp-server", "proto"],
            &[
                "-c",
                "--config",
                "-m",
                "--model",
                "-p",
                "--profile",
                "-s",
                "--sandbox",
                "-a",
                "--ask-for-approval",
                "-C",
                "--cd",
                "-i",
                "--image",
            ],
        ),
        _ => return false,
    };
    first_subcommand(cmd, value_opts).is_some_and(|s| subs.contains(&s))
}

/// 命令行里的第一个子命令：跳过可执行文件（node 包装再多跳一层脚本路径）与所有选项，
/// 再取第一个裸参数。`--opt=value` 自带值；`--opt value` 形态要连它的值一起跳过，
/// 靠 `value_opts` 认（见 [`is_service_mode`]）。
///
/// 取不到就是 None。不在 `value_opts` 里的取值选项，它的值仍会被当成子命令候选 ——
/// 这只会让 [`is_service_mode`] 漏挡，不会误挡。
fn first_subcommand<'a>(cmd: &'a [String], value_opts: &[&str]) -> Option<&'a str> {
    let skip = match cmd.first().map(|s| base(s)) {
        Some("node") | Some("node.exe") => 2,
        _ => 1,
    };
    let mut rest = cmd.iter().skip(skip);
    while let Some(a) = rest.next() {
        if !a.starts_with('-') {
            return Some(a);
        }
        if !a.contains('=') && value_opts.contains(&a.as_str()) {
            rest.next();
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
            ControlAction::TermKey => return Err(anyhow!("TermKey 动作需走 send_terminal_keys")),
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
            // 「中断当前任务」＝按 Esc，**不是**杀进程。
            //
            // 这里原先和 Stop/Kill 一起走 taskkill：那是南辕北辙 —— 真执行成功，用户的
            // 整个会话就没了，而他要的只是让 claude 停下手上这件事。之所以一直没炸，
            // 是因为不带 /F 的 taskkill 靠给顶层窗口发 WM_CLOSE，而 claude.exe 是终端里
            // 共享 ConPTY 的子进程、没有自己的窗口，于是既杀不掉也没反应 —— 表现成
            // 「点了没用」，反倒把真正的祸事盖住了。
            //
            // Unix 那边仍是 SIGINT，不动：它是那个平台上「中断」的常规做法。
            ControlAction::Interrupt => send_terminal_keys(pid, "esc"),
            ControlAction::Stop | ControlAction::Kill => {
                let force = matches!(action, ControlAction::Kill);
                let mut cmd = std::process::Command::new("taskkill");
                cmd.arg("/PID").arg(pid.to_string());
                if force {
                    cmd.arg("/F");
                }
                let out = cmd
                    .output()
                    .map_err(|e| anyhow!("taskkill 执行失败: {e}"))?;
                if !out.status.success() {
                    return Err(anyhow!(
                        "taskkill 失败: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                Ok(action_label(action))
            }
            ControlAction::Pause | ControlAction::Resume => {
                windows_suspend(pid, matches!(action, ControlAction::Pause))?;
                Ok(action_label(action))
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
    send_input_ex(pid, text, true)
}

/// 同 [`send_input`]，但可选择**不补末尾那个提交回车**。
///
/// `submit = false` 只有一个用途：回答终端的选择卡。那串内容是纯序号按键（"14" = 选项 1
/// 再按「下一题」），最后一下已经把本题提交掉了 —— 再补一个回车就落到翻页后的下一题上，
/// 把它按默认高亮项答掉，表现为「答完第一题，后面的题自己跳掉了」。
pub fn send_input_ex(pid: u32, text: &str, submit: bool) -> Result<&'static str> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(anyhow!("非法 pid: {pid}"));
    }
    #[cfg(unix)]
    {
        let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;

        #[cfg(target_os = "macos")]
        if let Ok(label) = applescript_write(&tty, text, submit) {
            return Ok(label);
        }

        inject_tiocsti(&tty, text, submit)
    }
    #[cfg(windows)]
    {
        windows_send_input(pid, text, submit)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, text, submit);
        Err(anyhow!("当前平台暂不支持远程发布任务"))
    }
}

/// 向终端注入按键（不提交），用于「撤回排队(↑)」「插入排队(Esc)」「多选卡提交(Tab+Enter)」。
/// spec："up:3" = 按 3 次上键；"esc" = 按 1 次 Esc；逗号可串成**有序序列**：
/// "tab:5,enter" = 先按 5 次 Tab 再按一次回车。仅 iTerm2(mac) 与 Windows 控制台
/// 可干净注入；Terminal.app 无法在不切前台的前提下注入方向键 → 返回错误（前端走提示）。
///
/// 序列是多选选择卡唯一的提交途径：Submit 按钮不在选项列表里（数字键索引不到它），
/// 只能 Tab 到最后一项之后再回车。分成两次下发的话，中间隔着队列轮询的几秒，
/// 期间任何一次别的注入插进来都会把焦点带走 —— 必须在一次调用里连着发完。
pub fn send_terminal_keys(pid: u32, spec: &str) -> Result<&'static str> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(anyhow!("非法 pid: {pid}"));
    }
    let steps = parse_key_spec(spec);
    if steps.is_empty() {
        return Ok("无按键");
    }
    // 逐段发。任何一段失败都立刻中止：序列是有序的，
    // 前一段没送到还接着发后面的，只会把焦点留在半路上。
    let mut last = "无按键";
    for (key, count) in steps {
        #[cfg(target_os = "macos")]
        {
            last = mac_send_key(pid, key, count)?;
        }
        #[cfg(windows)]
        {
            last = windows_send_key(pid, key, count)?;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;
            last = tiocsti_send_key(&tty, key, count)?;
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (key, count, &mut last);
            return Err(anyhow!("当前平台不支持按键注入"));
        }
    }
    Ok(last)
}

/// 解析 "up:3" / "esc" / "tab:5,enter" → [(键名, 次数)…]。
///
/// 逗号分隔成有序序列，每段 `键名[:次数]`。单段次数封顶 50 防误触发风暴，
/// 段数同样封顶 —— spec 来自 hub 下发，不该无限长。
fn parse_key_spec(spec: &str) -> Vec<(&str, usize)> {
    spec.split(',')
        .filter_map(|seg| {
            let mut it = seg.splitn(2, ':');
            let key = it.next().unwrap_or("").trim();
            if key.is_empty() {
                return None;
            }
            let count = it
                .next()
                .and_then(|c| c.trim().parse::<usize>().ok())
                .unwrap_or(1);
            // 次数 0 的段直接丢掉，别让它在下游被当成「发一次」
            (count > 0).then(|| (key, count.min(50)))
        })
        .take(8)
        .collect()
}

/// 键名 → 终端转义字节序列。↑ 用普通光标模式 ESC[A；Esc 单字节。
/// Tab/回车是普通控制字符：TUI 把 CR(0x0D) 当「提交」，LF 只会换行，故回车必须用 \r。
#[cfg(all(unix, not(target_os = "macos")))]
fn key_seq(key: &str) -> Option<&'static [u8]> {
    match key {
        "up" => Some(b"\x1b[A"),
        "esc" => Some(b"\x1b"),
        "tab" => Some(b"\t"),
        "enter" => Some(b"\r"),
        _ => None,
    }
}

/// 把按键 spec 展开成可直接写进 pty 的控制字符串，供 IDE 桥接下发。
///
/// Cursor/VSCode 的内嵌终端走 ConPTY，`WriteConsoleInput` 那条路注入不进去
///（扫描到桥接扩展时输入本就改走 `terminal.sendText`）。按键同理 —— 只是
/// 扩展只会「发文本」，所以这里把键名还原成终端本来就认的控制字符。
///
/// **不能出现 `\n`**：扩展见到换行会把整段包进 bracketed paste，届时 TUI 会把
/// ESC/Tab 当成粘贴进来的字面文本而不是按键。回车用 CR 正好避开这一点。
pub fn key_spec_to_chars(spec: &str) -> Option<String> {
    let mut out = String::new();
    for (key, count) in parse_key_spec(spec) {
        let seq = match key {
            "up" => "\x1b[A",
            "esc" => "\x1b",
            "tab" => "\t",
            "enter" => "\r",
            _ => return None,
        };
        for _ in 0..count {
            out.push_str(seq);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// macOS 终端按键注入：先试 iTerm2（write text 转义序列，无需切前台），
/// 匹配不到再试 Terminal.app（System Events key code，需切前台 + 辅助功能权限）。
#[cfg(target_os = "macos")]
fn mac_send_key(pid: u32, key: &str, count: usize) -> Result<&'static str> {
    let tty = tty_of(pid).ok_or_else(|| anyhow!("无法定位进程 {pid} 的终端设备"))?;
    let tty_e = tty.replace('\\', "\\\\").replace('"', "\\\"");

    // 1) iTerm2：write text 直接把转义序列写进会话，不切前台。
    // 回车用 CR(id 13) 而非 newline yes —— 后者有时发的是 LF，TUI 只换行不提交。
    let seq_expr = match key {
        "up" => "(character id 27) & \"[A\"",
        "esc" => "(character id 27)",
        "tab" => "(character id 9)",
        "enter" => "(character id 13)",
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
    if run_osascript(&iterm)
        .map(|o| o.contains("ok"))
        .unwrap_or(false)
    {
        return Ok("已注入按键");
    }

    // 2) Terminal.app：do script 送不了方向键，只能把目标标签页切到前台，再用
    // System Events 发键码（key code 126=↑，53=Esc）。切前台不可避免；且需在
    // 系统设置→隐私与安全性→辅助功能里允许「终端任务监控」，否则 System Events 被拒。
    let keycode = match key {
        "up" => 126,
        "esc" => 53,
        "tab" => 48,
        "enter" => 36, // Return，非小键盘 Enter(76)
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

/// 落盘一段临时 PowerShell 脚本 —— **必须带 UTF-8 BOM**。
///
/// `powershell.exe`（Windows PowerShell 5.1，本仓库调用的就是它）对**没有 BOM** 的 .ps1
/// 一律按系统 ANSI 代码页解码。中文 Windows 上那是 GBK：脚本里的中文注释按 UTF-8 写下去、
/// 按 GBK 读回来，三字节汉字的尾字节落在 GBK 前导字节区间（0x81-0xFE），会把紧随其后的
/// CRLF 当成自己的后继字节吃掉 —— 下一行源码于是被并进上一行注释里，整行代码消失。
///
/// 实测后果：`windows_send_input` 内嵌的 C# 里 `MkCtrlU()` 与 `WriteAll()` 两个方法声明被
/// 注释吞掉，`Add-Type` 编译失败（"c:\…\xxx.0.cs(15) : 类、结构或接口成员声明中的标记
/// "while" 无效"），于是独立 PowerShell 窗口里的会话永远注入不进去。
///
/// 加 BOM 后 PowerShell 按 UTF-8 解码，问题从源头消失；所有生成脚本都走这里，
/// 不留第二条写法。
#[cfg(windows)]
fn write_ps1(path: &std::path::Path, script: &str) -> Result<()> {
    let mut bytes = Vec::with_capacity(script.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(script.as_bytes());
    std::fs::write(path, bytes).map_err(|e| anyhow!("写入临时脚本失败: {e}"))
}

/// Windows：挂起 / 恢复整个进程，充当 Unix 那边 SIGSTOP / SIGCONT 的对应物。
///
/// Windows 没有信号，此前这两个动作直接返回「暂不支持」—— 网页上按钮点了就是没反应。
/// 唯一通用的做法是 ntdll 的 `NtSuspendProcess` / `NtResumeProcess`：它们没有官方文档，
/// 但从 XP 起就在，任务管理器的「挂起」走的也是这条路。
///
/// 只申请 `PROCESS_SUSPEND_RESUME`（0x0800）这一项权限：暂停用不着更大的权柄，
/// 万一句柄泄漏出去也做不了别的。
#[cfg(windows)]
fn windows_suspend(pid: u32, suspend: bool) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let dir = std::env::temp_dir();
    let ps_path = dir.join(format!("am-susp-{pid}-{}.ps1", std::process::id()));
    let script = r#"param([int]$TargetPid,[int]$Suspend)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
public class AmSusp {
  [DllImport("ntdll.dll")] static extern int NtSuspendProcess(IntPtr h);
  [DllImport("ntdll.dll")] static extern int NtResumeProcess(IntPtr h);
  [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint a, bool i, uint p);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool CloseHandle(IntPtr h);
  public static bool Run(uint pid, bool suspend){
    IntPtr h = OpenProcess(0x0800, false, pid);   // PROCESS_SUSPEND_RESUME
    if(h==IntPtr.Zero) return false;
    try { return (suspend ? NtSuspendProcess(h) : NtResumeProcess(h)) == 0; }
    finally { CloseHandle(h); }
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
if([AmSusp]::Run([uint32]$TargetPid,[bool]$Suspend)){ exit 0 } else { exit 2 }
"#;
    write_ps1(&ps_path, script)?;
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
        .arg(pid.to_string())
        .arg(if suspend { "1" } else { "0" })
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = std::fs::remove_file(&ps_path);
    let verb = if suspend { "暂停" } else { "恢复" };
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(_) => Err(anyhow!(
            "{verb}进程 {pid} 失败（可能已退出，或权限不足 —— 目标以更高完整性级别运行时需以管理员身份运行监控端）"
        )),
        Err(e) => Err(anyhow!("{verb}进程失败：powershell 执行失败: {e}")),
    }
}

/// Windows：AttachConsole + WriteConsoleInput 发虚拟键（按下+抬起）count 次。
#[cfg(windows)]
fn windows_send_key(pid: u32, key: &str, count: usize) -> Result<&'static str> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (vk, uch): (u16, u16) = match key {
        "up" => (0x26, 0),     // VK_UP，非可打印字符 → UnicodeChar 0
        "esc" => (0x1B, 27),   // VK_ESCAPE，UnicodeChar = ESC
        "tab" => (0x09, 9),    // VK_TAB，UnicodeChar = HT
        "enter" => (0x0D, 13), // VK_RETURN，UnicodeChar = CR（TUI 认 CR 为提交）
        _ => return Err(anyhow!("未知按键: {key}")),
    };

    // 与 `windows_send_input` 同一道判定：Windows Terminal 的 ConPTY 下 WriteConsoleInput
    // 「可能报错、也可能成功却没送达」，必须改走聚焦发键。
    //
    // 这里长期漏了这一分支，后果是「打断并执行」在 WT 里静默失效：Esc 没送达 → claude
    // 不产生 queue-operation:popAll → 排队列表一直挂着不消失，看着像会话卡死。发任务
    // 那条路径早就有这道判定，只有按键这条没有 —— 两者必须同进同退，日后放宽宿主判定
    // 也要一起改。
    if let Some(wt_pid) = windows_wt_pid(pid) {
        return windows_focus_send_key(wt_pid, vk, count);
    }
    let dir = std::env::temp_dir();
    let ps_path = dir.join(format!("am-key-{pid}-{}.ps1", std::process::id()));
    let script = r#"param([int]$TargetPid,[int]$Vk,[int]$Uch,[int]$Count)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class AmKey {
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool AttachConsole(uint pid);
  [DllImport("kernel32.dll",SetLastError=true)] public static extern bool FreeConsole();
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct KEY_EVENT_RECORD { public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode; public ushort wVirtualScanCode; public char UnicodeChar; public uint dwControlKeyState; }
  [StructLayout(LayoutKind.Explicit)] public struct INPUT_RECORD { [FieldOffset(0)] public ushort EventType; [FieldOffset(4)] public KEY_EVENT_RECORD Key; }
  [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode,EntryPoint="WriteConsoleInputW")] public static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
  static INPUT_RECORD Mk(ushort vk, char uc, bool down){ var r=new INPUT_RECORD(); r.EventType=1; var k=new KEY_EVENT_RECORD(); k.bKeyDown=down?1:0; k.wRepeatCount=1; k.wVirtualKeyCode=vk; k.wVirtualScanCode=0; k.UnicodeChar=uc; k.dwControlKeyState=0; r.Key=k; return r; }
  static bool WriteAll(IntPtr h, List<INPUT_RECORD> recs){
    int i=0; var arr=recs.ToArray();
    while(i<arr.Length){ int n=Math.Min(8, arr.Length-i); var chunk=new INPUT_RECORD[n]; Array.Copy(arr,i,chunk,0,n); uint w; if(!WriteConsoleInput(h, chunk, (uint)n, out w) || w==0) return false; i+=(int)w; }
    return true;
  }
  public static bool Send(uint pid, ushort vk, char uc, int count){
    FreeConsole();
    if(!AttachConsole(pid)) return false;
    try {
      IntPtr h=CreateFileW("CONIN$",0xC0000000u,3u,IntPtr.Zero,3u,0u,IntPtr.Zero);
      if(h==(IntPtr)(-1)) return false;
      var recs=new List<INPUT_RECORD>();
      for(int i=0;i<count;i++){ recs.Add(Mk(vk,uc,true)); recs.Add(Mk(vk,uc,false)); }
      return WriteAll(h, recs);
    } finally { FreeConsole(); }
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
if([AmKey]::Send([uint32]$TargetPid,[uint16]$Vk,[char]$Uch,$Count)){ exit 0 } else { exit 2 }
"#;
    write_ps1(&ps_path, script)?;
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
fn windows_send_input(pid: u32, text: &str, submit: bool) -> Result<&'static str> {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Windows Terminal（ConPTY）：WriteConsoleInput 对伪控制台不可靠（可能报错、也可能
    // 「成功却没送达」），故先判定——父链里有 WindowsTerminal.exe 就直接走聚焦粘贴，不再
    // 尝试 WriteConsoleInput。传统 conhost 控制台父链里没有它，继续走下面的 WriteConsoleInput。
    if let Some(wt_pid) = windows_wt_pid(pid) {
        return windows_paste_send(wt_pid, text, submit);
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
    std::fs::write(&txt_path, payload.as_bytes()).map_err(|e| anyhow!("写入临时文本失败: {e}"))?;

    // 脚本：读文本 → 逐字符写 KEY_EVENT_RECORD → 末尾补一个回车提交
    let script = r#"param([int]$TargetPid,[string]$TextFile,[int]$Submit=1)
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
  // Ctrl+U：0x15 是它的控制字符，0x55 是 VK_U，0x0008 是 LEFT_CTRL_PRESSED。
  // 三者都给全 —— 有的 TUI 读控制字符、有的看虚拟键码 + 修饰位。
  static INPUT_RECORD MkCtrlU(bool down){ var r=new INPUT_RECORD(); r.EventType=1; var k=new KEY_EVENT_RECORD(); k.bKeyDown=down?1:0; k.wRepeatCount=1; k.wVirtualKeyCode=0x55; k.wVirtualScanCode=0; k.UnicodeChar=(char)0x15; k.dwControlKeyState=0x0008; r.Key=k; return r; }
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
  public static bool Send(uint pid, string text, bool submit){
    FreeConsole();
    if(!AttachConsole(pid)) return false;
    try {
      IntPtr h=CreateFileW("CONIN$",0xC0000000u,3u,IntPtr.Zero,3u,0u,IntPtr.Zero);
      if(h==(IntPtr)(-1)) return false;
      var recs=new System.Collections.Generic.List<INPUT_RECORD>();
      // 先清空输入框：Ctrl+U（UnicodeChar=0x15 + VK_U + 左 Ctrl 按下位）。「全部撤回」
      // 会把原文调回终端输入框，不清的话这次注入会直接接在残留后面黏成一句。
      // 输入框本来就空时这一下无副作用。
      recs.Add(MkCtrlU(true)); recs.Add(MkCtrlU(false));
      // 换行必须带 VK_RETURN 才进得去：只给 UnicodeChar 不带虚拟键码时，控制台输入
      // 缓冲会把它丢掉 —— 多行内容于是被连成一行（末尾那个提交用的回车一直是带
      // 0x0D 的，正文里的换行漏了）。CR/LF 都归一成一次回车键事件，\r\n 不重复触发。
      char prev='\0';
      foreach(char c in text){
        if(c=='\r' || c=='\n'){
          if(c=='\n' && prev=='\r'){ prev=c; continue; }  // \r\n 只算一次
          recs.Add(Mk('\r',0x0D,true)); recs.Add(Mk('\r',0x0D,false));
        } else {
          recs.Add(Mk(c,0,true)); recs.Add(Mk(c,0,false));
        }
        prev=c;
      }
      // 末尾提交回车。选择卡的选项作答不补（submit=false）：那串数字的最后一下已经提交了
      // 本题，再来一个回车会落在翻页后的下一题上、把它按默认高亮项答掉。
      if(submit){ recs.Add(Mk('\r',0x0D,true)); recs.Add(Mk('\r',0x0D,false)); }
      return WriteAll(h, recs);
    } finally { FreeConsole(); }
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
$t=[System.IO.File]::ReadAllText($TextFile,[System.Text.Encoding]::UTF8)
if([AmConIn]::Send([uint32]$TargetPid,$t,($Submit -ne 0))){ exit 0 } else { exit 2 }
"#;
    write_ps1(&ps_path, script)?;

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
        .arg("-Submit")
        .arg(if submit { "1" } else { "0" })
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
                Some(wt_pid) => windows_paste_send(wt_pid, text, submit).map_err(|e| {
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
            "powershell.exe"
                | "pwsh.exe"
                | "cmd.exe"
                | "bash.exe"
                | "nu.exe"
                | "wsl.exe"
                | "bash"
                | "zsh"
                | "sh"
                | "fish"
                | "nu"
                | "pwsh"
                | "powershell"
                | "-zsh"
                | "-bash"
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

/// Windows Terminal 回退（按键版）：聚焦 wt_pid 的可见窗口 → keybd_event 连发 count 次该键。
///
/// 与 [`windows_paste_send`] 同源、同局限（抢前台焦点；多标签页只送到当前活动标签）。
/// 按键不经剪贴板，所以比粘贴那条路径更简单。每次按键之间留一点间隔——Esc/↑ 都是给
/// TUI 用的，连发过快时终端可能合并或丢事件。
#[cfg(windows)]
fn windows_focus_send_key(wt_pid: u32, vk: u16, count: usize) -> Result<&'static str> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let dir = std::env::temp_dir();
    let ps_path = dir.join(format!("am-fkey-{wt_pid}-{}.ps1", std::process::id()));
    let script = r#"param([int]$WtPid,[int]$Vk,[int]$Count)
$ErrorActionPreference='Stop'
$code=@'
using System;
using System.Runtime.InteropServices;
public class AmFKey {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  const uint KEYUP=2;
  static IntPtr FindWin(uint target){
    IntPtr found=IntPtr.Zero;
    EnumWindows((h,p)=>{ uint wp; GetWindowThreadProcessId(h,out wp); if(wp==target && IsWindowVisible(h)){ found=h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static bool Run(uint pid, byte vk, int count){
    IntPtr h=FindWin(pid);
    if(h==IntPtr.Zero) return false;
    ShowWindow(h,9); SetForegroundWindow(h);
    System.Threading.Thread.Sleep(180);
    for(int i=0;i<count;i++){
      keybd_event(vk,0,0,IntPtr.Zero); keybd_event(vk,0,KEYUP,IntPtr.Zero);
      System.Threading.Thread.Sleep(45);
    }
    return true;
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
if([AmFKey]::Run([uint32]$WtPid,[byte]$Vk,$Count)){ exit 0 } else { exit 4 }
"#;
    write_ps1(&ps_path, script)?;

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
        .arg("-Vk")
        .arg(vk.to_string())
        .arg("-Count")
        .arg(count.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = std::fs::remove_file(&ps_path);
    match out {
        Ok(o) if o.status.success() => Ok("已注入按键"),
        Ok(o) => Err(anyhow!(
            "按键注入失败（未找到可见的终端窗口）。{}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(anyhow!("powershell 执行失败: {e}")),
    }
}

/// Windows Terminal 回退：把文本放剪贴板 → 聚焦 wt_pid 的可见窗口 → 发 Ctrl+V + 回车。
/// 局限：会抢前台焦点；多标签页时粘到「当前活动标签」，claude 不在活动标签则会送错——
/// 这是已有 WT 标签页对外注入的固有限制（见 CreatePseudoConsole 文档）。
#[cfg(windows)]
fn windows_paste_send(wt_pid: u32, text: &str, submit: bool) -> Result<&'static str> {
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
    let script = r#"param([int]$WtPid,[string]$TextFile,[int]$Submit=1)
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
  const uint KEYUP=2; const byte VK_CTRL=0x11, VK_V=0x56, VK_RET=0x0D, VK_U=0x55;
  static IntPtr FindWin(uint target){
    IntPtr found=IntPtr.Zero;
    EnumWindows((h,p)=>{ uint wp; GetWindowThreadProcessId(h,out wp); if(wp==target && IsWindowVisible(h)){ found=h; return false; } return true; }, IntPtr.Zero);
    return found;
  }
  public static bool Run(uint pid, bool submit){
    IntPtr h=FindWin(pid);
    if(h==IntPtr.Zero) return false;
    ShowWindow(h,9); SetForegroundWindow(h);
    System.Threading.Thread.Sleep(180);
    // 先 Ctrl+U 清空输入框：撤回会把原文调回那里，不清就与这次粘贴的内容黏成一句
    keybd_event(VK_CTRL,0,0,IntPtr.Zero); keybd_event(VK_U,0,0,IntPtr.Zero);
    keybd_event(VK_U,0,KEYUP,IntPtr.Zero); keybd_event(VK_CTRL,0,KEYUP,IntPtr.Zero);
    System.Threading.Thread.Sleep(90);
    keybd_event(VK_CTRL,0,0,IntPtr.Zero); keybd_event(VK_V,0,0,IntPtr.Zero);
    keybd_event(VK_V,0,KEYUP,IntPtr.Zero); keybd_event(VK_CTRL,0,KEYUP,IntPtr.Zero);
    System.Threading.Thread.Sleep(140);
    // 选择卡的选项作答不补这个回车：粘进去的那串数字最后一下已经提交了本题，
    // 再回车就落到翻页后的下一题上、把它按默认高亮项答掉。
    if(submit){ keybd_event(VK_RET,0,0,IntPtr.Zero); keybd_event(VK_RET,0,KEYUP,IntPtr.Zero); }
    return true;
  }
}
'@
Add-Type -TypeDefinition $code -Language CSharp
$t=[System.IO.File]::ReadAllText($TextFile,[System.Text.Encoding]::UTF8)
$old=''
try { $old=Get-Clipboard -Raw } catch {}
Set-Clipboard -Value $t
$ok=[AmPaste]::Run([uint32]$WtPid,($Submit -ne 0))
Start-Sleep -Milliseconds 250
try { if($old -ne $null){ Set-Clipboard -Value $old } } catch {}
if($ok){ exit 0 } else { exit 4 }
"#;
    write_ps1(&ps_path, script)?;

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
        .arg("-Submit")
        .arg(if submit { "1" } else { "0" })
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
fn inject_tiocsti(tty: &str, text: &str, submit: bool) -> Result<&'static str> {
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
    // 先清空输入框：Ctrl+U(0x15) 删到行首。「全部撤回」会把原文调回终端输入框，
    // 不清的话下一条注入就直接接在残留后面，两段文字黏成一句。顺带也挡住了
    // 「人在终端里打了一半」的半截输入。输入框本来就空时这一下无副作用。
    bytes.push(0x15);
    if text.contains('\n') {
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }
    // 提交键必须是回车 CR(\r=0x0D)，不能用换行 LF(\n)：TUI（Claude Code 等）把 CR 当
    // 「提交」、把 LF 当「输入里换一行」。之前推 \n 导致文字进了输入框却只换行、不提交。
    // 选择卡的选项作答不补（submit=false）：见 send_input_ex。
    if submit {
        bytes.push(b'\r');
    }
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
fn applescript_write(tty: &str, text: &str, submit: bool) -> Result<&'static str> {
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
    // 提交段（submit=false 时整段不出现）：选择卡的选项作答那串数字最后一下已经提交了本题，
    // 再补回车就落到翻页后的下一题上、把它按默认高亮项答掉。
    let iterm_submit = if submit {
        format!(
            "          delay {submit_delay:.2}\n\
             \x20         tell s to write text (character id 13) newline no\n\
             \x20         delay 0.35\n\
             \x20         -- 兜底二次回车：若上面的回车被粘贴态吞掉（任务只换行没提交），这一下把它提交；\n\
             \x20         -- 若已提交则此时输入为空，claude 对空回车无动作，安全。\n\
             \x20         tell s to write text (character id 13) newline no\n"
        )
    } else {
        String::new()
    };
    let iterm = format!(
        r#"tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        if (tty of s) is "{tty_e}" then
          tell s to write text "{text_e}" newline no
{iterm_submit}          return "ok"
        end if
      end repeat
    end repeat
  end repeat
end tell
return "notfound""#
    );
    if run_osascript(&iterm)
        .map(|o| o.contains("ok"))
        .unwrap_or(false)
    {
        return Ok("已发送");
    }

    // Terminal.app：do script "..." in <tab> 会键入**并回车**，没有「只键入不提交」的写法。
    //
    // 选项作答（submit=false）在这里做等价变换：**去掉末位那个「提交/下一题」键**，让
    // do script 自带的回车充当它。回车在选择卡里就是「确认当前选中项」，与那个键作用相同
    // —— 0.11.8 的实测可证：发「14」时 4 提交了本题、紧随的回车又确认了下一题的默认项。
    // 于是「15」在这里发成「1」+回车：选中同一个选项、同样提交，也不会多出一个键落到
    // 下一题上。
    //
    // 为什么不像 iTerm2 那样干脆不提交：mac 从 10.12 起默认禁用 TIOCSTI，一旦在这里
    // 返回 Err「回退 TIOCSTI」，那条路根本走不通，选项作答就彻底失效 —— 0.11.9 正是
    // 这么干的，表现为「网页卡片收起了，终端的选择卡还在原地等人手动选」。
    let keys_only = !submit && !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    let term_text_e = if keys_only {
        // 纯 ASCII 数字，按字节切末位是安全的
        esc(&text[..text.len() - 1])
    } else {
        text_e.clone()
    };
    // 兜底二次回车只对「发布任务」有意义（长内容的首个回车可能被粘贴态吞掉）。
    // 选项作答绝不能补：第一个回车已经提交了本题，这一下会落在下一题上替人确认默认项。
    let term_fallback = if keys_only {
        ""
    } else {
        "        delay 0.35\n\
         \x20       -- 兜底二次回车（同 iTerm2）：长/多行内容被粘贴态吞掉回车时补一下提交\n\
         \x20       do script \"\" in t\n"
    };
    let terminal = format!(
        r#"tell application "Terminal"
  repeat with w in windows
    repeat with t in tabs of w
      if (tty of t) is "{tty_e}" then
        do script "{term_text_e}" in t
{term_fallback}        return "ok"
      end if
    end repeat
  end repeat
end tell
return "notfound""#
    );
    if run_osascript(&terminal)
        .map(|o| o.contains("ok"))
        .unwrap_or(false)
    {
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
        Err(anyhow!(
            "osascript: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
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
        assert_eq!(
            agent_kind("claude", &s(&["claude", "--flag"])),
            Some("claude")
        );
        assert_eq!(
            agent_kind("codex.exe", &s(&["C:\\bin\\codex.exe"])),
            Some("codex")
        );
        assert_eq!(
            agent_kind("zsh", &s(&["/usr/local/bin/claude"])),
            Some("claude")
        );
        // npm 全局安装形态：node + 包目录 claude-code
        assert_eq!(
            agent_kind(
                "node",
                &s(&[
                    "node",
                    "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
                ])
            ),
            Some("claude")
        );
        assert_eq!(
            agent_kind("node", &s(&["node", "/opt/bin/codex"])),
            Some("codex")
        );
    }

    /// 用户实际踩过：只开了 Claude Code，列表却多出 codex ——
    /// 业务参数（MCP 配置路径、扩展目录等）里的子串绝不能触发识别
    #[test]
    fn args_substrings_do_not_match() {
        assert_eq!(
            agent_kind(
                "node",
                &s(&[
                    "node",
                    "/x/mcp-server.js",
                    "--config",
                    "/Users/a/.claude/codex mcp.json"
                ])
            ),
            None,
            "第三个参数里的 codex/claude 字样不该命中"
        );
        assert_eq!(
            agent_kind(
                "node",
                &s(&["node", "/app/extensions/vendor-codex-helper/main.js"])
            ),
            None,
            "路径分量是 vendor-codex-helper 而非 codex，不该命中"
        );
        assert_eq!(
            agent_kind("Cursor Helper", &s(&["/Applications/Cursor.app/x"])),
            None
        );
        assert_eq!(
            agent_kind("claude-backup-tool", &s(&["claude-backup-tool"])),
            None
        );
    }
}

#[cfg(test)]
mod service_mode_tests {
    use super::is_service_mode;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// 核心回归：终端里的 `codex --yolo` 会拉起 ChatGPT 桌面版的 app-server 孙进程，
    /// 它继承同一个 tty、最近 shell 祖先也相同 —— 不挡掉就在同一个终端号下多出一条
    /// 空占位会话（用户看到「#21 终端有两个会话」）。
    #[test]
    fn chatgpt_app_server_is_service() {
        assert!(is_service_mode(
            "codex",
            &s(&[
                "/Applications/ChatGPT.app/Contents/Resources/codex",
                "app-server",
                "--listen",
                "stdio://",
            ])
        ));
        assert!(is_service_mode("codex", &s(&["codex", "mcp"])));
        assert!(is_service_mode("codex", &s(&["codex", "proto"])));
        // node 包装形态多一层脚本路径
        assert!(is_service_mode(
            "codex",
            &s(&["node", "/opt/bin/codex", "app-server"])
        ));
    }

    /// 交互式会话一律放行，选项与提示词都不能被当成子命令。
    #[test]
    fn interactive_sessions_pass() {
        assert!(!is_service_mode("codex", &s(&["codex"])));
        assert!(!is_service_mode("codex", &s(&["codex", "--yolo"])));
        assert!(
            !is_service_mode("codex", &s(&["codex", "exec", "跑一下测试"])),
            "exec 是用户自己跑的一次性任务，照常监控"
        );
        assert!(
            !is_service_mode("codex", &s(&["codex", "帮我看下 mcp 配置"])),
            "提示词里带 mcp 字样不该被当成子命令"
        );
        // 其它代理没有服务模式表，一律放行
        assert!(!is_service_mode("claude", &s(&["claude", "mcp", "serve"])));
    }
}

#[cfg(test)]
mod session_pins_tests {
    use super::resolve_session_pins;
    use std::collections::{HashMap, HashSet};

    fn parents(pairs: &[(u32, u32)]) -> HashMap<u32, u32> {
        pairs.iter().copied().collect()
    }
    fn alive(pids: &[u32]) -> HashSet<u32> {
        pids.iter().copied().collect()
    }

    /// 直接子进程报出存活 claude 的会话 → 采信。
    #[test]
    fn accepts_live_child() {
        // 100(claude) → 200(bash 后台任务，reporter)
        let cands = vec![(200, 100, "sess-A".to_string())];
        let got = resolve_session_pins(&cands, &parents(&[(200, 100)]), &alive(&[100]));
        assert_eq!(got.get(&100), Some(&"sess-A".to_string()));
    }

    /// 孙进程也能顺父链走到 claude → 采信。
    #[test]
    fn accepts_live_grandchild() {
        // 100(claude) → 200(bash) → 300(node，reporter)
        let cands = vec![(300, 100, "sess-A".to_string())];
        let got = resolve_session_pins(&cands, &parents(&[(300, 200), (200, 100)]), &alive(&[100]));
        assert_eq!(got.get(&100), Some(&"sess-A".to_string()));
    }

    /// 核心回归：claude 已退出、后台任务孤儿化仍带着已死 CLAUDE_PID → 丢弃，
    /// 不再产生指向不存在 pid 的假配对（旧行为会把 {17548: fc8e9621} 这类塞进去）。
    #[test]
    fn rejects_orphan_with_dead_claude_pid() {
        // 17548(claude) 已死，孤儿 200 仍报它；200 被系统重挂到 1
        let cands = vec![(200, 17548, "fc8e9621".to_string())];
        let got = resolve_session_pins(&cands, &parents(&[(200, 1)]), &alive(&[8968, 42640]));
        assert!(got.is_empty(), "已死 CLAUDE_PID 的孤儿配对必须被丢弃");
    }

    /// PID 重用：旧 pid 被无关的**非 claude** 新进程占用 → 不在 alive_claude 里 → 丢弃。
    #[test]
    fn rejects_reused_pid_by_non_claude() {
        let cands = vec![(200, 17548, "fc8e9621".to_string())];
        // 17548 现在活着，但不是 claude（不在集合里）
        let got = resolve_session_pins(&cands, &parents(&[(200, 17548)]), &alive(&[8968]));
        assert!(got.is_empty());
    }

    /// 存活 claude 但不是上报进程的祖先（父链走不到）→ 丢弃，防止张冠李戴。
    #[test]
    fn rejects_when_not_ancestor() {
        // 100 是存活 claude，但 reporter 200 的父链是 200→300→1，够不到 100
        let cands = vec![(200, 100, "sess-A".to_string())];
        let got = resolve_session_pins(
            &cands,
            &parents(&[(200, 300), (300, 1)]),
            &alive(&[100, 300]),
        );
        assert!(got.is_empty());
    }

    /// 同一 claude 的多个子孙报同一会话 → 去重为一条。
    #[test]
    fn dedups_same_claude() {
        let cands = vec![
            (200, 100, "sess-A".to_string()),
            (201, 100, "sess-A".to_string()),
        ];
        let got = resolve_session_pins(&cands, &parents(&[(200, 100), (201, 100)]), &alive(&[100]));
        assert_eq!(got.len(), 1);
        assert_eq!(got.get(&100), Some(&"sess-A".to_string()));
    }

    /// 多个不同 claude 各自的子进程 → 各配各的。
    #[test]
    fn multiple_distinct_claudes() {
        let cands = vec![
            (200, 100, "sess-A".to_string()),
            (300, 101, "sess-B".to_string()),
        ];
        let got = resolve_session_pins(
            &cands,
            &parents(&[(200, 100), (300, 101)]),
            &alive(&[100, 101]),
        );
        assert_eq!(got.get(&100), Some(&"sess-A".to_string()));
        assert_eq!(got.get(&101), Some(&"sess-B".to_string()));
    }
}

#[cfg(test)]
mod key_spec_tests {
    use super::parse_key_spec;

    /// 单段仍按老样子解析 —— recall/flush 两条既有通路不能被序列化改动带偏。
    #[test]
    fn single_segment_keeps_working() {
        assert_eq!(parse_key_spec("up:3"), vec![("up", 3)]);
        assert_eq!(parse_key_spec("esc"), vec![("esc", 1)]);
        // 次数封顶，防误触发风暴
        assert_eq!(parse_key_spec("up:999"), vec![("up", 50)]);
    }

    /// 多选卡的提交序列：Tab 走到 Submit，再回车。
    ///
    /// 这两下**必须同属一个 spec**：拆成两次下发的话，中间隔着队列轮询的几秒，
    /// 期间任何一次别的注入都会把焦点从 Submit 上带走，回车就落到别处去了。
    #[test]
    fn submit_sequence_is_ordered() {
        assert_eq!(
            parse_key_spec("tab:5,enter"),
            vec![("tab", 5), ("enter", 1)]
        );
        // 顺序即书写顺序，不做任何重排
        assert_eq!(
            parse_key_spec("enter,tab:2"),
            vec![("enter", 1), ("tab", 2)]
        );
    }

    /// 空段与 0 次段一律丢掉：0 次若被当成「发一次」，会凭空多出一下按键，
    /// 在选择卡上就是多勾一项或提前提交。
    #[test]
    fn empty_and_zero_segments_dropped() {
        assert_eq!(parse_key_spec("tab:0,enter"), vec![("enter", 1)]);
        assert_eq!(parse_key_spec(",,tab:2,"), vec![("tab", 2)]);
        assert!(parse_key_spec("").is_empty());
        assert!(parse_key_spec("   ").is_empty());
    }
}

#[cfg(test)]
mod bridge_key_tests {
    use super::key_spec_to_chars;

    /// 按键要能还原成终端本就认的控制字符 —— 内嵌终端（ConPTY）只收得到文本，
    /// 按键得靠这一步经桥接送达。
    #[test]
    fn keys_become_control_chars() {
        assert_eq!(key_spec_to_chars("esc").as_deref(), Some("\x1b"));
        assert_eq!(key_spec_to_chars("up:2").as_deref(), Some("\x1b[A\x1b[A"));
        assert_eq!(
            key_spec_to_chars("tab:3,enter").as_deref(),
            Some("\t\t\t\r")
        );
    }

    /// 回车必须是 CR：换行会让扩展把整段包进 bracketed paste，
    /// 届时 ESC/Tab 会被当成粘贴进来的字面文本，而不是按键。
    #[test]
    fn never_emits_a_newline() {
        for spec in ["enter", "tab:5,enter", "up:50", "esc"] {
            let s = key_spec_to_chars(spec).unwrap();
            assert!(!s.contains('\n'), "{spec} 展开后不能含 \n：{s:?}");
        }
    }

    /// 认不出的键名一律 None —— 宁可退回原生注入，也别把半串按键发出去。
    #[test]
    fn unknown_key_yields_none() {
        assert!(key_spec_to_chars("f5").is_none());
        assert!(key_spec_to_chars("tab:2,f5").is_none());
        assert!(key_spec_to_chars("").is_none());
    }
}

#[cfg(all(test, windows))]
mod ps1_encoding_tests {
    use super::write_ps1;

    /// 临时 .ps1 必须带 UTF-8 BOM。
    ///
    /// 没有 BOM 时 `powershell.exe`（5.1）按系统 ANSI 解码：中文 Windows 上是 GBK，
    /// 汉字的 UTF-8 尾字节会吃掉行尾 CRLF，把下一行源码并进注释——`windows_send_input`
    /// 内嵌的 C# 因此少了两个方法声明、`Add-Type` 编译失败，独立 PowerShell 窗口里的
    /// 会话就永远注入不进去。这条断言把「必须带 BOM」钉死。
    #[test]
    fn ps1_written_with_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("am-ps1-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.ps1");
        let script = "# 中文注释 —— 长\r\nWrite-Output 'ok'\r\n";
        write_ps1(&path, script).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[..3],
            &[0xEF, 0xBB, 0xBF],
            "缺 BOM，PowerShell 5.1 会按 GBK 读"
        );
        assert_eq!(&bytes[3..], script.as_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真机回归：把 `windows_send_input` 那段脚本落盘后交给 powershell 跑一遍，
    /// 编译失败会在 stderr 里出现 `Add-Type`。目标 pid 用 0（AttachConsole 必失败），
    /// 所以只验「C# 编译得过」，不会真的往谁的终端里注入东西。
    #[test]
    fn embedded_csharp_compiles_under_powershell() {
        let err = match super::windows_send_input(0, "x", false) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        };
        assert!(
            !err.contains("Add-Type"),
            "内嵌 C# 没编译过（多半又是脚本编码问题）：{err}"
        );
    }
}

#[cfg(test)]
mod desktop_host_tests {
    use super::{app_bundle_name, classify_chain, is_service_mode};
    use crate::model::IdeKind;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// 应用包判据只看包结构：`Contents/MacOS/` + 最外层 `.app`。
    #[test]
    fn app_bundle_name_takes_outermost_app() {
        assert_eq!(
            app_bundle_name("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
            Some("ChatGPT")
        );
        assert_eq!(
            app_bundle_name("/Applications/Claude.app/Contents/MacOS/Claude"),
            Some("Claude")
        );
        // Electron 子进程住在嵌套包里，报的仍该是外层应用
        assert_eq!(
            app_bundle_name(
                "/Applications/ChatGPT.app/Contents/Frameworks/Codex Framework.framework/Versions/150.0.7871.124/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer)"
            ),
            Some("ChatGPT")
        );
    }

    /// 不是应用包主程序的路径一律不认，免得把普通可执行文件当成桌面客户端。
    #[test]
    fn app_bundle_name_rejects_non_bundles() {
        assert_eq!(app_bundle_name("/Users/x/.local/bin/codex"), None);
        assert_eq!(app_bundle_name("/bin/zsh"), None);
        // 包里的辅助工具不在 Contents/MacOS 下 —— 继续往父链上找就是了
        assert_eq!(
            app_bundle_name("/Applications/Claude.app/Contents/Helpers/disclaimer"),
            None
        );
        assert_eq!(
            app_bundle_name("C:\\Program Files\\ChatGPT\\ChatGPT.exe"),
            None
        );
    }

    /// 回归：ChatGPT 桌面版真实命令行里 `app-server` 前面隔着一个 `-c KEY=VAL`。
    /// 不跳过选项的值就会取到 `features.code_mode_host=true`，服务模式判不出来 ——
    /// 于是这个共享宿主进程会被当成普通会话进程，按 cwd（恒为 `/`）去配对、
    /// 配不上就冒出一张「(会话尚未产生记录)」的空卡。
    #[test]
    fn option_values_are_not_subcommands() {
        assert!(is_service_mode(
            "codex",
            &s(&[
                "/Applications/ChatGPT.app/Contents/Resources/codex",
                "-c",
                "features.code_mode_host=true",
                "app-server",
                "--analytics-default-enabled",
            ])
        ));
        // `--opt=value` 自带值，不能再吞掉下一个参数
        assert!(is_service_mode(
            "codex",
            &s(&["codex", "--config=features.x=true", "app-server"])
        ));
        // 取值选项后面跟的是用户提示词时，提示词不该被当成子命令 —— 方向仍是「宁可漏挡」
        assert!(
            !is_service_mode("codex", &s(&["codex", "-m", "gpt-5", "帮我看下 mcp 配置"])),
            "选项的值和提示词都不是子命令"
        );
    }

    /// 造一条父进程链：链首是进程自己，往后是祖先。`(进程名, argv[0])`
    fn chain(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(n, e)| (n.to_string(), e.to_string()))
            .collect()
    }

    /// 实测链（ChatGPT 桌面版）：`61747 …/ChatGPT.app/Contents/Resources/codex … app-server`
    /// 的父进程就是 `61487 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT`。
    #[test]
    fn chatgpt_desktop_chain_is_desktop() {
        let (kind, name) = classify_chain(&chain(&[
            (
                "codex",
                "/Applications/ChatGPT.app/Contents/Resources/codex",
            ),
            (
                "ChatGPT",
                "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
            ),
        ]));
        assert_eq!(kind, IdeKind::Desktop);
        assert_eq!(name, "ChatGPT");
    }

    /// Claude 桌面版本地代理：代理二进制自己也在一个 `.app` 里，不能拿它当宿主
    /// （否则「自己托自己」，任何装在 .app 里的 CLI 都会被认成桌面会话）。
    #[test]
    fn claude_desktop_chain_reports_outer_app_not_itself() {
        let (kind, name) = classify_chain(&chain(&[
            (
                "claude",
                "/Users/u/Library/Application Support/Claude/claude-code/2.1.260/claude.app/Contents/MacOS/claude",
            ),
            ("Claude", "/Applications/Claude.app/Contents/MacOS/Claude"),
        ]));
        assert_eq!(kind, IdeKind::Desktop);
        assert_eq!(name, "Claude", "报外层宿主应用，不是代理自己那个 .app");
    }

    /// 核心回归：Cursor 的 Codex 插件宿主拉起的常驻进程 —— 父链里有 Cursor，
    /// 判定必须停在 IDE，绝不能落到桌面客户端那条分支。落过去就会绕开 tty 判据，
    /// 用户没开任何 codex 终端却冒出一张「codex 终端」假会话卡。
    #[test]
    fn cursor_plugin_host_is_ide_not_desktop() {
        for exe in [
            "/Applications/Cursor.app/Contents/Resources/app/extensions/codex/bin/codex",
            "/Users/u/.cursor/extensions/openai.codex/bin/codex",
        ] {
            let (kind, name) = classify_chain(&chain(&[
                ("codex", exe),
                (
                    "Cursor Helper (Plugin)",
                    "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Plugin).app/Contents/MacOS/Cursor Helper (Plugin)",
                ),
                ("Cursor", "/Applications/Cursor.app/Contents/MacOS/Cursor"),
            ]));
            assert_eq!(kind, IdeKind::Cursor, "{exe}");
            assert_eq!(name, "Cursor");
        }
    }

    /// 名单外的终端应用（Hyper / Tabby 之流，同样是 `.app`）不能被认成桌面客户端：
    /// 中间那个 shell 就是判据 —— 桌面客户端拉起的代理，父链里没有 shell。
    /// 认错了就等于在这种终端里跑 `codex mcp` 又会冒出假会话卡。
    #[test]
    fn unlisted_terminal_app_is_not_desktop() {
        let (kind, name) = classify_chain(&chain(&[
            ("codex", "/opt/homebrew/bin/codex"),
            ("-zsh", "-zsh"),
            ("Hyper", "/Applications/Hyper.app/Contents/MacOS/Hyper"),
        ]));
        assert_ne!(kind, IdeKind::Desktop, "shell 之上的应用包不是桌面客户端");
        assert_eq!((kind, name.as_str()), (IdeKind::Other, "Unknown"));
    }

    /// 名单内的终端仍走终端分支（.app 判据不能把它抢过去）。
    #[test]
    fn listed_terminal_stays_terminal() {
        let (kind, name) = classify_chain(&chain(&[
            ("codex", "/opt/homebrew/bin/codex"),
            ("-zsh", "-zsh"),
            ("iTerm2", "/Applications/iTerm.app/Contents/MacOS/iTerm2"),
        ]));
        assert_eq!(kind, IdeKind::Terminal);
        assert_eq!(name, "iTerm");
    }
}
