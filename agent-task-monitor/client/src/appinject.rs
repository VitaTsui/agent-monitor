//! 桌面客户端（Claude.app / ChatGPT.app）撰写框注入 —— **默认关闭的实验特性**。
//!
//! 与「终端注入」「Cursor/VSCode 扩展桥接」并列，是第三种注入目标，不是叠在它们之上的
//! 一层新逻辑：`agent::execute` 按会话宿主类型选一条送达路径，选中这条就走完这条。
//!
//! # 开关
//! 环境变量 `AM_DESKTOP_INJECT=1`（沿用本项目既有的配置机制：env 优先，
//! 缺省时由 `main::load_config_file` 从 `~/.agent-monitor/config.txt` 之类的
//! `KEY=VALUE` 文件补齐）。**没开就一行都不执行**，`enabled()` 是唯一入口的前置闸门。
//!
//! # 为什么用 Accessibility API，不用 AppleScript
//! 同一件事 AppleScript 也能做（`System Events` 的 `entire contents`），但实测
//! 每读一个元素属性就是一次 Apple Event 往返 ≈15ms：Claude.app 首页 343 个元素要
//! 5.3 秒，会话里元素更多。原生 AX 直接问目标进程，同一棵树 68ms，差 78 倍。
//! 顺带还避开了把用户正文拼进 AppleScript 字符串的转义雷区。
//!
//! # 会话身份：怎么确认屏幕上开着的就是要发的那条
//! 撰写框本身认不出自己属于哪条会话，但**窗口的 URL 认得出**。实测（macOS 26.0、
//! Claude.app 2026-09）Claude.app 的 `AXWebArea` 上有 `AXURL`，值就是 claude.ai 的
//! 当前路由，例如 `https://claude.ai/chat/9b5147d7-…`；本地代理（Cowork）会话的路由是
//! `/cowork/local_<uuid>`，那个 `local_<uuid>` 正是会话隔离家目录的名字，磁盘上拿得到
//! （见 `am_core::scanner::claude_local_agent_session_id`）。两边同一个值，比得起来。
//!
//! 判据只有一条：**目标会话 id 出现在某个 web 区域 URL 的某一段路径里**就算命中，
//! 否则拒绝。这也是上游自己的判法（app.asar 里 `getURL().includes(sessionId)`）。
//! 它不认任何产品文案，也不可能误命中——另一条会话的 URL 里不会出现这条会话的 uuid。
//!
//! - **ChatGPT.app 认不出来**。实测它的 `AXWebArea.AXURL` 恒为 `app://-/index.html`、
//!   `AXTitle` 恒为 `Codex`，整棵树里没有一个带任务 id 的 `AXDOMIdentifier`
//!   （只有 `radix-_r_2_` 这类自动生成的），唯一能读到的只有标题文字。标题会重名、
//!   也不是稳定主键，拿它放行等于赌。所以这个宿主上仍旧沿用老规矩：多于一条会话
//!   就拒绝（见 [`SessionRef::Unidentified`] 与 `agent::execute`）。
//!
//! - **只看一个窗口**。`target_window` 取的是宿主的焦点/主窗口，目标会话要是被拖到
//!   另一个窗口里，这里读到的 URL 就不是它 —— 结果是**拒绝**，不是发错人，方向是安全的。
//!   本机实测 Claude.app 一直只有一个窗口，所以先不为此改遍历顺序。
//!
//! # 其它已知边界（实测，不是猜的）
//! - **写入前必须先 `AXFocused = true`**。不设焦点时 `AXUIElementSetAttributeValue`
//!   照样返回 0（成功），值却纹丝不动 —— 静默失败。设焦点不抢系统前台焦点
//!   （实测写入前后 frontmost 应用不变）。
//! - **后台读是「滞后一拍」的**：App 在后台时 Chromium 把渲染进程的可访问性树序列化
//!   节流掉了，纯读永远唤不醒它 —— 实测连读 20 次、跨 4 秒，拿回来的一直是**上一次**
//!   写入的值。解法不是多读几次，而是**再发一次会改状态的 AX 请求**（这里重设一次
//!   `AXFocused`）把渲染进程踢醒，之后 200ms 内就能读到真值。
#![allow(clippy::result_large_err)]

/// 实验开关：`AM_DESKTOP_INJECT=1` 才启用。默认关闭 —— 未开启时调用方不会走到这里，
/// 这条路径一行代码都不执行。
pub fn enabled() -> bool {
    std::env::var("AM_DESKTOP_INJECT")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

/// 注入结果：说清到底做到了哪一步，调用方据此写日志/提示，不许含糊成「成功」。
///
/// 非 macOS 上这条路径整个不存在，两个变体都构造不出来，`-D warnings` 会判它们死。
/// 不是真的多余，所以按平台放行，而不是把类型删了让调用方那边散着写字符串。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injected {
    /// 文本已写进撰写框并回读校验通过，且已按下发送键、撰写框已清空。
    Submitted,
    /// 文本已写进撰写框并回读校验通过，但没提交（调用方要求不提交，或没找到发送键）。
    /// `why` 说明为什么没提交。
    Written { why: &'static str },
}

/// 要注入的那条会话的身份 —— 注入前拿它跟「窗口里此刻开着的会话」比对。
///
/// 这不是给日志看的标签，是一道闸门：认得出身份就比对身份，认不出就退回
/// 「宿主上只能有一条会话」的老规矩。两条路互斥，不并存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRef {
    /// Claude 桌面版本地代理（Cowork）会话，id 形如 `local_<uuid>`。
    /// 它会出现在 webview 的 URL 路径里，可以精确比对。
    ClaudeLocalAgent(String),
    /// 宿主 App 不暴露当前会话身份（实测 ChatGPT.app 就是这样）。
    /// 调用方必须先确认该宿主上只有这一条会话才允许走到这里。
    Unidentified,
}

/// 注入失败的原因。分类是为了让调用方能区分「该降级」与「该告诉用户别再试」。
///
/// 同 [`Injected`]：非 macOS 上只构造得出 `Other`，其余变体按平台放行。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone)]
pub enum InjectError {
    /// 没拿到辅助功能授权（AXIsProcessTrusted == false）。用户要去系统设置里勾。
    NotTrusted,
    /// 树里找不到撰写框。`diag` 里写清遍历了多少节点、找到了哪些文本域、期望什么。
    ComposerNotFound { diag: String },
    /// 撰写框里有未发送的内容。**绝不覆盖**，直接拒绝。
    DraftPresent { preview: String },
    /// 窗口里开着的不是要发的那条会话。发错会话比发不出去严重得多，所以直接拒绝，
    /// 并把「屏幕上是哪条」如实告诉用户，让他自己切过去再重发。
    WrongSession { want: String, on_screen: String },
    /// 写进去了但回读对不上（值不一致）。如实上报，不重试到「看起来成功」为止。
    /// `hint` 是能判定出来的具体成因（判不出来就是空串）。
    VerifyFailed {
        wrote: usize,
        read_back: String,
        hint: &'static str,
    },
    /// 其它（没窗口、AX 调用报错等）。
    Other(String),
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotTrusted => write!(
                f,
                "没有辅助功能权限：到「系统设置 → 隐私与安全性 → 辅助功能」里勾上本应用"
            ),
            Self::ComposerNotFound { diag } => write!(f, "没定位到撰写框（{diag}）"),
            Self::DraftPresent { preview } => {
                write!(f, "撰写器里有未发送内容（开头「{preview}」），已拒绝注入")
            }
            Self::WrongSession { want, on_screen } => write!(
                f,
                "窗口里开着的不是这条会话（屏幕上是「{on_screen}」，要发给 {want}），\
                 已拒绝注入；把那条会话切到前台再重发"
            ),
            Self::VerifyFailed {
                wrote,
                read_back,
                hint,
            } => write!(
                f,
                "写入后回读对不上：写了 {wrote} 字符，读回「{}」{}",
                truncate(read_back, 40),
                if hint.is_empty() {
                    String::new()
                } else {
                    format!("——{hint}")
                }
            ),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

/// 把一条结果提示送到用户眼前。
///
/// 客户端是托盘应用，`stderr`（tracing）没人看得到，网页那头也没有「命令执行结果」
/// 的回传通道 —— 注入被拒绝/降级时如果只写日志，用户看到的就是「点了没反应」。
/// 所以除了 `client_log` 留档，这里再弹一条系统通知当场告知。
pub fn notify(msg: &str) {
    #[cfg(target_os = "macos")]
    {
        // 引号会截断 AppleScript 字面量，换成中文引号（只影响提示文案观感）。
        let safe = msg.replace('"', "\u{201d}");
        let script = format!("display notification \"{safe}\" with title \"终端任务监控\"");
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = msg;
}

fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}…")
    } else {
        t
    }
}

/// 撰写框的候选 `AXDescription`（= 网页的 aria-label，本质是**产品文案**）。
///
/// 文案随版本/语言变，所以它只是**第一顺位**候选，命中不了还有两条结构判据兜底
/// （见 [`pick_composer`]）。要临时补一条新文案不必重新编译：
/// `AM_DESKTOP_COMPOSER_DESC="文案一,文案二"` 会追加进这份名单。
///
/// 只有 macOS 实现用得上；非 macOS 上整条路径都不存在，留着会被 `-D warnings` 判死。
#[cfg(target_os = "macos")]
const COMPOSER_DESC: &[&str] = &[
    "write your prompt to claude",
    "给 claude 写提示词",
    "message chatgpt",
    "询问任何问题",
    "随心输入",
];

/// 发送键的候选 `AXDescription`/`AXTitle`。同样只是第一顺位，见 [`pick_send_button`]。
/// 可用 `AM_DESKTOP_SEND_DESC` 追加。
#[cfg(target_os = "macos")]
const SEND_DESC: &[&str] = &["send message", "发送消息", "发送", "send"];

#[cfg(target_os = "macos")]
fn extra_candidates(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex};
    use core_foundation_sys::base::{
        kCFAllocatorDefault, Boolean, CFGetTypeID, CFRange, CFRelease, CFRetain, CFTypeRef,
    };
    use core_foundation_sys::string::{
        kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringGetBytes, CFStringGetLength,
        CFStringGetTypeID, CFStringRef,
    };
    use core_foundation_sys::url::{CFURLGetString, CFURLGetTypeID, CFURLRef};
    use std::ffi::c_void;
    use std::time::Duration;

    pub(super) type ElemRef = *const c_void;
    type AxError = i32;

    const AX_OK: AxError = 0;
    /// Chromium 分支（ChatGPT.app）不认 `AXManualAccessibility`，返回这个码，属正常。
    const AX_ATTR_UNSUPPORTED: AxError = -25205;
    /// 元素句柄已失效（kAXErrorInvalidUIElement）。App 内部一导航，之前那棵树上的
    /// 元素就全被销毁了，而后台状态下我们手里那棵是**导航前的旧快照**——
    /// 找得到「撰写框」，一写就报这个码。见 [`inject`] 里对它的处理。
    const AX_INVALID_ELEMENT: AxError = -25202;
    const AXVALUE_CGPOINT: u32 = 1;
    const AXVALUE_CGSIZE: u32 = 2;

    /// 单次遍历的节点上限。Claude.app 首页实测 343 个节点 / 68ms；留足余量的同时
    /// 保证长会话里的超大树不会把注入线程挂死。命中上限会写进诊断信息。
    const MAX_NODES: usize = 20_000;
    const MAX_DEPTH: usize = 60;
    /// 回读校验的最多轮数与每轮间隔。每轮都先「踢醒」再读，实测第 1 轮（约 200ms）就对上。
    const VERIFY_ROUNDS: usize = 8;
    const VERIFY_INTERVAL: Duration = Duration::from_millis(150);

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> Boolean;
        fn AXUIElementCreateApplication(pid: i32) -> ElemRef;
        fn AXUIElementCopyAttributeValue(
            el: ElemRef,
            attr: CFStringRef,
            out: *mut CFTypeRef,
        ) -> AxError;
        fn AXUIElementSetAttributeValue(el: ElemRef, attr: CFStringRef, val: CFTypeRef) -> AxError;
        fn AXUIElementIsAttributeSettable(
            el: ElemRef,
            attr: CFStringRef,
            out: *mut Boolean,
        ) -> AxError;
        fn AXUIElementPerformAction(el: ElemRef, action: CFStringRef) -> AxError;
        fn AXValueGetValue(v: ElemRef, ty: u32, out: *mut c_void) -> Boolean;
    }

    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CgPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CgSize {
        w: f64,
        h: f64,
    }

    /// 持有所有权的 CFString（Drop 释放）。CF 的 Create/Copy 规则是「谁 Create/Copy 谁
    /// Release」，用 RAII 包住免得每条早退路径都要手写 CFRelease。
    struct CfStr(CFStringRef);
    impl CfStr {
        fn new(s: &str) -> Self {
            Self(unsafe {
                CFStringCreateWithBytes(
                    kCFAllocatorDefault,
                    s.as_ptr(),
                    s.len() as isize,
                    kCFStringEncodingUTF8,
                    0,
                )
            })
        }
        fn get(&self) -> CFStringRef {
            self.0
        }
    }
    impl Drop for CfStr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) }
            }
        }
    }

    /// 持有所有权的任意 CF 对象（Drop 释放）。
    struct CfObj(CFTypeRef);
    impl Drop for CfObj {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }

    /// 持有所有权的 AXUIElement（Drop 释放）。
    pub(super) struct Elem(ElemRef);
    impl Elem {
        /// 接管一个**已经 +1 过引用**的指针。
        fn owned(p: ElemRef) -> Self {
            Self(p)
        }
        /// 借用一个**不归自己所有**的指针，自己 retain 一份。
        fn retained(p: ElemRef) -> Self {
            unsafe { CFRetain(p as CFTypeRef) };
            Self(p)
        }
        pub(super) fn get(&self) -> ElemRef {
            self.0
        }
    }
    impl Drop for Elem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) }
            }
        }
    }

    fn cf_to_string(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        unsafe {
            if CFGetTypeID(s as CFTypeRef) != CFStringGetTypeID() {
                return None;
            }
            let range = CFRange {
                location: 0,
                length: CFStringGetLength(s),
            };
            let mut needed: isize = 0;
            CFStringGetBytes(
                s,
                range,
                kCFStringEncodingUTF8,
                0,
                0,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
            if needed <= 0 {
                return Some(String::new());
            }
            let mut buf = vec![0u8; needed as usize];
            CFStringGetBytes(
                s,
                range,
                kCFStringEncodingUTF8,
                0,
                0,
                buf.as_mut_ptr(),
                needed,
                &mut needed,
            );
            String::from_utf8(buf).ok()
        }
    }

    fn attr(el: ElemRef, name: &str) -> Option<CfObj> {
        let key = CfStr::new(name);
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(el, key.get(), &mut out) };
        if err != AX_OK || out.is_null() {
            return None;
        }
        Some(CfObj(out))
    }

    fn attr_string(el: ElemRef, name: &str) -> Option<String> {
        cf_to_string(attr(el, name)?.0 as CFStringRef)
    }

    /// 读一个值为 `CFURL` 的属性（`AXURL`）。
    ///
    /// 不能走 [`attr_string`]：`AXURL` 回来的是 CFURL 不是 CFString，
    /// `cf_to_string` 的类型闸门会把它判掉，永远返回 None。
    /// `CFURLGetString` 是 Get 语义（不转移所有权），返回的串归那个 CFURL 所有，
    /// 在 `CfObj` 还活着的这一行里读完就行，不许 release。
    fn attr_url(el: ElemRef, name: &str) -> Option<String> {
        let o = attr(el, name)?;
        unsafe {
            if CFGetTypeID(o.0) != CFURLGetTypeID() {
                return None;
            }
            cf_to_string(CFURLGetString(o.0 as CFURLRef))
        }
    }

    /// 窗口里全部 web 区域的「URL + 文档标题」。
    ///
    /// Claude.app 实测有两个：Electron 外壳自己那张
    /// `file:///Applications/Claude.app/…/index.html`，和真正装着会话的
    /// `https://claude.ai/…`。这里不挑、不猜哪张是「主」的，全收回来交给
    /// [`url_names_session`] 逐个比 —— 挑错了就等于把判据建在猜测上。
    pub(super) fn web_areas(root: ElemRef) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut visited = 0usize;
        let mut stack: Vec<(Elem, usize)> = vec![(Elem::retained(root), 0)];
        while let Some((el, depth)) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                break;
            }
            if attr_string(el.get(), "AXRole").as_deref() == Some("AXWebArea") {
                out.push((
                    attr_url(el.get(), "AXURL").unwrap_or_default(),
                    attr_string(el.get(), "AXTitle").unwrap_or_default(),
                ));
            }
            if depth < MAX_DEPTH {
                for c in children(el.get()) {
                    stack.push((c, depth + 1));
                }
            }
        }
        out
    }

    /// 这个 URL 指的是不是 `session_id` 这条会话。
    ///
    /// 判据：**把 URL 按 `/` 切开，某一段完全等于会话 id**。会话 id 是
    /// `local_<uuid>`，全局唯一，所以命中即确定；反过来，别的会话的 URL 里
    /// 不可能出现这条会话的 uuid，不存在误放行。
    ///
    /// 之所以比「路径段」而不是整条路由（`/cowork/<id>`）：路由名是上游的产品结构，
    /// 改名就会让这里永远拒绝；而会话 id 是数据主键，改不了。同一个取舍上游自己也做过
    /// —— app.asar 里判「用户是不是正在看这条会话」用的就是 `getURL().includes(id)`。
    /// 这里比整段而不是子串包含，是为了不让一个 id 意外命中另一个更长的 id。
    pub(super) fn url_names_session(url: &str, session_id: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        // 查询串/锚点不属于路径，先切掉，免得 `?next=local_x` 这种回跳参数冒充当前会话。
        let path = url.split(['?', '#']).next().unwrap_or(url);
        path.split('/').any(|seg| seg == session_id)
    }

    fn attr_point(el: ElemRef, name: &str) -> Option<CgPoint> {
        let o = attr(el, name)?;
        let mut p = CgPoint::default();
        let ok = unsafe {
            AXValueGetValue(
                o.0 as ElemRef,
                AXVALUE_CGPOINT,
                &mut p as *mut _ as *mut c_void,
            )
        };
        (ok != 0).then_some(p)
    }

    fn attr_size(el: ElemRef, name: &str) -> Option<CgSize> {
        let o = attr(el, name)?;
        let mut s = CgSize::default();
        let ok = unsafe {
            AXValueGetValue(
                o.0 as ElemRef,
                AXVALUE_CGSIZE,
                &mut s as *mut _ as *mut c_void,
            )
        };
        (ok != 0).then_some(s)
    }

    fn children(el: ElemRef) -> Vec<Elem> {
        let Some(o) = attr(el, "AXChildren") else {
            return Vec::new();
        };
        unsafe {
            if CFGetTypeID(o.0) != CFArrayGetTypeID() {
                return Vec::new();
            }
            let arr = o.0 as core_foundation_sys::array::CFArrayRef;
            let n = CFArrayGetCount(arr);
            let mut out = Vec::with_capacity(n.max(0) as usize);
            for i in 0..n {
                let c = CFArrayGetValueAtIndex(arr, i);
                if !c.is_null() {
                    out.push(Elem::retained(c as ElemRef));
                }
            }
            out
        }
    }

    fn value_settable(el: ElemRef) -> bool {
        let key = CfStr::new("AXValue");
        let mut b: Boolean = 0;
        unsafe { AXUIElementIsAttributeSettable(el, key.get(), &mut b) == AX_OK && b != 0 }
    }

    /// 树里被收集下来的一个候选元素。位置/尺寸留着做结构判据（撰写框在窗口底部、
    /// 发送键在撰写框那一行的最右侧），不做任何**索引路径**判断 ——
    /// 实测路径随导航状态变（Claude 首页与会话内完全不同），写死索引必炸。
    pub struct Node {
        el: Elem,
        pub role: String,
        pub desc: String,
        pub title: String,
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
        pub settable: bool,
    }

    impl Node {
        fn label(&self) -> String {
            if !self.desc.is_empty() {
                self.desc.clone()
            } else {
                self.title.clone()
            }
        }
        /// 元素身份指纹。用来对比「写入前后按钮集合」的差异 —— 发送键正是在撰写框
        /// 从空变成有内容时**冒出来**的那个键（Claude 空撰写框上根本没有发送键，
        /// 那个位置摆的是语音键；照「最右侧按钮」硬取就会去按语音）。
        fn fingerprint(&self) -> String {
            format!(
                "{}|{}|{}|{:.0},{:.0}",
                self.role, self.desc, self.title, self.x, self.y
            )
        }
    }

    fn collect(root: ElemRef, want: &[&str]) -> (Vec<Node>, usize, bool) {
        let mut found = Vec::new();
        let mut visited = 0usize;
        let mut stack: Vec<(Elem, usize)> = vec![(Elem::retained(root), 0)];
        let mut hit_cap = false;
        while let Some((el, depth)) = stack.pop() {
            visited += 1;
            if visited > MAX_NODES {
                hit_cap = true;
                break;
            }
            let role = attr_string(el.get(), "AXRole").unwrap_or_default();
            if want.contains(&role.as_str()) {
                let p = attr_point(el.get(), "AXPosition").unwrap_or_default();
                let s = attr_size(el.get(), "AXSize").unwrap_or_default();
                found.push(Node {
                    el: Elem::retained(el.get()),
                    desc: attr_string(el.get(), "AXDescription").unwrap_or_default(),
                    title: attr_string(el.get(), "AXTitle").unwrap_or_default(),
                    settable: value_settable(el.get()),
                    role,
                    x: p.x,
                    y: p.y,
                    w: s.w,
                    h: s.h,
                });
            }
            if depth < MAX_DEPTH {
                // 后压的先弹：靠后的兄弟先被访问。撰写框在 DOM 里通常靠后，
                // 这样命中得更早（不影响正确性，只影响命中时机）。
                for c in children(el.get()) {
                    stack.push((c, depth + 1));
                }
            }
        }
        (found, visited, hit_cap)
    }

    pub(super) fn app_element(pid: u32) -> Elem {
        Elem::owned(unsafe { AXUIElementCreateApplication(pid as i32) })
    }

    /// Electron（Claude.app）必须先打开这个开关，否则 window 下只有一串空 AXGroup、
    /// 什么都找不到。Chromium 分支（ChatGPT.app）不支持它，返回 -25205，忽略即可。
    pub(super) fn enable_manual_accessibility(app: &Elem) -> AxError {
        let key = CfStr::new("AXManualAccessibility");
        unsafe { AXUIElementSetAttributeValue(app.get(), key.get(), kCFBooleanTrue) }
    }

    pub(super) fn target_window(app: &Elem) -> Option<Elem> {
        for name in ["AXFocusedWindow", "AXMainWindow"] {
            if let Some(o) = attr(app.get(), name) {
                return Some(Elem::retained(o.0 as ElemRef));
            }
        }
        let o = attr(app.get(), "AXWindows")?;
        unsafe {
            if CFGetTypeID(o.0) != CFArrayGetTypeID() {
                return None;
            }
            let arr = o.0 as core_foundation_sys::array::CFArrayRef;
            if CFArrayGetCount(arr) == 0 {
                return None;
            }
            Some(Elem::retained(CFArrayGetValueAtIndex(arr, 0) as ElemRef))
        }
    }

    /// 把被后台节流的渲染进程「踢醒」，让它把可访问性树重新序列化一遍。
    ///
    /// 这是本模块最反直觉的一处，写清原因免得后人删掉它：App 在后台时纯读 AX 属性
    /// **永远拿不到新值** —— 实测连读 20 次跨 4 秒，读回来的一直是上一次写入的内容。
    /// 唯一有效的办法是再发一次**会改状态**的 AX 请求把渲染进程唤醒。重设 `AXFocused`
    /// 是其中最无害的一个：它只把插入点放回撰写框，不抢系统前台焦点
    /// （实测写入前后 frontmost 应用不变），也不改内容。
    fn wake(el: ElemRef) {
        let key = CfStr::new("AXFocused");
        unsafe { AXUIElementSetAttributeValue(el, key.get(), kCFBooleanTrue) };
    }

    /// 撰写框是不是空的。
    ///
    /// 不能直接看 `AXValue.is_empty()`：Chromium 把空的可编辑段落报成 `"\n"`，
    /// 而 ChatGPT 还会把**占位文案**当成内容混进 `AXValue`
    /// （空撰写框实测读回 `"\n随心输入"`）。照字面判空，要么永远误判成「有草稿」而
    /// 拒绝一切注入，要么反过来把占位文案当草稿。
    ///
    /// 两条判据都成立才算空，任何一条拿不准就当作**有草稿**（宁可拒绝，绝不覆盖）：
    /// 1. `AXValue` 为空，或以那个孤零零的 `"\n"` 打头且其后不再有换行
    ///    （多行草稿必然带第二个换行）；
    /// 2. 文本域下**深度 2**（`AXTextArea > AXGroup > AXStaticText`）没有非空静态文本
    ///    —— 草稿正文长在这一层；ChatGPT 的占位文案多套一层 `AXGroup`、长在深度 3，
    ///    因此不会被误当成草稿。以「Shift+Enter 开头」的草稿（`AXValue` 同样以 `"\n"`
    ///    打头、骗得过第 1 条）实测由这一条拦下。
    fn composer_is_empty(el: ElemRef, value: &str) -> bool {
        if value.is_empty() {
            return true;
        }
        let Some(rest) = value.strip_prefix('\n') else {
            return false;
        };
        if rest.contains('\n') {
            return false;
        }
        !has_content_text(el, 0)
    }

    /// 文本域下深度 2 是否存在非空 `AXStaticText`（草稿正文所在的层）。
    fn has_content_text(el: ElemRef, depth: usize) -> bool {
        if depth >= 2 {
            let role = attr_string(el, "AXRole").unwrap_or_default();
            return role == "AXStaticText"
                && !attr_string(el, "AXValue").unwrap_or_default().is_empty();
        }
        children(el)
            .iter()
            .any(|c| has_content_text(c.get(), depth + 1))
    }

    /// 撰写框候选顺序（**没有一条依赖索引路径**）：
    /// 1. `AXDescription` 命中已知文案名单（含 `AM_DESKTOP_COMPOSER_DESC` 追加的）；
    /// 2. 全树**唯一**可写 `AXTextArea`（ChatGPT 的撰写框无 description、无 id，
    ///    唯一性就是它的结构特征）；
    /// 3. 位置最靠下、且中心落在窗口下半部的可写 `AXTextArea`（撰写框永远在底部）。
    ///
    /// 三条都落空就返回 None，由调用方拼出可诊断的失败信息 —— 绝不静默失败。
    fn pick_composer(nodes: &[Node], win_y: f64, win_h: f64) -> Option<&Node> {
        let writable: Vec<&Node> = nodes.iter().filter(|n| n.settable).collect();
        if writable.is_empty() {
            return None;
        }
        let mut wanted: Vec<String> = COMPOSER_DESC.iter().map(|s| s.to_string()).collect();
        wanted.extend(extra_candidates("AM_DESKTOP_COMPOSER_DESC"));
        if let Some(n) = writable.iter().find(|n| {
            let d = n.desc.to_ascii_lowercase();
            !d.is_empty() && wanted.iter().any(|w| d.contains(w.as_str()))
        }) {
            return Some(n);
        }
        if writable.len() == 1 {
            return Some(writable[0]);
        }
        writable
            .into_iter()
            .filter(|n| n.y + n.h / 2.0 > win_y + win_h * 0.5)
            .max_by(|a, b| a.y.total_cmp(&b.y))
    }

    /// 发送键候选顺序（同样一条都不靠索引路径）：
    /// 1. `AXDescription`/`AXTitle` 命中已知文案名单（含 `AM_DESKTOP_SEND_DESC`）。
    ///    Claude.app 走这条：撰写框一有内容，原本那个「Use voice mode」就地换成
    ///    `desc="Send message"`。
    /// 2. **写入后新冒出来**、落在撰写框那一行的按钮。给「空撰写框上根本没有发送键、
    ///    有内容才长出来」的形态用。
    /// 3. 撰写框那一行里**最靠右的无名按钮**（desc 与 title 都为空）。ChatGPT.app 走
    ///    这条：它的发送键没有 description、没有 title、空/非空两种状态下都在，
    ///    前两条都够不着。「无名」是硬条件 —— 旁边的「听写」「添加文件等内容」
    ///    「请求批准」都有名字，正好被排除掉。
    ///
    /// 三条都落空就返回 None：**不猜、不乱按**，让调用方如实报「已写入但没提交」。
    ///
    /// 第 3 条毕竟是按位置猜的，所以只在**回读校验已经通过**之后才会走到；按完还要再
    /// 确认撰写框是否清空，没清空就如实报「按了但确认不了」——按错键的表现也是这个。
    fn pick_send_button<'a>(
        after: &'a [Node],
        before: &[Node],
        composer: (f64, f64, f64),
    ) -> Option<&'a Node> {
        let (cx, cy, ch) = composer;
        // 撰写框那一行的纵向带：撰写框顶边往上 20px，到底边往下 80px
        // （实测 Claude 的发送键在撰写框上沿偏上 5px、ChatGPT 的在下沿偏下 4px）。
        // 三条候选**都**先过这道位置闸门：会话正文里出现一个叫「发送」的按钮
        // （消息内容、工具卡片都可能带），只按名字找就会点到它上面去。
        let (top, bottom) = (cy - 20.0, cy + ch + 80.0);
        let in_row = |n: &&Node| n.y >= top && n.y <= bottom && n.x >= cx;

        let mut wanted: Vec<String> = SEND_DESC.iter().map(|s| s.to_string()).collect();
        wanted.extend(extra_candidates("AM_DESKTOP_SEND_DESC"));
        if let Some(n) = after.iter().filter(in_row).find(|n| {
            let l = n.label().to_ascii_lowercase();
            !l.is_empty() && wanted.iter().any(|w| l == *w || l.contains(w.as_str()))
        }) {
            return Some(n);
        }

        let old: std::collections::HashSet<String> =
            before.iter().map(|n| n.fingerprint()).collect();
        if let Some(n) = after
            .iter()
            .filter(|n| !old.contains(&n.fingerprint()))
            .filter(in_row)
            .max_by(|a, b| a.x.total_cmp(&b.x))
        {
            return Some(n);
        }
        after
            .iter()
            .filter(|n| n.label().is_empty())
            .filter(in_row)
            .max_by(|a, b| a.x.total_cmp(&b.x))
    }

    /// 回读对不上时，能不能说清是**哪种**对不上。
    ///
    /// 目前唯一判得出的成因：ChatGPT.app 的撰写框把换行吞成空格（实测写入 4 行
    /// 37 字符，读回同样 37 字符但全在一个段落里，换行位置变成空格）。Claude.app
    /// 不会。这不是我们能修的 —— 换行要靠合成键盘事件才进得去，而那必须抢前台焦点，
    /// 正是这条路线要避开的。判据是「拿写进去的和读回来的逐字比」，不认任何产品文案。
    pub(super) fn verify_hint(wrote: &str, read_back: &str) -> &'static str {
        if wrote.contains('\n')
            && !read_back.contains('\n')
            && wrote.replace('\n', " ") == *read_back
        {
            return "该应用的撰写框不接受换行（换行被吞成空格），只能发单行内容";
        }
        ""
    }

    /// 对外入口：跑 [`attempt`]，只有在它报「句柄失效」时**再跑一趟**。
    ///
    /// 这不是「重试到看起来成功为止」：`-25202` 是一个含义明确、且可恢复的错误
    /// ——「你手里这个元素已经不存在了」，正确的应对就是重新解析一次句柄。所以
    /// 只对这一个错误码重试，只重试一次，其余任何失败都原样上报。
    pub fn inject(
        host_pid: u32,
        target: &SessionRef,
        text: &str,
        submit: bool,
    ) -> Result<(Injected, String), InjectError> {
        match attempt(host_pid, target, text, submit) {
            Ok(v) => Ok(v),
            Err((e, false)) => Err(e),
            Err((_, true)) => attempt(host_pid, target, text, submit).map_err(|(e, _)| e),
        }
    }

    fn diag_line(n: &Node) -> String {
        format!(
            "{}[desc={:?} title={:?} {:.0},{:.0} {:.0}x{:.0} 可写={}]",
            n.role, n.desc, n.title, n.x, n.y, n.w, n.h, n.settable
        )
    }

    /// 跑一趟完整的「定位 → 查草稿 → 写入 → 回读 → （可选）提交」。
    ///
    /// 失败时第二个返回值是「句柄失效，重走一遍多半就好了」——只有 [`inject`] 关心它。
    fn attempt(
        host_pid: u32,
        target: &SessionRef,
        text: &str,
        submit: bool,
    ) -> Result<(Injected, String), (InjectError, bool)> {
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err((InjectError::NotTrusted, false));
        }
        let app = app_element(host_pid);
        let manual = enable_manual_accessibility(&app);
        if manual != AX_OK && manual != AX_ATTR_UNSUPPORTED {
            // 不是「不支持」而是别的错，说明这个 App 的 AX 通道本身有问题，早报早好。
            return Err((
                InjectError::Other(format!("打开可访问性树失败 AXError={manual}")),
                false,
            ));
        }
        std::thread::sleep(Duration::from_millis(120));
        let win = target_window(&app).ok_or_else(|| {
            (
                InjectError::Other(format!("进程 {host_pid} 没有可见窗口")),
                false,
            )
        })?;
        let wp = attr_point(win.get(), "AXPosition").unwrap_or_default();
        let ws = attr_size(win.get(), "AXSize").unwrap_or_default();

        let (fields, visited, capped) = collect(win.get(), &["AXTextArea", "AXTextField"]);
        let Some(composer) = pick_composer(&fields, wp.y, ws.h) else {
            let listed: Vec<String> = fields.iter().take(8).map(diag_line).collect();
            return Err((InjectError::ComposerNotFound {
                diag: format!(
                    "遍历 {visited} 个节点{}，找到 {} 个文本域：[{}]；期望 role=AXTextArea 且 AXValue 可写，\
                     description 命中 {:?} 之一，或全树唯一，或位于窗口下半部最靠下的那个",
                    if capped { "（触到上限被截断）" } else { "" },
                    fields.len(),
                    listed.join(", "),
                    COMPOSER_DESC
                ),
            }, false));
        };
        let cel = composer.el.get();
        let cbox = (composer.x, composer.y, composer.h);

        // 会话身份校验。排在**所有动作最前面**：屏幕上要是别的会话，后面每一步都不该发生
        // —— 连草稿判定都不该做，那是别人的草稿。
        //
        // 读之前先踢醒渲染进程（理由见 `wake`）。这一下在这里比在别处更要紧：后台节流时
        // 直接读 URL 拿到的是**导航前**的旧值，而旧值恰好错在最危险的方向 —— 用户刚切走，
        // 我们却还以为停在目标会话上，于是把话发进别人的对话里。
        if let SessionRef::ClaudeLocalAgent(want) = target {
            wake(cel);
            std::thread::sleep(Duration::from_millis(150));
            let areas = web_areas(win.get());
            if !areas.iter().any(|(url, _)| url_names_session(url, want)) {
                // 说清屏幕上是哪条，用户才知道该切到哪去。这里用文档标题只为了让提示
                // 好读，判据自始至终只有上面那条 URL 比对，跟标题文案无关。
                let on_screen = areas
                    .iter()
                    .find(|(url, _)| url.starts_with("http"))
                    .map(|(url, title)| {
                        if title.is_empty() {
                            url.clone()
                        } else {
                            format!("{} / {url}", truncate(title, 40))
                        }
                    })
                    .unwrap_or_else(|| {
                        format!("认不出来（{} 个 web 区域都没有可读的 URL）", areas.len())
                    });
                return Err((
                    InjectError::WrongSession {
                        want: want.clone(),
                        on_screen,
                    },
                    false,
                ));
            }
        }

        // 草稿保护：只要读到内容就拒绝，绝不覆盖用户没发出去的东西。
        //
        // 每次都必须「先踢醒再读」：直接读拿到的是上一次状态变更前的旧快照
        // （见 `wake` 的说明），而旧快照两个方向都会错 —— 用户刚清空却读成有草稿
        // （白白误拒），用户刚敲下半句却读成空（把它盖掉，这是绝不能出的事）。
        // 读两轮是为了防单次刷新没跟上，两轮都得是踢醒后的**新鲜**读数才有意义。
        for _ in 0..2 {
            wake(cel);
            std::thread::sleep(Duration::from_millis(150));
            let v = attr_string(cel, "AXValue").unwrap_or_default();
            if !composer_is_empty(cel, &v) {
                return Err((
                    InjectError::DraftPresent {
                        preview: truncate(v.trim_start_matches('\n'), 20),
                    },
                    false,
                ));
            }
        }

        // 提交要用的「写入前按钮集合」：发送键是写入后才冒出来的那一个。
        let buttons_before = if submit {
            collect(win.get(), &["AXButton"]).0
        } else {
            Vec::new()
        };

        let key = CfStr::new("AXValue");
        let val = CfStr::new(text);
        let err = unsafe { AXUIElementSetAttributeValue(cel, key.get(), val.get() as CFTypeRef) };
        if err == AX_INVALID_ELEMENT {
            // 句柄失效：手里这棵树是导航前的旧快照。刚才这次写虽然失败了，但它是个**会改
            // 状态**的请求，已经把被节流的渲染进程踢醒 —— 交给 `inject` 重走一遍，
            // 那一遍拿到的就是新树。整趟从头再来（含草稿判定），不是接着这半截往下写：
            // 新页面上可能正躺着用户的草稿。
            return Err((
                InjectError::Other("撰写框句柄已失效（页面刚跳转）".into()),
                true,
            ));
        }
        if err != AX_OK {
            return Err((
                InjectError::Other(format!("写入撰写框失败 AXError={err}")),
                false,
            ));
        }

        // 回读校验。每轮先踢醒再读 —— 只读不写永远拿不到新值（见 `wake` 的说明）。
        // 轮数封顶，失败就如实上报，不会为了「看起来成功」反复重写。
        let mut last = String::new();
        let mut ok = false;
        for _ in 0..VERIFY_ROUNDS {
            wake(cel);
            std::thread::sleep(VERIFY_INTERVAL);
            last = attr_string(cel, "AXValue").unwrap_or_default();
            if last == text {
                ok = true;
                break;
            }
        }
        if !ok {
            // 写歪了就把撰写框收拾干净再报错：留着一段半残的内容在那儿，用户下次
            // 打开 App 会以为是自己写的，我们自己下一次注入也会被草稿保护挡住。
            // 这里写回空串是安全的 —— 走到这一步说明进来时它本就是空的，
            // 里面躺着的只有我们刚写的那份。
            let empty = CfStr::new("");
            unsafe { AXUIElementSetAttributeValue(cel, key.get(), empty.get() as CFTypeRef) };
            return Err((
                InjectError::VerifyFailed {
                    wrote: text.chars().count(),
                    read_back: last.clone(),
                    hint: verify_hint(text, &last),
                },
                false,
            ));
        }
        let wrote_msg = format!("已写入 {} 字符并回读校验通过", text.chars().count());

        if !submit {
            return Ok((
                Injected::Written {
                    why: "调用方未要求提交",
                },
                wrote_msg,
            ));
        }

        // 再踢一次再扫按钮：发送键是「撰写框有了内容」之后才渲染出来的，比刚才那次
        // 回读又晚一拍。少了这一下，扫到的还是空撰写框那套按钮（Claude 上是语音键），
        // 于是明明写进去了却报「没找到发送键」——实测就是这么漏的。
        wake(cel);
        std::thread::sleep(VERIFY_INTERVAL);
        let buttons_after = collect(win.get(), &["AXButton"]).0;
        let Some(btn) = pick_send_button(&buttons_after, &buttons_before, cbox) else {
            return Ok((
                Injected::Written {
                    why: "没找到发送键，内容留在撰写框里等手动发送",
                },
                wrote_msg,
            ));
        };
        let press = CfStr::new("AXPress");
        let perr = unsafe { AXUIElementPerformAction(btn.el.get(), press.get()) };
        if perr != AX_OK {
            return Ok((
                Injected::Written {
                    why: "按发送键失败，内容留在撰写框里等手动发送",
                },
                format!(
                    "{wrote_msg}；AXPress 报错 {perr}（发送键 {}）",
                    diag_line(btn)
                ),
            ));
        }
        // 提交是否真的发生：撰写框应当回到「空」。同样要先踢醒再读。
        let mut cleared = false;
        for _ in 0..VERIFY_ROUNDS {
            wake(cel);
            std::thread::sleep(VERIFY_INTERVAL);
            let v = attr_string(cel, "AXValue").unwrap_or_default();
            if composer_is_empty(cel, &v) {
                cleared = true;
                break;
            }
        }
        if cleared {
            Ok((
                Injected::Submitted,
                format!("{wrote_msg}，已按发送键并确认撰写框已清空"),
            ))
        } else {
            Ok((
                Injected::Written {
                    why: "按了发送键但撰写框没清空，无法确认已提交",
                },
                wrote_msg,
            ))
        }
    }
}

/// 往桌面客户端里 `target` 那条会话的撰写框注入一条消息。
///
/// `host_pid` 是宿主 GUI 应用（Claude.app / ChatGPT.app）的进程 id，
/// 由 `am_core::process::desktop_host` 顺父链算出来。
///
/// `target` 是要发给哪条会话：
/// - [`SessionRef::ClaudeLocalAgent`]：这里会先比对窗口 URL，对不上直接拒绝
///   （[`InjectError::WrongSession`]），宿主上有几条会话都不影响；
/// - [`SessionRef::Unidentified`]：这里比不了，**调用方必须已经确认该宿主上只有这一条
///   会话**，否则就是在赌。
///
/// 调用前还要先过 [`enabled`] 这道闸门。
#[cfg(target_os = "macos")]
pub fn inject(
    host_pid: u32,
    target: &SessionRef,
    text: &str,
    submit: bool,
) -> Result<(Injected, String), InjectError> {
    imp::inject(host_pid, target, text, submit)
}

#[cfg(not(target_os = "macos"))]
pub fn inject(
    host_pid: u32,
    target: &SessionRef,
    text: &str,
    submit: bool,
) -> Result<(Injected, String), InjectError> {
    let _ = (host_pid, target, text, submit);
    Err(InjectError::Other(
        "桌面客户端注入目前只在 macOS 上实现（Windows 需改用 UIAutomation）".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 开关默认关闭：没设环境变量时 `enabled()` 必须是 false。
    /// 这些用例会动进程级环境变量，`cargo test` 默认多线程并行，所以串在一个用例里跑。
    #[test]
    fn switch_defaults_to_off() {
        std::env::remove_var("AM_DESKTOP_INJECT");
        assert!(!enabled(), "未配置时实验开关必须是关的");
        for on in ["1", "true", "TRUE", "yes", "on"] {
            std::env::set_var("AM_DESKTOP_INJECT", on);
            assert!(enabled(), "{on} 应当算开启");
        }
        for off in ["0", "false", "no", "", " "] {
            std::env::set_var("AM_DESKTOP_INJECT", off);
            assert!(!enabled(), "{off:?} 应当算关闭");
        }
        std::env::remove_var("AM_DESKTOP_INJECT");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extra_candidates_are_split_and_normalized() {
        std::env::set_var(
            "AM_DESKTOP_TEST_LIST",
            " 给 Claude 写提示词 , Message ChatGPT ,, ",
        );
        let got = extra_candidates("AM_DESKTOP_TEST_LIST");
        assert_eq!(got, vec!["给 claude 写提示词", "message chatgpt"]);
        std::env::remove_var("AM_DESKTOP_TEST_LIST");
        assert!(extra_candidates("AM_DESKTOP_TEST_LIST").is_empty());
    }

    #[test]
    fn error_messages_are_actionable() {
        let e = InjectError::DraftPresent {
            preview: "没发出去的半句话".into(),
        };
        let s = e.to_string();
        assert!(s.contains("未发送内容"), "{s}");
        let e = InjectError::VerifyFailed {
            wrote: 12,
            read_back: "对不上的内容".into(),
            hint: "",
        };
        assert!(e.to_string().contains("回读对不上"));
    }

    /// 回读对不上时要能说清成因。ChatGPT.app 把换行吞成空格是实测到的唯一可判定成因，
    /// 判据是「写进去的换行换成空格后与读回来的逐字相等」，不认任何产品文案。
    #[cfg(target_os = "macos")]
    #[test]
    fn verify_hint_spots_newline_flattening() {
        use super::imp::verify_hint;
        assert!(verify_hint("第一行\n第二行", "第一行 第二行").contains("不接受换行"));
        // 内容对得上就没有「对不上」这回事，不该给提示
        assert_eq!(verify_hint("单行", "单行"), "");
        // 原文本来就没换行 → 不是这个成因
        assert_eq!(verify_hint("abc", "abd"), "");
        // 换行还在，只是内容截断了 → 也不是这个成因，别乱扣帽子
        assert_eq!(verify_hint("a\nb", "a\n"), "");
    }

    /// 会话身份比对：URL 的某一段等于会话 id 才算「屏幕上就是这条」。
    /// 这条判据只要松一点，就会把消息发进别人的对话里，所以两个方向都得钉住。
    #[cfg(target_os = "macos")]
    #[test]
    fn url_matching_identifies_the_session_on_screen() {
        use super::imp::url_names_session;
        let id = "local_18c6b796-a156-4546-a70e-0dd4da930073";
        // 实测的本地代理路由形态
        assert!(url_names_session(
            &format!("https://claude.ai/cowork/{id}"),
            id
        ));
        // 路由名变了也照样认得出——判据是会话 id 那一段，不是产品的路由结构
        assert!(url_names_session(
            &format!("https://claude.ai/agent/{id}/files"),
            id
        ));
        // 屏幕上是别的会话 → 必须不匹配
        assert!(!url_names_session(
            "https://claude.ai/cowork/local_ea3779b7-7948-47f1-9b0c-6b4e15ff8249",
            id
        ));
        // 屏幕上是普通网页对话（不是本地代理会话）→ 不匹配
        assert!(!url_names_session(
            "https://claude.ai/chat/9b5147d7-60cd-4caf-8097-b67892c67267",
            id
        ));
        // Electron 外壳那张空页 → 不匹配
        assert!(!url_names_session(
            "file:///Applications/Claude.app/Contents/Resources/app.asar/index.html",
            id
        ));
        // 查询串里的回跳参数不算「当前开着的会话」
        assert!(!url_names_session(
            &format!("https://claude.ai/cowork/other?next=/cowork/{id}"),
            id
        ));
        // 只是前缀/子串，不是完整的一段 → 不匹配（否则短 id 会命中长 id）
        assert!(!url_names_session(
            &format!("https://claude.ai/cowork/{id}-2"),
            id
        ));
        assert!(!url_names_session(
            "https://claude.ai/cowork/local_18c6",
            id
        ));
        // 空 id 是「认不出身份」，绝不能变成「什么都匹配」
        assert!(!url_names_session("https://claude.ai/cowork/x", ""));
    }

    /// 对着**真实运行中的**宿主 App 读一遍窗口 URL，验证「屏幕上是哪条会话」这条链路
    /// 从 AX 里真的读得出来。
    ///
    /// 只读：只调 `web_areas`（读 `AXURL`/`AXTitle`），不碰撰写框、不写一个字、不按任何键。
    /// 需要辅助功能授权和一个开着的 App，所以标 `#[ignore]`，CI 不跑；手动跑：
    /// `AM_INJECT_PROBE_PID=<宿主 pid> cargo test -p am-client -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "需要真实运行中的宿主 App 与辅助功能授权，靠 AM_INJECT_PROBE_PID 指定"]
    fn reads_on_screen_session_url_from_live_app() {
        let Ok(pid) = std::env::var("AM_INJECT_PROBE_PID") else {
            panic!("请用 AM_INJECT_PROBE_PID=<Claude.app 或 ChatGPT.app 的 pid> 指定宿主");
        };
        let pid: u32 = pid.trim().parse().expect("AM_INJECT_PROBE_PID 必须是数字");
        let app = imp::app_element(pid);
        imp::enable_manual_accessibility(&app);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let win = imp::target_window(&app).expect("宿主没有可见窗口");
        let areas = imp::web_areas(win.get());
        assert!(!areas.is_empty(), "整棵树里一个 AXWebArea 都没有");
        for (url, title) in &areas {
            println!("web 区域：url={url} title={title}");
        }
        // 反向必须成立：一个不在屏幕上的会话 id 绝不能匹配上任何一条 URL。
        let bogus = "local_00000000-0000-0000-0000-000000000000";
        assert!(
            !areas.iter().any(|(u, _)| imp::url_names_session(u, bogus)),
            "不在屏幕上的会话 id 匹配上了，判据太松"
        );
    }

    /// 拒绝的理由要能让用户直接动手：屏幕上是哪条、该发给哪条、下一步做什么。
    #[test]
    fn wrong_session_error_says_what_is_on_screen() {
        let e = InjectError::WrongSession {
            want: "local_abc".into(),
            on_screen: "周报 / https://claude.ai/cowork/local_xyz".into(),
        };
        let s = e.to_string();
        assert!(s.contains("local_abc"), "{s}");
        assert!(s.contains("local_xyz"), "{s}");
        assert!(s.contains("切到前台"), "{s}");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate("一二三四五", 3), "一二三…");
        assert_eq!(truncate("abc", 10), "abc");
    }
}
