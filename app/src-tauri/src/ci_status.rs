//! CIStatusBar 가 내보내는 `~/.config/ci-status/status.json` 의 `recentJobs` 를 읽는다.
//! 읽기 전용이다. 파일이 없거나 깨졌거나 오래됐으면 빈 목록이며 오류로 올리지 않는다
//! (CYD 화면은 "job 없음"으로 정상 동작해야 하고, 낡은 "진행중"을 계속 보여주면 안 된다).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiState {
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiJob {
    pub project: String,
    pub name: String,
    pub state: CiState,
    pub percent: Option<u8>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(rename = "intervalSeconds", default)]
    interval_seconds: f64,
    #[serde(rename = "recentJobs", default)]
    recent_jobs: Vec<RawJob>,
}

#[derive(Deserialize)]
struct RawJob {
    project: String,
    name: String,
    state: String,
    percent: Option<u8>,
    #[serde(rename = "startedAt")]
    started_at: u64,
    #[serde(rename = "finishedAt")]
    finished_at: Option<u64>,
}

/// 기본 경로 `~/.config/ci-status/status.json` (CIStatusBar 기본값과 같다).
pub fn default_path() -> Option<PathBuf> {
    // CIStatusBar 는 macOS 에서 `~/.config` 를 쓰므로 `config_dir()`(~/Library/...)가 아니라 홈 기준이다.
    dirs_next::home_dir().map(|h| h.join(".config/ci-status/status.json"))
}

/// 최대 4개. 신선도 기준은 파일 mtime(CIStatusBar 가 매 갱신마다 atomic write)이다:
/// `3 * intervalSeconds + 30`초보다 오래되면 빈 목록.
pub fn read_recent_jobs(path: &Path, now: SystemTime) -> Vec<CiJob> {
    let Ok(meta) = std::fs::metadata(path) else { return Vec::new() };
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let Ok(raw) = serde_json::from_slice::<RawFile>(&bytes) else { return Vec::new() };
    let interval = if raw.interval_seconds > 0.0 { raw.interval_seconds } else { 60.0 };
    let limit = Duration::from_secs_f64(3.0 * interval + 30.0);
    let age = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).unwrap_or_default();
    if age > limit {
        return Vec::new();
    }
    raw.recent_jobs
        .into_iter()
        .filter_map(|j| {
            let state = match j.state.as_str() {
                "running" => CiState::Running,
                "success" => CiState::Success,
                "failed" => CiState::Failed,
                _ => return None,
            };
            Some(CiJob {
                project: j.project,
                name: j.name,
                state,
                percent: j.percent,
                started_at: j.started_at,
                finished_at: j.finished_at,
            })
        })
        .take(4)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SAMPLE: &str = r#"{"updatedAt":"2026-11-09T01:20:47Z","intervalSeconds":10,"errors":[],"pipelines":[],
      "recentJobs":[
        {"project":"ios/seqnex","name":"build","state":"running","percent":60,"startedAt":1000},
        {"project":"ios/seqnex","name":"unit-test","state":"success","startedAt":700,"finishedAt":880},
        {"project":"web/api","name":"deploy","state":"failed","startedAt":500,"finishedAt":803},
        {"project":"web/api","name":"lint","state":"success","startedAt":300,"finishedAt":348},
        {"project":"web/api","name":"extra","state":"success","startedAt":100,"finishedAt":148}]}"#;

    fn tmp(name: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cistatus-test-{}-{name}.json", std::process::id()));
        std::fs::File::create(&p).unwrap().write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn reads_recent_jobs_from_sample() {
        let p = tmp("ok", SAMPLE);
        let jobs = read_recent_jobs(&p, SystemTime::now());
        assert_eq!(jobs.len(), 4, "최대 4개로 자른다");
        assert_eq!(jobs[0].name, "build");
        assert_eq!(jobs[0].state, CiState::Running);
        assert_eq!(jobs[0].percent, Some(60));
        assert_eq!(jobs[1].finished_at, Some(880));
        assert_eq!(jobs[2].state, CiState::Failed);
    }

    #[test]
    fn stale_status_file_yields_no_jobs() {
        let p = tmp("stale", SAMPLE); // intervalSeconds=10 → 한도 60초
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(600)).unwrap();
        assert!(read_recent_jobs(&p, SystemTime::now()).is_empty());
    }

    #[test]
    fn missing_or_corrupt_file_yields_no_jobs() {
        assert!(read_recent_jobs(Path::new("/nonexistent/status.json"), SystemTime::now()).is_empty());
        let p = tmp("corrupt", r#"{"recentJobs":[{"project":"a""#);
        assert!(read_recent_jobs(&p, SystemTime::now()).is_empty());
    }

    #[test]
    fn unknown_state_is_dropped_not_fatal() {
        let p = tmp("unk", r#"{"intervalSeconds":10,"recentJobs":[
          {"project":"a/b","name":"x","state":"weird","startedAt":1},
          {"project":"a/b","name":"y","state":"success","startedAt":1,"finishedAt":2}]}"#);
        let jobs = read_recent_jobs(&p, SystemTime::now());
        assert_eq!(jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["y"]);
    }
}
