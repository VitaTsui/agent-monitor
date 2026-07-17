use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};
fn main() {
    let sys = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new().with_cwd(UpdateKind::Always).with_cmd(UpdateKind::Always),
        ),
    );
    for (pid, p) in sys.processes() {
        if p.name() == "claude" {
            println!("claude pid={pid} cwd={:?}", p.cwd());
            let mut cur = *pid;
            for _ in 0..10 {
                let Some(pr) = sys.process(cur) else { println!("  <no proc {cur}>"); break };
                println!("  {} name={:?} exe={:?}", cur, pr.name(), pr.exe());
                match pr.parent() { Some(pp) if pp != cur => cur = pp, _ => break }
            }
        }
    }
}
