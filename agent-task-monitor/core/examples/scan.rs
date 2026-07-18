fn main() {
    let mut s = am_core::process::ProcessScanner::new();
    let procs = s.scan();
    println!("detected {} agent processes:", procs.len());
    for p in &procs {
        println!("  pid={} agent={} tty={:?} cwd={:?} ide={}", p.pid, p.agent, p.tty, p.cwd, p.ide_name);
    }
}
