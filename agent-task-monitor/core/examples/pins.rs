//! 诊断：复现客户端的 session_pins() 采集，看 Windows 上到底读不读得到别的进程的 env。
//!
//! 客户端日志长期 `env权威=0`，但手工验证 claude 子进程的 env 里确实有 CLAUDE_PID /
//! CLAUDE_CODE_SESSION_ID。这里直接调同一套 sysinfo 采集路径，把中间量打出来分辨是
//! 「读不到 environ」还是「读到了但被存活祖先校验滤掉」。
//!
//! 跑：cargo run -p am-core --example pins

use sysinfo::{ProcessRefreshKind, System, UpdateKind};

fn main() {
    let mut sys = System::new();
    // 计时：这套扫描要不要放进客户端的每轮循环，取决于它到底多贵
    let t0 = std::time::Instant::now();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_environ(UpdateKind::Always));
    let scan_ms = t0.elapsed().as_millis();
    // 第二次（缓存预热后）更接近稳态循环里的实际开销
    let t1 = std::time::Instant::now();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_environ(UpdateKind::Always));
    let scan2_ms = t1.elapsed().as_millis();
    println!("env 扫描耗时：首次 {scan_ms} ms，二次 {scan2_ms} ms");

    let mut total = 0usize;
    let mut with_env = 0usize;
    let mut claude_procs: Vec<(u32, String)> = Vec::new();
    let mut carriers: Vec<(u32, String, String, String)> = Vec::new(); // reporter, name, CLAUDE_PID, sid

    for (pid, p) in sys.processes() {
        total += 1;
        let environ = p.environ();
        if !environ.is_empty() {
            with_env += 1;
        }
        let name = p.name().to_string();
        if name.to_lowercase().contains("claude") {
            claude_procs.push((pid.as_u32(), name.clone()));
        }
        let (mut cp, mut sid) = (None, None);
        for kv in environ {
            if let Some(v) = kv.strip_prefix("CLAUDE_PID=") {
                cp = Some(v.trim().to_string());
            } else if let Some(v) = kv.strip_prefix("CLAUDE_CODE_SESSION_ID=") {
                if !v.is_empty() {
                    sid = Some(v.to_string());
                }
            }
        }
        if let (Some(cp), Some(sid)) = (cp, sid) {
            carriers.push((pid.as_u32(), name, cp, sid));
        }
    }

    println!("进程总数 = {total}");
    println!(
        "能读到 environ 的进程数 = {with_env}  ({}%)",
        with_env * 100 / total.max(1)
    );
    println!("\nclaude 进程 = {claude_procs:?}");
    println!(
        "\n带 CLAUDE_PID+SESSION_ID 的进程（candidates）= {} 个",
        carriers.len()
    );
    for (pid, name, cp, sid) in &carriers {
        println!(
            "  reporter pid={pid} name={name} CLAUDE_PID={cp} sid={}",
            &sid[..8.min(sid.len())]
        );
    }
}
