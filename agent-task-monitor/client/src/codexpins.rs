//! 从 Codex 自己的本地日志数据库恢复「会话 ↔ TUI 进程」配对。
//!
//! Codex 的 app-server 同时托管多条 thread，打开 rollout 文件的是共享 app-server，
//! 不是各终端的 TUI。因此不能靠文件句柄，也不能在同项目并发时按时间顺序猜。
//! TUI 写入 `markdown_stream` 的助手原文与对应 rollout 中的助手原文完全相同；精确匹配
//! 这段内容，就能把 thread 绑定回真正显示它的 TUI pid。

use am_core::model::ProcessInfo;
use am_core::scanner::SessionSummary;
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// 读取当前用户 Codex 日志中，仍存活的 TUI pid 对应的会话 id。
pub fn session_pins(
    processes: &[ProcessInfo],
    sessions: &[SessionSummary],
) -> Result<HashMap<u32, String>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(HashMap::new());
    };
    let fingerprints: Vec<(String, String)> = sessions
        .iter()
        .filter(|s| s.provider == "codex" && !s.desktop && !s.pairing_fingerprint.is_empty())
        .map(|s| (s.session_id.clone(), s.pairing_fingerprint.clone()))
        .collect();
    session_pins_from(&home.join(".codex/logs_2.sqlite"), processes, &fingerprints)
}

fn session_pins_from(
    path: &Path,
    processes: &[ProcessInfo],
    fingerprints: &[(String, String)],
) -> Result<HashMap<u32, String>> {
    if !path.is_file() || fingerprints.is_empty() {
        return Ok(HashMap::new());
    }

    let live: Vec<u32> = processes
        .iter()
        .filter(|p| p.agent == "codex" && !p.shared_host)
        .map(|p| p.pid)
        .collect();
    if live.is_empty() {
        return Ok(HashMap::new());
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("打开 Codex 日志数据库失败：{}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(100))?;

    // process_uuid 有索引；每个活 TUI 只取最近 4096 个流片段。指纹最多 512 字符，
    // 即使上游按单字拆 delta，这个窗口也足够覆盖，同时不会扫描整张日志表。
    let mut stmt = conn.prepare(
        "SELECT feedback_log_body
         FROM logs
         WHERE process_uuid LIKE ?1
           AND target = 'codex_tui::markdown_stream'
         ORDER BY id DESC
         LIMIT 4096",
    )?;

    let mut candidates: HashMap<u32, Vec<String>> = HashMap::new();
    for pid in live {
        let pattern = format!("pid:{pid}:%");
        let rows = stmt.query_map([pattern], |row| row.get::<_, Option<String>>(0))?;
        let mut deltas = Vec::new();
        for body in rows {
            let Some(body) = body? else { continue };
            let Some(encoded) = body.strip_prefix("push_delta: ") else {
                continue;
            };
            if let Ok(delta) = serde_json::from_str::<String>(encoded) {
                deltas.push(delta);
            }
        }
        deltas.reverse();
        let stream = deltas.concat();
        if stream.is_empty() {
            continue;
        }
        for (sid, fingerprint) in fingerprints {
            if fingerprint.chars().count() >= 24 && stream.contains(fingerprint) {
                candidates.entry(pid).or_default().push(sid.clone());
            }
        }
    }

    // 两边都必须唯一：一个 TUI 命中多条 thread，或一条 thread 同时命中多个 TUI，
    // 都说明日志里出现了引用/重复文本。宁可退回旧配对，也绝不拿歧义证据覆盖缓存。
    let mut sid_hits: HashMap<String, usize> = HashMap::new();
    for sids in candidates.values() {
        for sid in sids {
            *sid_hits.entry(sid.clone()).or_default() += 1;
        }
    }
    Ok(candidates
        .into_iter()
        .filter_map(|(pid, sids)| {
            if sids.len() != 1 || sid_hits.get(sids[0].as_str()) != Some(&1) {
                return None;
            }
            Some((pid, sids.into_iter().next().unwrap()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use am_core::model::IdeKind;
    use rusqlite::params;

    fn process(pid: u32, agent: &str, shared_host: bool) -> ProcessInfo {
        ProcessInfo {
            pid,
            agent: agent.into(),
            tty: String::new(),
            cwd: "/work/same-project".into(),
            ide: IdeKind::Terminal,
            ide_name: "Terminal".into(),
            start_time: 1,
            cpu_usage: 0.0,
            memory: 0,
            command: String::new(),
            shell_pid: None,
            shell_start: None,
            shared_host,
        }
    }

    fn database() -> (std::path::PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("am-codexpins-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("logs_2.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL,
                feedback_log_body TEXT,
                process_uuid TEXT
            );
            CREATE INDEX idx_logs_process_uuid_id ON logs(process_uuid, id DESC);",
        )
        .unwrap();
        (db, conn)
    }

    fn push(conn: &Connection, pid: u32, text: &str) {
        for delta in text.chars().map(|c| c.to_string()) {
            conn.execute(
                "INSERT INTO logs(target, feedback_log_body, process_uuid) VALUES(?1, ?2, ?3)",
                params![
                    "codex_tui::markdown_stream",
                    format!("push_delta: {}", serde_json::to_string(&delta).unwrap()),
                    format!("pid:{pid}:uuid")
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn matches_same_project_sessions_by_exact_tui_output() {
        let (db, conn) = database();
        let text22 = "这是第二十二号会话独有的、长度足够用于可靠配对的完整助手回复。";
        let text23 = "这是第二十三号会话独有的、长度足够用于可靠配对的完整助手回复。";
        push(&conn, 9499, text22);
        push(&conn, 8079, text23);
        drop(conn);

        let pins = session_pins_from(
            &db,
            &[process(8079, "codex", false), process(9499, "codex", false)],
            &[
                ("sid-22".into(), text22.into()),
                ("sid-23".into(), text23.into()),
            ],
        )
        .unwrap();

        assert_eq!(pins.get(&8079).map(String::as_str), Some("sid-23"));
        assert_eq!(pins.get(&9499).map(String::as_str), Some("sid-22"));
        std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
    }

    #[test]
    fn rejects_ambiguous_or_non_tui_evidence() {
        let (db, conn) = database();
        let repeated = "同一段足够长的回复如果在两个终端都出现，就不能拿来决定向哪个终端下发。";
        push(&conn, 8079, repeated);
        push(&conn, 9499, repeated);
        conn.execute(
            "INSERT INTO logs(target, feedback_log_body, process_uuid) VALUES('codex_core', ?1, 'pid:7000:app-server')",
            [format!("push_delta: {}", serde_json::to_string(repeated).unwrap())],
        )
        .unwrap();
        drop(conn);

        let pins = session_pins_from(
            &db,
            &[
                process(8079, "codex", false),
                process(9499, "codex", false),
                process(7000, "codex", true),
            ],
            &[("sid".into(), repeated.into())],
        )
        .unwrap();
        assert!(pins.is_empty());
        std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
    }
}
