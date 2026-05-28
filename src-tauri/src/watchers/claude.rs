use crate::types::{AgentKind, TokenCounts, TokenEvent};
use anyhow::{anyhow, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Outer<'a> {
    #[serde(rename = "type")] kind: &'a str,
    #[serde(default)] message: Option<Message<'a>>,
    #[serde(default)] timestamp: Option<&'a str>,
    #[serde(default, rename = "sessionId")] session_id: Option<&'a str>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(default)] model: Option<&'a str>,
    #[serde(default)] usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)] input_tokens: u32,
    #[serde(default)] output_tokens: u32,
    #[serde(default)] cache_creation_input_tokens: u32,
    #[serde(default)] cache_read_input_tokens: u32,
}

pub fn parse_line(line: &str, project_path: &Path, fallback_session: &str) -> Result<Option<TokenEvent>> {
    if line.trim().is_empty() { return Ok(None); }
    let outer: Outer = serde_json::from_str(line).map_err(|e| anyhow!("json: {e}"))?;
    if outer.kind != "assistant" { return Ok(None); }
    let msg = outer.message.ok_or_else(|| anyhow!("no message"))?;
    let usage = match msg.usage { Some(u) => u, None => return Ok(None) };
    let model = msg.model.unwrap_or("unknown").to_string();
    let session_id = outer.session_id.unwrap_or(fallback_session).to_string();
    let ts = outer.timestamp.and_then(parse_iso8601).unwrap_or_else(SystemTime::now);
    Ok(Some(TokenEvent {
        agent: AgentKind::Claude,
        project_path: project_path.to_path_buf(),
        session_id,
        model,
        ts,
        counts: TokenCounts {
            tokens_in: usage.input_tokens,
            tokens_out: usage.output_tokens,
            tokens_cache_read: usage.cache_read_input_tokens,
            tokens_cache_create: usage.cache_creation_input_tokens,
        },
    }))
}

fn parse_iso8601(s: &str) -> Option<SystemTime> {
    let mut iter = s.splitn(2, 'T');
    let date = iter.next()?;
    let rest = iter.next()?.trim_end_matches('Z');
    let mut d = date.splitn(3, '-');
    let yr: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let dy: u32 = d.next()?.parse().ok()?;
    let mut t = rest.splitn(3, ':');
    let hh: u32 = t.next()?.parse().ok()?;
    let mm: u32 = t.next()?.parse().ok()?;
    let sec_part = t.next()?;
    let (ss_int, ss_frac) = sec_part.split_once('.').unwrap_or((sec_part, "0"));
    let ss: u32 = ss_int.parse().ok()?;
    let frac_ms: u32 = ss_frac.chars().take(3).collect::<String>().parse().ok()?;
    let days = days_since_epoch(yr, mo, dy)?;
    let secs = days as u64 * 86400 + hh as u64 * 3600 + mm as u64 * 60 + ss as u64;
    Some(UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(frac_ms as u64))
}

fn days_since_epoch(year: i64, month: u32, day: u32) -> Option<i64> {
    let m = (month as i64 + 9) % 12;
    let y = year - m / 10;
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + (day as i64 - 1);
    Some(days - 719468)
}

/// Claude 프로젝트 디렉토리 이름을 실제 경로로 복원한다.
/// Claude는 경로의 "/" 를 "-" 로 치환하고, 첫 문자가 "-" 이면 절대경로(루트 `/`)를 의미한다.
/// 주의: "-" 로 치환된 디렉토리 구분자와 원래 이름 안의 "-" 는 구별 불가 (손실 변환).
/// "--" 패턴은 "/@" 가 있던 경우를 복원하는 휴리스틱이다.
pub fn decode_project_path(claude_project_dir: &Path) -> PathBuf {
    let name = claude_project_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let decoded = if let Some(rest) = name.strip_prefix('-') {
        format!("/{}", rest.replace("--", "/@").replace('-', "/"))
    } else {
        name.replace('-', "/")
    };
    PathBuf::from(decoded)
}

pub struct ClaudeWatcher;

impl ClaudeWatcher {
    /// projects_root: 보통 ~/.claude/projects
    pub fn spawn(projects_root: PathBuf, tx: mpsc::UnboundedSender<TokenEvent>) -> Result<()> {
        std::thread::spawn(move || {
            if !projects_root.exists() {
                tracing::warn!(?projects_root, "claude projects dir 없음 — Claude 미설치?");
                return;
            }
            let mut offsets: HashMap<PathBuf, u64> = HashMap::new();
            let (notify_tx, notify_rx) = std_mpsc::channel::<notify::Result<Event>>();
            let mut watcher = match notify::recommended_watcher(move |res| { let _ = notify_tx.send(res); }) {
                Ok(w) => w,
                Err(e) => { tracing::error!(%e, "notify watcher init failed"); return; }
            };
            if let Err(e) = watcher.watch(&projects_root, RecursiveMode::Recursive) {
                tracing::error!(%e, "watch failed");
                return;
            }

            // 시작 시점에 기존 파일들의 offset = 파일 끝으로 (이미 본 것 무시)
            for entry in walk_jsonl(&projects_root) {
                if let Ok(meta) = std::fs::metadata(&entry) {
                    offsets.insert(entry, meta.len());
                }
            }

            // 5h 데이터 복원: 기존 파일들을 offset=0부터 다시 읽어 aggregator에 흘려넣음.
            // offset 캐시는 업데이트하지 않음 — FSEvents 후속 이벤트는 file end 기준으로만 잡힘.
            for entry in walk_jsonl(&projects_root) {
                let project_dir = entry.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let session_id = entry.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let decoded = decode_project_path(&project_dir);
                tail_file(&entry, 0, &decoded, &session_id, &tx);
            }

            loop {
                match notify_rx.recv() {
                    Ok(Ok(event)) => {
                        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            for path in event.paths {
                                if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                                    let project_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                                    let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                                    let decoded = decode_project_path(&project_dir);
                                    let offset = offsets.entry(path.clone()).or_insert(0);
                                    *offset = tail_file(&path, *offset, &decoded, &session_id, &tx);
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => tracing::warn!(?e, "notify error"),
                    Err(_) => break,
                }
            }
        });
        Ok(())
    }
}

fn walk_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Ok(files) = std::fs::read_dir(entry.path()) {
                    for f in files.flatten() {
                        let p = f.path();
                        if p.extension().map(|e| e == "jsonl").unwrap_or(false) { out.push(p); }
                    }
                }
            }
        }
    }
    out
}

fn tail_file(path: &Path, offset: u64, project: &Path, session_id: &str, tx: &mpsc::UnboundedSender<TokenEvent>) -> u64 {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return offset };
    let file_len = f.metadata().map(|m| m.len()).unwrap_or(offset);
    // 파일이 회전/잘림되어 현재 offset보다 작아진 경우, 처음부터 다시 읽음.
    let effective_offset = if file_len < offset { 0 } else { offset };
    if f.seek(SeekFrom::Start(effective_offset)).is_err() { return offset; }
    let reader = BufReader::new(&f);
    let mut new_offset = effective_offset;
    for line in reader.lines().map_while(|r| r.ok()) {
        new_offset += line.len() as u64 + 1;
        match parse_line(&line, project, session_id) {
            Ok(Some(ev)) => { let _ = tx.send(ev); }
            Ok(None) => {}
            Err(e) => tracing::debug!(?path, %e, "jsonl line parse error"),
        }
    }
    new_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentKind;
    use std::path::PathBuf;

    #[test]
    fn parses_assistant_usage_line() {
        let line = r#"{"type":"assistant","message":{"id":"m","role":"assistant","model":"claude-sonnet-4-6","content":[],"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}},"timestamp":"2026-05-28T08:30:02.000Z","sessionId":"s1"}"#;
        let project = PathBuf::from("/tmp/p");
        let ev = parse_line(line, &project, "s1").expect("should parse").expect("should be Some");
        assert_eq!(ev.agent, AgentKind::Claude);
        assert_eq!(ev.counts.tokens_in, 100);
        assert_eq!(ev.counts.tokens_out, 50);
        assert_eq!(ev.counts.tokens_cache_create, 10);
        assert_eq!(ev.counts.tokens_cache_read, 20);
        assert_eq!(ev.model, "claude-sonnet-4-6");
    }

    #[test]
    fn user_line_returns_none() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"2026-05-28T08:30:00.000Z","sessionId":"s"}"#;
        let project = PathBuf::from("/tmp/p");
        let r = parse_line(line, &project, "s").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn broken_json_returns_err() {
        let project = PathBuf::from("/tmp/p");
        let r = parse_line("not json", &project, "s");
        assert!(r.is_err());
    }

    #[test]
    fn project_dir_name_extracted_from_encoded_path() {
        // Claude는 "/" → "-", 밑줄은 보존. 입력: `-Users-me-Desktop--Projects-2_App`
        // strip '-': `Users-me-Desktop--Projects-2_App`
        // "--" → "/@": `Users-me-Desktop/@Projects-2_App`
        // "-" → "/": `Users/me/Desktop/@Projects/2_App`
        let dir = PathBuf::from("/Users/me/.claude/projects/-Users-me-Desktop--Projects-2_App");
        let pp = decode_project_path(&dir);
        assert!(
            pp.to_string_lossy().starts_with("/Users/me/Desktop"),
            "expected absolute Users path, got {:?}", pp
        );
    }

    #[test]
    fn fixture_file_yields_two_events() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/sample.jsonl");
        let lines = std::fs::read_to_string(&path).unwrap();
        let project = PathBuf::from("/tmp/p");
        let mut count = 0;
        for line in lines.lines() {
            if let Ok(Some(_)) = parse_line(line, &project, "abc") { count += 1; }
        }
        assert_eq!(count, 2);
    }
}
