//! Antigravity watcher — ~/.gemini/antigravity-cli/conversations/*.db (SQLite)의
//! gen_metadata 및 steps 테이블을 tail하여 턴별 토큰 사용량과 quota 한도 정보를 수집한다.

use crate::types::{AgentKind, TokenCounts, TokenEvent};
use crate::watchers::claude::parse_iso8601;
use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc as std_mpsc;
use std::sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const FIVE_H: Duration = Duration::from_secs(5 * 3600);

/// Antigravity 서버가 보고한 Gemini 계정 사용량.
#[derive(Default)]
pub struct AntigravityQuota {
    pub used_pct_5h: Mutex<Option<f32>>,
    pub reset_5h: Mutex<Option<SystemTime>>,
    pub used_pct_weekly: Mutex<Option<f32>>,
    pub reset_weekly: Mutex<Option<SystemTime>>,
}

#[derive(Debug, PartialEq)]
struct UsageLimits {
    used_pct_5h: f32,
    reset_5h: SystemTime,
    used_pct_weekly: f32,
    reset_weekly: SystemTime,
}

/// `agy -p /usage` 출력 중 Gemini Models의 두 계정 quota만 읽는다.
/// 대화별 context window(256k)는 계정 quota가 아니므로 여기서 절대 사용하지 않는다.
fn parse_usage_limits(output: &str) -> Option<UsageLimits> {
    let mut five_hour = None;
    let mut weekly = None;

    for line in output.lines().map(str::trim) {
        if !line.starts_with("Gemini Models") {
            continue;
        }
        let remaining = line
            .split_whitespace()
            .find_map(|part| part.strip_suffix('%'))?
            .parse::<f32>()
            .ok()?;
        let reset_at = parse_iso8601(line.split_whitespace().last()?)?;
        let used = (100.0 - remaining).clamp(0.0, 100.0);

        if line.contains("Five Hour Limit Remaining") {
            five_hour = Some((used, reset_at));
        } else if line.contains("Weekly Limit Remaining") {
            weekly = Some((used, reset_at));
        }
    }

    let ((used_pct_5h, reset_5h), (used_pct_weekly, reset_weekly)) = (five_hour?, weekly?);
    Some(UsageLimits { used_pct_5h, reset_5h, used_pct_weekly, reset_weekly })
}

pub(crate) fn poll_usage(quota: &AntigravityQuota) {
    // Finder/LaunchAgent에서 띄운 macOS 앱은 shell PATH를 물려받지 않을 수 있다.
    let agy_path = dirs_next::home_dir()
        .map(|home| home.join(".local/bin/agy"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("agy"));
    let output = match Command::new(agy_path).args(["-p", "/usage"]).output() {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            tracing::warn!(status = ?output.status, "antigravity /usage 명령 실패");
            return;
        }
        Err(e) => {
            tracing::warn!(%e, "antigravity /usage 실행 실패");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(limits) = parse_usage_limits(&stdout) else {
        tracing::warn!("antigravity /usage 출력에서 Gemini quota를 찾지 못함");
        return;
    };
    *quota.used_pct_5h.lock().unwrap() = Some(limits.used_pct_5h);
    *quota.reset_5h.lock().unwrap() = Some(limits.reset_5h);
    *quota.used_pct_weekly.lock().unwrap() = Some(limits.used_pct_weekly);
    *quota.reset_weekly.lock().unwrap() = Some(limits.reset_weekly);
}

fn spawn_usage_poller(quota: Arc<AntigravityQuota>, interval_secs: Arc<AtomicU64>) {
    std::thread::spawn(move || loop {
        poll_usage(&quota);
        std::thread::sleep(Duration::from_secs(interval_secs.load(Ordering::Relaxed).max(60)));
    });
}

// ── Zero-dependency Protobuf Parser ─────────────────────────────

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut val: u64 = 0;
    let mut shift: u32 = 0;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        val |= ((b & 0x7f) as u64) << shift;
        shift += 7;
        if (b & 0x80) == 0 {
            return Some(val);
        }
        if shift >= 64 {
            return None;
        }
    }
    None
}

fn skip_field(wire_type: u32, buf: &[u8], pos: &mut usize) -> bool {
    match wire_type {
        0 => read_varint(buf, pos).is_some(),
        1 => {
            if *pos + 8 <= buf.len() {
                *pos += 8;
                true
            } else {
                false
            }
        }
        2 => {
            if let Some(len) = read_varint(buf, pos) {
                let len = len as usize;
                if *pos + len <= buf.len() {
                    *pos += len;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        5 => {
            if *pos + 4 <= buf.len() {
                *pos += 4;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedTurn {
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub tokens_cache_read: u32,
    pub last_step_index: Option<i64>,
}

/// `gen_metadata`의 Protobuf BLOB 파싱
pub fn parse_gen_metadata(buf: &[u8]) -> Option<ParsedTurn> {
    let mut turn = ParsedTurn::default();
    let mut pos = 0;

    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u32;

        if wire_type == 2 {
            let len = read_varint(buf, &mut pos)? as usize;
            if pos + len > buf.len() {
                return None;
            }
            let sub_slice = &buf[pos..pos + len];
            pos += len;

            if field_num == 1 {
                // F1: Turn Execution Metadata
                parse_turn_execution_msg(sub_slice, &mut turn);
            }
        } else if !skip_field(wire_type, buf, &mut pos) {
            return None;
        }
    }

    if turn.tokens_in > 0 || turn.tokens_out > 0 || !turn.model.is_empty() {
        Some(turn)
    } else {
        None
    }
}

fn parse_turn_execution_msg(buf: &[u8], turn: &mut ParsedTurn) {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = match read_varint(buf, &mut pos) {
            Some(t) => t,
            None => break,
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u32;

        if wire_type == 2 {
            let len = match read_varint(buf, &mut pos) {
                Some(l) => l as usize,
                None => break,
            };
            if pos + len > buf.len() {
                break;
            }
            let sub_slice = &buf[pos..pos + len];
            pos += len;

            match field_num {
                19 => {
                    if let Ok(m) = std::str::from_utf8(sub_slice) {
                        turn.model = m.to_string();
                    }
                }
                4 | 17 => {
                    // Token Usage submessage
                    parse_token_usage_container(sub_slice, turn);
                }
                20 => {
                    // Key-Value metadata pair (F1: key, F2: val)
                    let mut k_pos = 0;
                    let mut key = "";
                    let mut val = "";
                    while k_pos < sub_slice.len() {
                        let k_tag = match read_varint(sub_slice, &mut k_pos) {
                            Some(t) => t,
                            None => break,
                        };
                        let k_fn = (k_tag >> 3) as u32;
                        let k_wt = (k_tag & 0x7) as u32;
                        if k_wt == 2 {
                            let k_len = match read_varint(sub_slice, &mut k_pos) {
                                Some(l) => l as usize,
                                None => break,
                            };
                            if k_pos + k_len > sub_slice.len() {
                                break;
                            }
                            let s = std::str::from_utf8(&sub_slice[k_pos..k_pos + k_len]).unwrap_or("");
                            k_pos += k_len;
                            if k_fn == 1 {
                                key = s;
                            } else if k_fn == 2 {
                                val = s;
                            }
                        } else if !skip_field(k_wt, sub_slice, &mut k_pos) {
                            break;
                        }
                    }
                    if key == "last_step_index" {
                        if let Ok(s_idx) = val.parse::<i64>() {
                            turn.last_step_index = Some(s_idx);
                        }
                    }
                }
                _ => {}
            }
        } else if !skip_field(wire_type, buf, &mut pos) {
            break;
        }
    }
}

fn parse_token_usage_container(buf: &[u8], turn: &mut ParsedTurn) {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = match read_varint(buf, &mut pos) {
            Some(t) => t,
            None => break,
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u32;

        if wire_type == 0 {
            let v = match read_varint(buf, &mut pos) {
                Some(v) => v as u32,
                None => break,
            };
            match field_num {
                1 => turn.tokens_out = v,
                2 => turn.tokens_in = v,
                3 => turn.tokens_cache_read = v,
                _ => {}
            }
        } else if wire_type == 2 {
            let len = match read_varint(buf, &mut pos) {
                Some(l) => l as usize,
                None => break,
            };
            if pos + len > buf.len() {
                break;
            }
            let sub = &buf[pos..pos + len];
            pos += len;

            if field_num == 2 {
                // Nested token usage: F17 -> F2 -> F1(out), F2(in), F3(cache)
                parse_token_usage_container(sub, turn);
            }
        } else if !skip_field(wire_type, buf, &mut pos) {
            break;
        }
    }
}

/// `steps.metadata`의 Protobuf BLOB에서 타임스탬프 추출 (F1.F1: epoch seconds, F1.F2: nanos)
pub fn parse_step_timestamp(buf: &[u8]) -> Option<SystemTime> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos)?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x7) as u32;

        if wire_type == 2 {
            let len = read_varint(buf, &mut pos)? as usize;
            if pos + len > buf.len() {
                return None;
            }
            let sub = &buf[pos..pos + len];
            pos += len;

            if field_num == 1 {
                let mut sub_pos = 0;
                let mut secs: Option<u64> = None;
                let mut nanos: u32 = 0;
                while sub_pos < sub.len() {
                    let stag = read_varint(sub, &mut sub_pos)?;
                    let sf = (stag >> 3) as u32;
                    let sw = (stag & 0x7) as u32;
                    if sw == 0 {
                        let val = read_varint(sub, &mut sub_pos)?;
                        match sf {
                            1 => secs = Some(val),
                            2 => nanos = (val % 1_000_000_000) as u32,
                            _ => {}
                        }
                    } else if !skip_field(sw, sub, &mut sub_pos) {
                        break;
                    }
                }
                if let Some(s) = secs {
                    return Some(UNIX_EPOCH + Duration::from_secs(s) + Duration::from_nanos(nanos as u64));
                }
            }
        } else if !skip_field(wire_type, buf, &mut pos) {
            return None;
        }
    }
    None
}

/// `file:///` 형식의 URI 문자열을 디코딩하여 로컬 PathBuf로 변환
pub fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let path_str = uri.strip_prefix("file://")?;
    let mut decoded = String::with_capacity(path_str.len());
    let mut chars = path_str.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    decoded.push(byte as char);
                    continue;
                }
            }
            decoded.push('%');
            decoded.push_str(&hex);
        } else {
            decoded.push(c);
        }
    }
    Some(PathBuf::from(decoded))
}

/// `trajectory_metadata_blob`에서 workspace URI 추출
fn extract_workspace_from_blob(blob: &[u8]) -> Option<PathBuf> {
    let pattern = b"file:///";
    if let Some(idx) = blob.windows(pattern.len()).position(|window| window == pattern) {
        let rest = &blob[idx..];
        let len = rest
            .iter()
            .position(|&b| b < 32 || b == b'"' || b == b'\'' || b == b'\0' || b == b'\n' || b == b'\r')
            .unwrap_or(rest.len());
        if let Ok(uri_str) = std::str::from_utf8(&rest[..len]) {
            return decode_file_uri(uri_str);
        }
    }
    None
}

// ── SQLite Scanner ──────────────────────────────────────────────

fn open_readonly_db(db_path: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(500));
    Some(conn)
}

/// 세션 DB에서 workspace 경로 조회
fn get_session_project_path(conn: &Connection) -> Option<PathBuf> {
    let mut stmt = conn
        .prepare("SELECT data FROM trajectory_metadata_blob WHERE id = 'main' LIMIT 1")
        .ok()?;
    let row: Result<Vec<u8>, _> = stmt.query_row([], |r| r.get(0));
    if let Ok(blob) = row {
        if let Some(path) = extract_workspace_from_blob(&blob) {
            return Some(path);
        }
    }
    None
}

/// conversation_summaries.db에서 workspace 경로 조회
fn get_project_from_summaries(summaries_db: &Path, conversation_id: &str) -> Option<PathBuf> {
    let conn = open_readonly_db(summaries_db)?;
    let mut stmt = conn
        .prepare("SELECT workspace_uris FROM conversation_summaries WHERE conversation_id = ?1 LIMIT 1")
        .ok()?;
    let uris_json: Result<String, _> = stmt.query_row([conversation_id], |r| r.get(0));
    if let Ok(json) = uris_json {
        if let Ok(uris) = serde_json::from_str::<Vec<String>>(&json) {
            if let Some(first) = uris.first() {
                return decode_file_uri(first);
            }
        }
    }
    None
}

/// 단일 conversation DB의 새 `gen_metadata` row들을 읽어 TokenEvent 생성
fn tail_conversation_db(
    db_path: &Path,
    summaries_db: &Path,
    last_idx: i64,
    tx: &mpsc::UnboundedSender<TokenEvent>,
) -> i64 {
    let conn = match open_readonly_db(db_path) {
        Some(c) => c,
        None => return last_idx,
    };

    let session_id = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let project_path = get_session_project_path(&conn)
        .or_else(|| get_project_from_summaries(summaries_db, &session_id))
        .unwrap_or_else(|| PathBuf::from("/"));

    let mut stmt = match conn.prepare(
        "SELECT idx, data FROM gen_metadata WHERE idx > ?1 ORDER BY idx ASC",
    ) {
        Ok(s) => s,
        Err(_) => return last_idx,
    };

    let mut step_ts_stmt = conn
        .prepare("SELECT metadata FROM steps WHERE idx = ?1 LIMIT 1")
        .ok();

    let rows = stmt.query_map([last_idx], |r| {
        let idx: i64 = r.get(0)?;
        let data: Vec<u8> = r.get(1)?;
        Ok((idx, data))
    });

    let mut new_last_idx = last_idx;
    let now = SystemTime::now();

    if let Ok(iter) = rows {
        for item in iter.flatten() {
            let (idx, data) = item;
            new_last_idx = new_last_idx.max(idx);

            if let Some(turn) = parse_gen_metadata(&data) {
                let mut ts = now;
                let step_idx_to_query = turn.last_step_index.unwrap_or(idx);
                if let Some(ref mut s_stmt) = step_ts_stmt {
                    if let Ok(meta) = s_stmt.query_row([step_idx_to_query], |r| r.get::<_, Option<Vec<u8>>>(0)) {
                        if let Some(meta_blob) = meta {
                            if let Some(step_ts) = parse_step_timestamp(&meta_blob) {
                                ts = step_ts;
                            }
                        }
                    }
                }
                if last_idx >= 0 && ts > now {
                    ts = now;
                }

                if now.duration_since(ts).unwrap_or_default() <= FIVE_H {
                    let ev = TokenEvent {
                        agent: AgentKind::Antigravity,
                        project_path: project_path.clone(),
                        session_id: session_id.clone(),
                        model: turn.model,
                        ts,
                        counts: TokenCounts {
                            tokens_in: turn.tokens_in,
                            tokens_out: turn.tokens_out,
                            tokens_cache_read: turn.tokens_cache_read,
                            tokens_cache_create: 0,
                        },
                    };
                    let _ = tx.send(ev);
                }
            }
        }
    }

    new_last_idx
}

// ── Watcher Spawn ───────────────────────────────────────────────

pub struct AntigravityWatcher;

impl AntigravityWatcher {
    pub fn spawn(
        conversations_root: PathBuf,
        summaries_db: PathBuf,
        tx: mpsc::UnboundedSender<TokenEvent>,
        quota: Arc<AntigravityQuota>,
        quota_poll_interval_secs: Arc<AtomicU64>,
    ) -> Result<()> {
        spawn_usage_poller(quota, quota_poll_interval_secs);
        std::thread::spawn(move || {
            if !conversations_root.exists() {
                tracing::warn!(?conversations_root, "antigravity conversations dir 없음");
                return;
            }

            let mut last_indices: HashMap<PathBuf, i64> = HashMap::new();

            if let Ok(entries) = std::fs::read_dir(&conversations_root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "db").unwrap_or(false) {
                        let last_idx = tail_conversation_db(&path, &summaries_db, -1, &tx);
                        last_indices.insert(path, last_idx);
                    }
                }
            }

            let (notify_tx, notify_rx) = std_mpsc::channel::<notify::Result<Event>>();
            let mut watcher = match notify::recommended_watcher(move |res| {
                let _ = notify_tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(%e, "notify watcher init failed for antigravity");
                    return;
                }
            };

            if let Err(e) = watcher.watch(&conversations_root, RecursiveMode::NonRecursive) {
                tracing::error!(%e, "antigravity conversations watch failed");
                return;
            }

            loop {
                match notify_rx.recv() {
                    Ok(Ok(event)) => {
                        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            for path in event.paths {
                                let is_db = path.extension().map(|e| e == "db").unwrap_or(false);
                                let is_wal = path.file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.ends_with(".db-wal"))
                                    .unwrap_or(false);

                                let target_db = if is_db {
                                    Some(path)
                                } else if is_wal {
                                    let db_name = path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .map(|s| s.trim_end_matches("-wal"))
                                        .map(PathBuf::from);
                                    db_name.map(|n| path.parent().unwrap_or(Path::new("")).join(n))
                                } else {
                                    None
                                };

                                if let Some(db) = target_db {
                                    if db.exists() {
                                        let last = last_indices.entry(db.clone()).or_insert(-1);
                                        let new_idx = tail_conversation_db(&db, &summaries_db, *last, &tx);
                                        *last = new_idx;
                                    }
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(%e, "antigravity watcher event error");
                    }
                    Err(_) => {
                        tracing::info!("antigravity watcher exit");
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_file_uri() {
        let uri = "file:///Users/wannypark/Desktop/%40Projects/2_App";
        let path = decode_file_uri(uri).unwrap();
        assert_eq!(path, PathBuf::from("/Users/wannypark/Desktop/@Projects/2_App"));
    }

    #[test]
    fn test_parse_gen_metadata_hex() {
        let hex = "12020203222439353732613763312d303035372d346263642d383666312d63643433366461646562376142b003889cb630a5c5c930f0b18f31f6b18f31fbcff23182d0f23193e3f431c1c8fd31bab28332bfb28332e2f78332edf78332acc4ae32b3c4ae32d0edb032d3edb032f090b332f290b332d4f9b632d4d2b832e8edba32eaedba32a1f5ba32a6f5ba3282c5bc3283ffbc32cdd1bd32cfd1bd328fb0be32c3ecbe32cfecbe32d1ecbe3294abbf32edbabf32bf82c0328ea5c23296a5c232fdbcc4328abdc4328bbdc432a0bdc432a1bdc432b6bdc432bbbdc432e0d4c632e5d4c632f7d4c632b886c732add9c732afd9c7328987c8329087c8329487c83293bcc8329cd1c9329ed1c932d5b3ca32a5c0cb3291efcb32aeefcb32889fcc32d5d6cc32ecd6cc32b791cd32b991cd328493cd328793cd32cda2cd32cfa2cd32f2a5ce32d4c1ce3298d8d232f3a9d332e4d5d332d4efd532cfddd632dff7d632de84d732e284d73283e5d832fefcdc32aad4dd32acd4dd32b1d4dd32b3d4dd3290f5dd3296f5dd32e9fade32ebfade32d2efdf32d7dee132d9dee132bbb7e232bfb7e23281d9e23283d9e23299b8e3329db8e332d2fde432d4cfe632f69de7328dade83291ade832c6b5e832c4b8e832f1d2e932a382ea32bb87ea320acf0418920a227708920a10cd870118be0230183a28626f742d63653739366236632d636331632d346237372d613161302d62386261353462366239313142210a0973657373696f6e494412142d33373530373633303334333632383935353739488302503b5a174f346d47616f544f4b72765231653850714b4f6779516b4a1510ffffffffffffffffff015208089cbe012080d00f5a08080210d2bba787036205108d82f25e8a018b01127708920a10cd870118be0230183a28626f742d63653739366236632d636331632d346237372d613161302d62386261353462366239313142210a0973657373696f6e494412142d33373530373633303334333632383935353739488302503b5a174f346d47616f544f4b72765231653850714b4f6779516b2210626636363638323263376664643031649a011067656d696e692d332e372d666c617368a201240a0a6d6f64656c5f656e756d12164d4f44454c5f504c414345484f4c4445525f4d323938a201350a0d7472616a6563746f72795f6964122466383465663135642d393933342d346363312d613661302d376637656433396562613361a201340a0a726571756573745f6964122666383465663135642d393933342d346363312d613661302d3766376564333965626133612d30a201140a0b757365645f636c61756465120566616c7365a201210a18757365645f636c617564655f636f6e736572766174697665120566616c7365a2011e0a15757365645f6e6f6e5f67656d696e695f6d6f64656c120566616c7365a201140a0f6c6173745f737465705f696e646578120131";
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<u8>>();

        let turn = parse_gen_metadata(&bytes).expect("Turn should be parsed");
        assert_eq!(turn.model, "gemini-3.7-flash");
        assert_eq!(turn.tokens_out, 1298);
        assert_eq!(turn.tokens_in, 17357);
        assert_eq!(turn.tokens_cache_read, 318);
        assert_eq!(turn.last_step_index, Some(1));
    }

    #[test]
    fn usage_output_uses_only_gemini_account_limits() {
        let output = "Gemini Models\tWeekly Limit Remaining\t99%\t2026-09-04T00:16:29Z\n\
Gemini Models\tFive Hour Limit Remaining\t95%\t2026-08-28T05:16:29Z\n\
Claude and GPT models\tWeekly Limit Remaining\t100%\t2026-09-04T00:25:14Z\n\
Claude and GPT models\tFive Hour Limit Remaining\t100%\t2026-08-28T05:25:14Z";

        let limits = parse_usage_limits(output).expect("Gemini limits should parse");
        assert_eq!(limits.used_pct_5h, 5.0);
        assert_eq!(limits.used_pct_weekly, 1.0);
        assert_eq!(limits.reset_5h, parse_iso8601("2026-08-28T05:16:29Z").unwrap());
        assert_eq!(limits.reset_weekly, parse_iso8601("2026-09-04T00:16:29Z").unwrap());
    }

    #[test]
    fn usage_output_requires_both_gemini_windows() {
        let output = "Gemini Models\tFive Hour Limit Remaining\t95%\t2026-08-28T05:16:29Z";
        assert_eq!(parse_usage_limits(output), None);
    }

    #[test]
    fn test_parse_step_timestamp_hex() {
        let hex = "0a0c08b8929ad40610d0b0b990011804622439353732613763312d303035372d346263642d383666312d636434333664616465623761a2014c0a2466383465663135642d393933342d346363312d613661302d376637656433396562613361222462613766633834372d303066662d343064322d393230342d666530653032326536353964d201120a100803120c08b8929ad40610e0d198a001";
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<u8>>();

        let ts = parse_step_timestamp(&bytes).expect("Timestamp should be parsed");
        let dur = ts.duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(dur.as_secs(), 1787201848);
    }
}
