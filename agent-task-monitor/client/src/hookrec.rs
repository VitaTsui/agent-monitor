//! Claude Code hook 上报：让 agent **自己说出**「我是谁」，取代靠启发式猜配对。
//!
//! 病灶回顾：会话（jsonl）与进程（claude.exe）的对应关系一直靠猜 —— 权威 pin 只存在于
//! claude 临时派生的工具子进程 env 里（跑完就没），空闲会话一个都抓不到；抓不到就退回
//! mtime 启发式，同一时刻开的两个终端极易配错，表现为钉钉里标题串号、下发打到别的终端。
//! 为此堆了终端锚、pin 累积表、clear-follow 迁移等一层层补丁。
//!
//! 而 Claude Code 的 hook 在 stdin 里**直接给出** `session_id` 和 `cwd`，环境变量里还有
//! `CLAUDE_PID`。这是一条权威、及时、无需碰运气的配对信号 —— 有它就不必猜。
//!
//! 通道沿用 bridge 的思路：不引入本地服务，纯文件。hook 进程把一条记录写进
//! `<data_dir>/hooks/<pid>.json`，扫描循环读取即可。
//!
//! 用法（配在 `~/.claude/settings.json`）：
//! ```json
//! { "hooks": { "SessionStart": [ { "hooks": [
//!     { "type": "command", "command": "<客户端可执行文件> hook" } ] } ] } }
//! ```
//! `PreToolUse` 挂同样的命令也可以（更频繁、更保险）。hook 必须**极快且绝不打断 claude**，
//! 所以这里只做「读 stdin → 写一个小文件」，任何异常都静默退出 0。

use std::io::Read;
use std::path::{Path, PathBuf};

/// hook 记录目录
pub fn hooks_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("hooks")
}

/// 一条 hook 上报：某个 claude 进程自报的会话身份。
///
/// 只暴露配对真正要用的两个字段。cwd 与写入时刻也在落盘的 JSON 里（前者便于排查、后者用于
/// TTL 判定），但都在 [`read_reports`] 内部消化，不必带出来让调用方多拿两个用不上的值。
#[derive(Debug, Clone)]
pub struct HookReport {
    pub claude_pid: u32,
    pub session_id: String,
    /// 这条记录的落盘时刻（epoch 毫秒）。
    ///
    /// 「有没有作答」不看它，看 [`Self::select_at_ms`] —— 这条记录可能只是某个无关工具
    /// 触发的 hook，它的落盘时刻与那张卡什么时候弹出来没有关系。
    /// （TTL 用的秒级 `at` 在 [`read_reports`] 内部消化，不必带出来。）
    #[allow(dead_code)]
    pub at_ms: u64,
    /// 终端**此刻正等着你选**：AskUserQuestion 的整份 input（questions/options）。
    ///
    /// 从 jsonl 里读到的 select 消息是「事后」的 —— 那条记录要等这一轮落盘才看得见，
    /// 人在终端上选完了远端才亮出选项，等于没用。而 PreToolUse 在工具**执行前**触发，
    /// 拿到的就是即将弹给用户的那些选项，这才是「远程替终端做决定」需要的时机。
    pub pending_select: Option<serde_json::Value>,
    /// 这张待选卡**弹出**的时刻（epoch 毫秒），不是本条记录的落盘时刻。
    ///
    /// 与 jsonl 里那次 AskUserQuestion 的 tool_result 时刻比大小，就知道它有没有被答过。
    /// 必须是「弹出时刻」而不是「落盘时刻」：卡片会被后续无关的 hook 事件一路继承下去，
    /// 每继承一次就重盖一次落盘时刻的话，「作答时刻 ≥ 弹出时刻」永远不可能成立，
    /// 答完的卡片就永久挂在界面上（见 `record` 里的说明）。
    pub select_at_ms: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `am-client hook` 子命令入口：从 stdin 读 hook JSON，落一条记录。
///
/// **绝不失败**：hook 是同步阻塞 claude 的，报错或挂起都会拖累用户的会话。
/// 任何异常都静默返回，宁可这次没记上，也不能影响 claude。
pub fn run_hook_cli(data_dir: &Path) {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return;
    }
    let v: serde_json::Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => return,
    };
    let session_id = v.get("session_id").and_then(|x| x.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let cwd = v.get("cwd").and_then(|x| x.as_str()).unwrap_or("");
    // CLAUDE_PID 由 claude 注入到子进程环境；hook 进程正是它的子进程，所以拿得到。
    // 万一没有就退回父进程 pid（hook 的父进程即 claude 本身）。
    let claude_pid = std::env::var("CLAUDE_PID")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .or_else(parent_pid)
        .unwrap_or(0);
    if claude_pid == 0 {
        return;
    }
    let dir = hooks_dir(data_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // 待选项：AskUserQuestion 即将执行时记下，作答后清掉，**其余事件一律原样继承**。
    //
    // 早先是「除了 AskUserQuestion 的 PreToolUse，一律写 None」，靠覆盖来清除。那个设想
    // 漏了一件事：claude 可以**并行**发起多个工具调用。AskUserQuestion 正阻塞等着作答时，
    // 同一批次里另一个工具的 PreToolUse 一触发，待选状态就被冲掉了 —— 卡片还挂在终端上
    // 等你，远端却再也看不到它。线上抓到过：15:26:45 调用 AskUserQuestion（jsonl 里没有
    // tool_result，即从未作答），15:32:53 记录已被改写成 null。
    //
    // 所以清除只认一个明确信号：PostToolUse(AskUserQuestion) —— 那才是「答完了」。
    let event = v
        .get("hook_event_name")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let tool = v.get("tool_name").and_then(|x| x.as_str()).unwrap_or("");
    //
    // **继承时连它的弹出时刻一起继承**，不要重新盖上「此刻」的时间戳。
    // 那个时间戳是「这张卡是什么时候弹出来的」，唯一的用途是与 jsonl 里那次
    // AskUserQuestion 的 tool_result 时刻比大小，判断它有没有被答过
    // （见 client::state 的回填处）。而 PostToolUse(AskUserQuestion) 这条 hook 缺席的
    // 情形不少（旧版没写这条配置、拿不到 CLAUDE_PID、用户按 Esc 打断），
    // 那时全靠这个比较把答完的卡撤下来。
    //
    // 早先继承只带过 pending_select、时间戳照写「此刻」：人在终端答完之后，会话继续跑，
    // 随便哪个别的工具触发一次 hook，这张陈旧卡片的时刻就被重新盖成「现在」——
    // 于是「作答时刻 ≥ 弹出时刻」这个判据**永远再也不可能成立**，卡片就此永久挂在界面上。
    // 实测会话 9168ec90：4 次 AskUserQuestion 全部有配对的 tool_result（即全答过了），
    // 界面上那张卡却还在。
    let (pending_select, select_at_ms) = match (event, tool) {
        ("PreToolUse", "AskUserQuestion") => (v.get("tool_input").cloned(), now_ms()),
        ("PostToolUse", "AskUserQuestion") => (None, 0),
        // 与选择卡无关的事件：把上一条记录里的待选**连同它的弹出时刻**原样带过来
        _ => prev_pending(&dir, claude_pid),
    };
    let rec = serde_json::json!({
        "claude_pid": claude_pid,
        "session_id": session_id,
        "cwd": cwd,
        "at": now_secs(),
        "at_ms": now_ms(),
        "pending_select": pending_select,
        "select_at_ms": select_at_ms,
    });
    let Ok(txt) = serde_json::to_string(&rec) else {
        return;
    };
    // 原子写：扫描循环随时可能在读，半个文件会解析失败
    let tmp = dir.join(format!("{claude_pid}.json.tmp"));
    if std::fs::write(&tmp, txt).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{claude_pid}.json")));
    }
}

/// 读上一条记录里的待选状态**与它的弹出时刻**，供与选择卡无关的 hook 事件原样继承。
///
/// 时刻必须一起继承：它是「这张卡什么时候弹出来的」，重新盖成「此刻」会让
/// 「已作答」的判据永远失效（见上面写记录处的说明）。
/// 旧客户端写的记录没有 `select_at_ms`，此时**弹出时刻按「未知」（0）算**，而不是退回
/// 那条记录的落盘时刻 —— 后者已经被旧逻辑一路重盖成「此刻」，是个错得离谱的晚值，
/// 继承过来只会把卡片继续钉在界面上。
///
/// 「未知」是安全的：撤卡的判据是「jsonl 里那次 AskUserQuestion 已经有 tool_result」，
/// 而这个事实只在**真的答过**时才成立 —— 卡片还开着时 `select_answered_ms` 是 None
/// （见 scanner 的 `open_ask`），拿不到就不会撤。于是升级之后，残留的旧卡当轮即撤，
/// 正开着的卡照常显示。
///
/// hook 必须极快，这里只读一个几百字节的小文件、任何异常都当「没有」——
/// 丢一次待选顶多是远端少显示一张卡，而拖慢 hook 会直接卡住用户的会话。
fn prev_pending(dir: &Path, claude_pid: u32) -> (Option<serde_json::Value>, u64) {
    let Ok(txt) = std::fs::read_to_string(dir.join(format!("{claude_pid}.json"))) else {
        return (None, 0);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return (None, 0);
    };
    let sel = v.get("pending_select").filter(|x| !x.is_null()).cloned();
    if sel.is_none() {
        return (None, 0);
    }
    let at = v.get("select_at_ms").and_then(|x| x.as_u64()).unwrap_or(0);
    (sel, at)
}

/// 取当前进程的父进程 pid（CLAUDE_PID 缺失时的兜底）
#[cfg(windows)]
fn parent_pid() -> Option<u32> {
    // Windows 上没有现成的轻量 API，交给 sysinfo 又太重（hook 要求极快）。
    // 缺 CLAUDE_PID 时直接放弃这条记录 —— 有 PreToolUse 的话下一次工具调用还会再报。
    None
}

#[cfg(not(windows))]
fn parent_pid() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

/// 读取全部 hook 上报（供扫描循环用），顺带清理过期记录。
///
/// `max_age_secs`：超过这个时长的记录视为陈旧 —— 进程多半已退出，留着会让死 pid 一直占着
/// 一条权威配对。取值应显著大于 claude 的空闲时长，否则长时间不动的会话会失去 hook 信号
/// （不过那时还有 pin 累积表和启发式兜底，不至于配不上）。
pub fn read_reports(data_dir: &Path, max_age_secs: u64) -> Vec<HookReport> {
    let dir = hooks_dir(data_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let now = now_secs();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
            let _ = std::fs::remove_file(&path); // 坏文件直接删，免得每轮都解析失败
            continue;
        };
        let at = v.get("at").and_then(|x| x.as_u64()).unwrap_or(0);
        if now.saturating_sub(at) > max_age_secs {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let (Some(pid), Some(sid)) = (
            v.get("claude_pid").and_then(|x| x.as_u64()),
            v.get("session_id").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        out.push(HookReport {
            claude_pid: pid as u32,
            session_id: sid.to_string(),
            // 旧客户端写的记录没有 at_ms，退回秒精度（比不上就当没答，卡片多留一轮）
            at_ms: v
                .get("at_ms")
                .and_then(|x| x.as_u64())
                .unwrap_or(at.saturating_mul(1000)),
            pending_select: v.get("pending_select").filter(|x| !x.is_null()).cloned(),
            // 这张卡**弹出**的时刻。缺失（旧客户端写的记录）按「未知」算 0 ——
            // 见 prev_pending 的说明：残留的旧卡当轮即撤，正开着的卡照常显示。
            select_at_ms: v.get("select_at_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        });
    }
    out
}

/// 删掉某个 pid 的 hook 记录（进程已确认退出时调用，避免 pid 重用后张冠李戴）
pub fn drop_report(data_dir: &Path, claude_pid: u32) {
    let _ = std::fs::remove_file(hooks_dir(data_dir).join(format!("{claude_pid}.json")));
}

// ---------- 自动写入 Claude Code 的 hook 配置 ----------

/// 配置版本：改了写入内容就加一，客户端会重写一次（同 BRIDGE_EXT_VERSION 的套路）
///
/// 2：加挂 PostToolUse(AskUserQuestion)，用于清除「终端正等你选」状态
const HOOK_CONFIG_VERSION: &str = "2";

/// 标记本条目由 agent-monitor 写入 —— 用它识别自己的旧条目并替换，
/// 绝不碰用户自己配的其它 hook。
const MARK: &str = "agent-monitor:pairing";

/// 确保 `~/.claude/settings.json` 里有本客户端的配对 hook。
///
/// 不这么做的话每个用户都得手工写一遍 JSON，而写错**完全静默**（尤其 Windows 路径的反斜杠会
/// 被 bash 当转义符吞掉，hook 找不到命令既不报错也不阻断会话，只是配对一直没生效）。
///
/// 与桥接扩展同一套路：版本标记只写一次；出任何问题都静默跳过 —— 配置文件是用户的，
/// 宁可这次没配上，也绝不能弄坏它或让客户端起不来。
pub fn ensure_hook_config(data_dir: &Path, exe: &Path, force: bool) -> bool {
    let marker = data_dir.join(format!("hook-config-{HOOK_CONFIG_VERSION}.done"));
    if !force && marker.exists() {
        return false;
    }
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let path = home.join(".claude").join("settings.json");
    // Claude Code 没装/没跑过就没有这个文件，此时不该替它创建目录结构
    let Ok(txt) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return false;
    };
    if !root.is_object() {
        return false;
    }

    // **正斜杠**：hook 命令交给 bash 执行，Windows 路径里的反斜杠会被当成转义符吃掉
    // （D:\A\B.exe → D:AB.exe，找不到且静默失败）。正斜杠在 Windows 上照样能调起程序。
    let cmd = format!("{} hook", exe.to_string_lossy().replace('\\', "/"));

    let changed = apply_hook_config(&mut root, &cmd);

    if !changed && !force {
        let _ = std::fs::write(&marker, HOOK_CONFIG_VERSION);
        return false;
    }
    // 备份 + 原子写：这是用户的配置文件，改坏了他会丢掉自己所有的 hook/权限设置
    let bak = path.with_extension("json.am-bak");
    let _ = std::fs::copy(&path, &bak);
    let Ok(out) = serde_json::to_string_pretty(&root) else {
        return false;
    };
    let tmp = path.with_extension("json.am-tmp");
    if std::fs::write(&tmp, &out).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    let _ = std::fs::write(&marker, HOOK_CONFIG_VERSION);
    true
}

/// 把配对 hook 合并进 settings 的 JSON 树，返回是否有改动。
///
/// 纯函数（不碰文件），逻辑要点：
/// - 只替换**自己**写过的条目（认 `_source` 标记），用户手写的 hook 一概不动；
/// - 客户端换安装位置后命令会变，所以先摘旧条目再加新的，而不是简单去重；
/// - 用户已手工配了等价命令时不重复添加。
pub(crate) fn apply_hook_config(root: &mut serde_json::Value, cmd: &str) -> bool {
    let Some(obj) = root.as_object_mut() else {
        return false;
    };
    let Some(hooks) = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return false;
    };

    let mut changed = false;
    // SessionStart：会话一开就报身份，不必等它执行工具
    // PreToolUse：兜底，万一 SessionStart 那次没写成，下次调工具补上；
    //             同时它是「终端正等你选」的实时信号来源（见 run_hook_cli）
    // PostToolUse：只盯 AskUserQuestion —— 用户选完后若 claude 直接收尾、
    //             不再调任何工具，就没有下一次 PreToolUse 来覆盖掉待选状态，
    //             远端会一直挂着一张已经作废的选项卡。这条专门来清它。
    for (event, matcher) in [
        ("SessionStart", None),
        ("PreToolUse", Some("*")),
        ("PostToolUse", Some("AskUserQuestion")),
    ] {
        let entry = match matcher {
            Some(m) => serde_json::json!({
                "matcher": m,
                "hooks": [{ "type": "command", "command": cmd, "_source": MARK }],
            }),
            None => serde_json::json!({
                "hooks": [{ "type": "command", "command": cmd, "_source": MARK }],
            }),
        };
        let list = hooks.entry(event).or_insert_with(|| serde_json::json!([]));
        let Some(arr) = list.as_array_mut() else {
            continue;
        };
        // 已经是我们写的、且命令一致 → 什么都不用做。
        // **必须先判这个**：若先摘旧条目再判重，稳态下每次都会「摘掉又加回」，
        // 于是每次客户端启动都改写一遍用户的配置文件（实测被幂等性测试抓到）。
        if arr.iter().any(|e| is_ours(e) && has_cmd(e, cmd)) {
            continue;
        }
        // 摘掉自己写的旧条目（命令变了，比如客户端换了安装位置），保留用户其它条目
        let before = arr.len();
        arr.retain(|e| !is_ours(e));
        let removed = arr.len() != before;
        // 用户已手工配了等价命令就不重复加
        let dup = arr.iter().any(|e| has_cmd(e, cmd));
        if !dup {
            arr.push(entry);
            changed = true;
        } else if removed {
            changed = true;
        }
    }
    changed
}

/// 这条 hook 条目里是否含指定命令
fn has_cmd(entry: &serde_json::Value, cmd: &str) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hs| {
            hs.iter()
                .any(|h| h.get("command").and_then(|c| c.as_str()) == Some(cmd))
        })
        .unwrap_or(false)
}

/// 这条 hook 条目是不是我们写的（认 `_source` 标记）
fn is_ours(entry: &serde_json::Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hs| {
            hs.iter()
                .any(|h| h.get("_source").and_then(|s| s.as_str()) == Some(MARK))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_own_entries() {
        let ours = serde_json::json!({
            "hooks": [{ "type": "command", "command": "x hook", "_source": MARK }]
        });
        let theirs = serde_json::json!({
            "hooks": [{ "type": "command", "command": "my-own-script.sh" }]
        });
        assert!(is_ours(&ours));
        assert!(!is_ours(&theirs), "绝不能把用户自己配的 hook 认成我们的");
    }

    /// Windows 路径必须转成正斜杠 —— 反斜杠会被 bash 当转义符吃掉
    #[test]
    fn windows_path_uses_forward_slashes() {
        let p = std::path::PathBuf::from(r"D:\AgentMonitor\AgentMonitor.exe");
        let cmd = format!("{} hook", p.to_string_lossy().replace('\\', "/"));
        assert_eq!(cmd, "D:/AgentMonitor/AgentMonitor.exe hook");
        assert!(!cmd.contains('\\'));
    }

    /// **最关键的一条**：用户自己配的 hook 一根汗毛都不能动。
    /// 这个文件是用户的，弄坏了他会丢掉全部 hook / 权限设置。
    #[test]
    fn never_touches_user_hooks() {
        let mut root = serde_json::json!({
            "model": "opus",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "my-stop-hook.sh" }] }],
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "audit.sh" }] }
                ]
            }
        });
        assert!(apply_hook_config(&mut root, "C:/am/am.exe hook"));

        // 用户的 Stop 原样保留
        assert_eq!(
            root["hooks"]["Stop"][0]["hooks"][0]["command"],
            "my-stop-hook.sh"
        );
        // 用户的 PreToolUse 条目还在，我们的追加在后面
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["hooks"][0]["command"], "audit.sh");
        assert_eq!(pre[1]["hooks"][0]["command"], "C:/am/am.exe hook");
        // 其它设置不受影响
        assert_eq!(root["model"], "opus");
    }

    /// 客户端换了安装位置：应替换掉自己的旧条目，而不是堆两条
    #[test]
    fn replaces_own_stale_entry() {
        let mut root = serde_json::json!({ "hooks": {} });
        apply_hook_config(&mut root, "D:/OLD/am.exe hook");
        apply_hook_config(&mut root, "D:/NEW/am.exe hook");
        let ss = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 1, "旧条目该被替换而非累积：{ss:?}");
        assert_eq!(ss[0]["hooks"][0]["command"], "D:/NEW/am.exe hook");
    }

    /// 重复调用不产生变化（幂等），避免每次启动都改写用户配置
    #[test]
    fn is_idempotent() {
        let mut root = serde_json::json!({ "hooks": {} });
        assert!(apply_hook_config(&mut root, "C:/am/am.exe hook"));
        let after_first = root.clone();
        let changed = apply_hook_config(&mut root, "C:/am/am.exe hook");
        assert!(!changed, "第二次不该报告有改动");
        assert_eq!(root, after_first, "内容也不该变");
    }

    /// settings.json 里原本没有 hooks 字段时也要能建起来
    #[test]
    fn creates_hooks_when_absent() {
        let mut root = serde_json::json!({ "model": "opus" });
        assert!(apply_hook_config(&mut root, "C:/am/am.exe hook"));
        assert!(root["hooks"]["SessionStart"].is_array());
        assert!(root["hooks"]["PreToolUse"].is_array());
        assert_eq!(root["hooks"]["PreToolUse"][0]["matcher"], "*");
    }

    /// **答完的选项卡必须撤得下来**：卡片被后续无关的 hook 事件一路继承时，
    /// 「弹出时刻」不能跟着重盖成「此刻」——否则「作答时刻 ≥ 弹出时刻」这个判据
    /// 永远不可能成立，卡片就永久挂在界面上（实测会话 9168ec90：4 次
    /// AskUserQuestion 全都有配对的 tool_result，界面上那张卡却还在）。
    #[test]
    fn inherited_card_keeps_its_original_popup_time() {
        let dir = std::env::temp_dir().join(format!("am-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pid = 4242u32;
        let card = serde_json::json!({ "questions": [{ "question": "选哪个?" }] });
        let popup = 1_000_000u64;

        // ① 卡片弹出：PreToolUse(AskUserQuestion) 记下它与弹出时刻
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::json!({
                "claude_pid": pid, "session_id": "s1", "cwd": "/p",
                "at": popup / 1000, "at_ms": popup,
                "pending_select": card, "select_at_ms": popup,
            })
            .to_string(),
        )
        .unwrap();

        // ② 人在终端答完了，会话继续跑：别的工具触发 hook，卡片被原样继承
        let (inherited, at) = prev_pending(&dir, pid);
        assert_eq!(inherited.as_ref(), Some(&card), "待选要继承下来");
        assert_eq!(at, popup, "弹出时刻必须一起继承，不能盖成「此刻」");

        // ③ 旧客户端写的记录没有 select_at_ms → 弹出时刻按「未知」(0) 算。
        // 那条记录的 at_ms 早被旧逻辑一路重盖成「此刻」，继承过来只会把卡继续钉住；
        // 给 0 则「已答过」的判据当轮成立，残留卡片立刻撤下（正开着的卡因为
        // select_answered_ms 为 None，照样不受影响）。
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::json!({
                "claude_pid": pid, "session_id": "s1", "cwd": "/p",
                "at": popup / 1000, "at_ms": popup, "pending_select": card,
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            prev_pending(&dir, pid).1,
            0,
            "缺字段时按未知算，不退回 at_ms"
        );

        // ④ 没有待选时不该凭空造出一个时刻
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::json!({
                "claude_pid": pid, "session_id": "s1", "cwd": "/p",
                "at": popup / 1000, "at_ms": popup, "pending_select": serde_json::Value::Null,
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(prev_pending(&dir, pid), (None, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
