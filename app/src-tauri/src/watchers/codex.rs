//! Codex usage reader — 세션 rollout JSONL을 tail해서 턴별 토큰 사용량과 서버 보고
//! rate_limits(5h/주간)를 읽는다. state_5.sqlite는 활성 thread의 rollout_path를 찾는
//! 용도로만 사용한다(threads.tokens_used 누적값은 부정확/분해불가라 더 이상 쓰지 않음).

use crate::types::{AgentKind, TokenCounts, TokenEvent};
use crate::watchers::claude::parse_iso8601;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const FIVE_H: Duration = Duration::from_secs(5 * 3600);

/// 서버 보고 Codex 한도. 두 경로가 같은 슬롯에 쓴다 — rollout tail(payload.rate_limits)과
/// 유휴 시 능동 조회(codex_rpc::read_rate_limits). lib.rs 틱이 읽어 카드에 주입.
#[derive(Default)]
pub struct CodexQuota {
    pub used_pct_5h: Mutex<Option<f32>>,
    pub reset_5h: Mutex<Option<SystemTime>>,
    pub used_pct_weekly: Mutex<Option<f32>>,
    pub reset_weekly: Mutex<Option<SystemTime>>,
    /// 마지막 능동 조회가 실패한 이유(로그인 안 됨 등). 성공하면 None으로 지운다.
    /// 카드의 프로젝트 줄 아래에 그대로 표시된다.
    pub last_error: Mutex<Option<String>>,
}

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<Payload>,
}
#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    info: Option<Info>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}
#[derive(Deserialize, Default)]
struct Info {
    #[serde(default)]
    last_token_usage: Usage,
}
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}
#[derive(Debug, Deserialize, Default)]
pub(crate) struct RateLimits {
    #[serde(default)]
    pub(crate) primary: Option<Window>,
    #[serde(default)]
    pub(crate) secondary: Option<Window>,
}
#[derive(Debug, Deserialize, Default)]
pub(crate) struct Window {
    #[serde(default)]
    pub(crate) used_percent: f64,
    #[serde(default)]
    pub(crate) window_minutes: Option<i64>,
    #[serde(default)]
    pub(crate) resets_at: i64,
}

fn epoch(secs: i64) -> Option<SystemTime> {
    if secs > 0 {
        Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
    } else {
        None
    }
}

struct ThreadRow {
    id: String,
    model: String,
    cwd: String,
    rollout_path: String,
}

#[derive(Deserialize)]
struct SessionMetaLine {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    payload: Option<SessionMeta>,
}

#[derive(Deserialize, Default)]
struct SessionMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    cwd: String,
}

/// Codex 0.151+ no longer keeps the `threads` table in state_5.sqlite.  The
/// canonical rollout files still live under ~/.codex/sessions/YYYY/MM/DD, so
/// discover the newest ones directly and read their session_meta first line.
fn recent_session_rollouts(root: &Path) -> Vec<ThreadRow> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("rollout-"))
            {
                out.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    paths.reverse();

    paths
        .into_iter()
        .take(5)
        .filter_map(|path| {
            let first = BufReader::new(File::open(&path).ok()?)
                .lines()
                .next()?
                .ok()?;
            let line: SessionMetaLine = serde_json::from_str(&first).ok()?;
            if line.kind != "session_meta" {
                return None;
            }
            let meta = line.payload?;
            if meta.id.is_empty() || meta.cwd.is_empty() {
                return None;
            }
            Some(ThreadRow {
                id: meta.id,
                model: "codex".to_string(),
                cwd: meta.cwd,
                rollout_path: path.to_string_lossy().into_owned(),
            })
        })
        .collect()
}

/// 최근 활성(archived=0) thread 몇 개의 rollout 경로를 sqlite에서 조회 (read-only WAL).
fn active_threads(db: &Path) -> Vec<ThreadRow> {
    let conn = match Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let _ = conn.busy_timeout(Duration::from_millis(500));
    let mut stmt = match conn.prepare(
        "SELECT id, COALESCE(model,''), cwd, rollout_path FROM threads \
         WHERE archived = 0 ORDER BY updated_at DESC LIMIT 5",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = stmt.query_map([], |r| {
        Ok(ThreadRow {
            id: r.get(0)?,
            model: r.get(1)?,
            cwd: r.get(2)?,
            rollout_path: r.get(3)?,
        })
    });
    match rows {
        Ok(it) => it.filter_map(|r| r.ok()).collect(),
        Err(_) => vec![],
    }
}

/// last_token_usage(턴 델타)를 in/cache/out으로 분해. input_tokens는 캐시 포함 총입력이라
/// 신규입력 = input - cached, cache_read = cached, out = output.
fn parse_counts(u: &Usage) -> TokenCounts {
    let cached = u.cached_input_tokens.max(0);
    let fresh_in = (u.input_tokens - cached).max(0);
    let cap = u32::MAX as i64;
    TokenCounts {
        tokens_in: fresh_in.min(cap) as u32,
        tokens_out: u.output_tokens.max(0).min(cap) as u32,
        tokens_cache_read: cached.min(cap) as u32,
        tokens_cache_create: 0,
    }
}

fn apply_window(w: &Window, quota: &CodexQuota, is_secondary: bool) {
    // window_minutes: 10080(7일), 300(5시간).
    // window_minutes가 명시되어 있지 않은 경우 primary는 5h, secondary는 weekly로 기본 폴백.
    let is_weekly = match w.window_minutes {
        Some(m) => m > 360,
        None => is_secondary,
    };

    if is_weekly {
        *quota.used_pct_weekly.lock().unwrap() = Some(w.used_percent as f32);
        *quota.reset_weekly.lock().unwrap() = epoch(w.resets_at);
    } else {
        *quota.used_pct_5h.lock().unwrap() = Some(w.used_percent as f32);
        *quota.reset_5h.lock().unwrap() = epoch(w.resets_at);
    }
}

pub(crate) fn apply_rate_limits(rl: &RateLimits, quota: &CodexQuota) {
    if let Some(p) = &rl.primary {
        apply_window(p, quota, false);
    }
    if let Some(s) = &rl.secondary {
        apply_window(s, quota, true);
    }
    // **last_error 는 여기서 건드리지 않는다.** 이 함수는 rollout tail 도 부르는데,
    // rollout 의 rate_limits 는 과거 턴이 남긴 값이라 "지금 조회가 되는지"에 대해
    // 아무것도 증명하지 못한다. 그걸로 에러를 지우면 방금 실패한 능동 조회의
    // 메시지가 낡은 로그 한 줄에 덮여 사라진다. 지우는 건 조회 성공뿐이다
    // (codex_rpc::poll_rate_limits).
}

/// rollout을 offset부터 tail. replay_window=true(파일 최초 인지)면 5h 이내 token_count만
/// emit해 과거 폭주를 막고, rate_limits는 항상 최신으로 갱신한다. 반환: 새 offset.
fn tail_rollout(
    path: &Path,
    offset: u64,
    t: &ThreadRow,
    tx: &mpsc::UnboundedSender<TokenEvent>,
    quota: &CodexQuota,
    replay_window: bool,
) -> u64 {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return offset,
    };
    let file_len = f.metadata().map(|m| m.len()).unwrap_or(offset);
    let eff = if file_len < offset { 0 } else { offset };
    if f.seek(SeekFrom::Start(eff)).is_err() {
        return offset;
    }
    let mut reader = BufReader::new(&f);
    let mut new_off = eff;
    let mut buf = Vec::new();
    let cutoff = SystemTime::now().checked_sub(FIVE_H).unwrap_or(UNIX_EPOCH);
    loop {
        buf.clear();
        let n = match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if !buf.ends_with(b"\n") {
            break; // 미완 줄 — 다음 폴링까지 보류
        }
        new_off += n as u64;
        let line = String::from_utf8_lossy(&buf);
        let parsed: Line = match serde_json::from_str(line.trim_end()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(p) = parsed.payload else { continue };
        if p.kind != "token_count" {
            continue;
        }
        if let Some(rl) = &p.rate_limits {
            apply_rate_limits(rl, quota);
        }
        let Some(info) = p.info else { continue };
        let ts = parsed
            .timestamp
            .as_deref()
            .and_then(parse_iso8601)
            .unwrap_or_else(SystemTime::now);
        if replay_window && ts < cutoff {
            continue; // 시작 시 5h 밖 과거 턴은 집계에서 제외 (rate_limits만 반영)
        }
        let counts = parse_counts(&info.last_token_usage);
        if counts.total() == 0 {
            continue;
        }
        let _ = tx.send(TokenEvent {
            agent: AgentKind::Codex,
            project_path: PathBuf::from(&t.cwd),
            session_id: t.id.clone(),
            model: t.model.clone(),
            ts,
            counts,
            prompt_preview: None,
        });
    }
    new_off
}

pub struct CodexWatcher;

impl CodexWatcher {
    pub fn spawn(
        state_db: PathBuf,
        tx: mpsc::UnboundedSender<TokenEvent>,
        quota: Arc<CodexQuota>,
    ) -> Result<()> {
        if !state_db.exists() {
            tracing::warn!(?state_db, "codex db 없음 — Codex 미설치?");
            return Ok(());
        }
        std::thread::spawn(move || {
            let mut offsets: HashMap<PathBuf, u64> = HashMap::new();
            let sessions_root = state_db.parent().unwrap_or(Path::new(".")).join("sessions");
            loop {
                let mut threads = active_threads(&state_db);
                if threads.is_empty() {
                    threads = recent_session_rollouts(&sessions_root);
                }
                for t in threads {
                    let path = PathBuf::from(&t.rollout_path);
                    if !path.exists() {
                        continue;
                    }
                    let known = offsets.contains_key(&path);
                    let off = *offsets.get(&path).unwrap_or(&0);
                    let new_off = tail_rollout(&path, off, &t, &tx, &quota, !known);
                    offsets.insert(path, new_off);
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

    #[test]
    fn parse_counts_splits_cached_input_and_output() {
        let u = Usage {
            input_tokens: 21853,
            cached_input_tokens: 4480,
            output_tokens: 238,
        };
        let c = parse_counts(&u);
        assert_eq!(c.tokens_cache_read, 4480);
        assert_eq!(c.tokens_in, 21853 - 4480);
        assert_eq!(c.tokens_out, 238);
        assert_eq!(c.total(), 21853 + 238); // fresh_in + cached + out == input + out
    }

    #[test]
    fn rate_limits_populate_quota() {
        let q = CodexQuota::default();
        let rl = RateLimits {
            primary: Some(Window {
                used_percent: 31.0,
                window_minutes: Some(300),
                resets_at: 1_780_158_830,
            }),
            secondary: Some(Window {
                used_percent: 60.0,
                window_minutes: Some(10080),
                resets_at: 1_780_378_930,
            }),
        };
        apply_rate_limits(&rl, &q);
        assert_eq!(*q.used_pct_5h.lock().unwrap(), Some(31.0));
        assert_eq!(*q.used_pct_weekly.lock().unwrap(), Some(60.0));
        assert!(q.reset_5h.lock().unwrap().is_some());
        assert!(q.reset_weekly.lock().unwrap().is_some());
    }

    #[test]
    fn rate_limits_primary_weekly_window() {
        let q = CodexQuota::default();
        let rl = RateLimits {
            primary: Some(Window {
                used_percent: 90.0,
                window_minutes: Some(10080),
                resets_at: 1_787_203_953,
            }),
            secondary: None,
        };
        apply_rate_limits(&rl, &q);
        assert_eq!(*q.used_pct_5h.lock().unwrap(), None);
        assert_eq!(*q.used_pct_weekly.lock().unwrap(), Some(90.0));
        assert!(q.reset_5h.lock().unwrap().is_none());
        assert!(q.reset_weekly.lock().unwrap().is_some());
    }
}
