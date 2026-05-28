use crate::types::{AgentKind, TokenCounts, TokenEvent};
use anyhow::{anyhow, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Per-watcher state: last seen cumulative `tokens_used` per session_id.
#[derive(Default)]
pub struct PollState {
    pub last_tokens: HashMap<String, u32>,
}

/// Single poll: opens read-only WAL connection, reads all `threads` rows,
/// produces TokenEvents for any session whose `tokens_used` increased since last poll.
/// New sessions (not yet in `last_tokens`) are seeded WITHOUT emitting,
/// unless `seed_emit` is true (used for startup replay).
pub fn poll_once(db_path: &Path, state: &mut PollState, seed_emit: bool) -> Result<Vec<TokenEvent>> {
    if !db_path.exists() { return Err(anyhow!("db not found: {}", db_path.display())); }

    let mut events = vec![];
    let mut last_err: Option<rusqlite::Error> = None;

    for attempt in 0..3 {
        match try_poll(db_path, state, seed_emit) {
            Ok(evs) => { events = evs; last_err = None; break; }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(100 * (attempt + 1)));
            }
        }
    }

    if let Some(e) = last_err {
        return Err(anyhow!("codex poll failed after 3 retries: {:?}", e));
    }
    Ok(events)
}

fn try_poll(db_path: &Path, state: &mut PollState, seed_emit: bool) -> Result<Vec<TokenEvent>, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_millis(500))?;
    let mut stmt = conn.prepare(
        "SELECT id, model, cwd, tokens_used, updated_at_ms FROM threads"
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,      // id (session_id)
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),  // model
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),  // cwd
            r.get::<_, i64>(3)?,         // tokens_used (cumulative)
            r.get::<_, i64>(4)?,         // updated_at_ms
        ))
    })?;

    let mut out = vec![];
    for row in rows {
        let (session_id, model, cwd, tokens_used_i64, updated_at_ms) = row?;
        let cur = tokens_used_i64.max(0) as u32;  // 음수 클램프

        match state.last_tokens.get(&session_id).copied() {
            None => {
                // 새 세션. seed_emit=true (startup replay)면 누적값을 한 번에 흘려보냄.
                if seed_emit && cur > 0 {
                    let ts = UNIX_EPOCH + Duration::from_millis(updated_at_ms.max(0) as u64);
                    out.push(TokenEvent {
                        agent: AgentKind::Codex,
                        project_path: PathBuf::from(&cwd),
                        session_id: session_id.clone(),
                        model,
                        ts,
                        counts: TokenCounts {
                            tokens_in: cur,
                            tokens_out: 0,
                            tokens_cache_read: 0,
                            tokens_cache_create: 0,
                        },
                    });
                }
                state.last_tokens.insert(session_id, cur);
            }
            Some(last) if cur > last => {
                let delta = cur - last;
                let ts = UNIX_EPOCH + Duration::from_millis(updated_at_ms.max(0) as u64);
                out.push(TokenEvent {
                    agent: AgentKind::Codex,
                    project_path: PathBuf::from(&cwd),
                    session_id: session_id.clone(),
                    model,
                    ts,
                    counts: TokenCounts {
                        tokens_in: delta,
                        tokens_out: 0,
                        tokens_cache_read: 0,
                        tokens_cache_create: 0,
                    },
                });
                state.last_tokens.insert(session_id, cur);
            }
            _ => {} // 변화 없거나 감소 — 무시
        }
    }
    Ok(out)
}

pub struct CodexWatcher;

impl CodexWatcher {
    pub fn spawn(db_path: PathBuf, tx: tokio::sync::mpsc::UnboundedSender<TokenEvent>) -> Result<()> {
        if !db_path.exists() {
            tracing::warn!(?db_path, "codex db 없음 — Codex 미설치?");
            return Ok(());
        }
        std::thread::spawn(move || {
            let mut state = PollState::default();
            // Startup replay: 한 번 seed_emit=true로 전체 누적치를 흘림 → aggregator의 rotating bucket가 5h 윈도우만 살림
            match poll_once(&db_path, &mut state, true) {
                Ok(events) => for ev in events { let _ = tx.send(ev); },
                Err(e) => tracing::warn!(?db_path, %e, "codex initial poll error"),
            }
            // Steady state: 2s마다 delta만 흘림
            loop {
                match poll_once(&db_path, &mut state, false) {
                    Ok(events) => for ev in events { let _ = tx.send(ev); },
                    Err(e) => tracing::warn!(?db_path, %e, "codex poll error"),
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn make_db(td: &TempDir) -> std::path::PathBuf {
        let path = td.path().join("state_5.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE threads (
              id TEXT PRIMARY KEY,
              model TEXT,
              cwd TEXT,
              tokens_used INTEGER NOT NULL DEFAULT 0,
              created_at_ms INTEGER NOT NULL,
              updated_at_ms INTEGER NOT NULL
            );
        "#).unwrap();
        path
    }

    fn upsert(path: &std::path::Path, id: &str, tokens: i64, updated_ms: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO threads (id, model, cwd, tokens_used, created_at_ms, updated_at_ms) VALUES (?,?,?,?,?,?)",
            rusqlite::params![id, "gpt-5-codex", "/tmp/p", tokens, updated_ms, updated_ms],
        ).unwrap();
    }

    #[test]
    fn first_poll_seed_emits_when_requested() {
        let td = TempDir::new().unwrap();
        let path = make_db(&td);
        upsert(&path, "s1", 1000, 1_700_000_000_000);
        let mut state = PollState::default();
        let events = poll_once(&path, &mut state, true).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].counts.tokens_in, 1000);
        assert_eq!(state.last_tokens.get("s1"), Some(&1000));
    }

    #[test]
    fn first_poll_no_seed_returns_no_events_but_seeds_state() {
        let td = TempDir::new().unwrap();
        let path = make_db(&td);
        upsert(&path, "s1", 1000, 1_700_000_000_000);
        let mut state = PollState::default();
        let events = poll_once(&path, &mut state, false).unwrap();
        assert_eq!(events.len(), 0);
        assert_eq!(state.last_tokens.get("s1"), Some(&1000));
    }

    #[test]
    fn subsequent_poll_emits_delta() {
        let td = TempDir::new().unwrap();
        let path = make_db(&td);
        upsert(&path, "s1", 1000, 1_700_000_000_000);
        let mut state = PollState::default();
        let _ = poll_once(&path, &mut state, false).unwrap();
        upsert(&path, "s1", 1250, 1_700_000_001_000);
        let events = poll_once(&path, &mut state, false).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].counts.tokens_in, 250);
        assert_eq!(state.last_tokens.get("s1"), Some(&1250));
    }

    #[test]
    fn no_change_no_event() {
        let td = TempDir::new().unwrap();
        let path = make_db(&td);
        upsert(&path, "s1", 1000, 1_700_000_000_000);
        let mut state = PollState::default();
        let _ = poll_once(&path, &mut state, false).unwrap();
        let events = poll_once(&path, &mut state, false).unwrap();
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn missing_db_returns_err() {
        let mut state = PollState::default();
        let r = poll_once(std::path::Path::new("/nonexistent.sqlite"), &mut state, false);
        assert!(r.is_err());
    }
}
