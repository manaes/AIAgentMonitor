//! TestHub `GET /api/automation` 을 폴링해 **진행 중인 테스트 단계**를 CYD `j[]` 용 `CiJob`(running)으로 바꾼다.
//! 읽기 전용이다. URL 은 `~/.config/ci-status/testhub.url` 한 줄(예: `http://host:8090`)이며, 파일이 없으면 꺼진다
//! (사내 주소를 저장소에 넣지 않기 위함). 실패·미진행이면 빈 목록이며 오류로 올리지 않는다.
use crate::ci_status::{CiJob, CiState};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Shared = Arc<Mutex<Vec<CiJob>>>;

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    active: bool,
    run: Option<Run>,
}
#[derive(Deserialize)]
struct Run {
    #[serde(default)]
    app: String,
    #[serde(default)]
    steps: Vec<Step>,
}
#[derive(Deserialize)]
struct Step {
    key: String,
    status: String,
    progress: Option<Progress>,
    #[serde(rename = "elapsedSeconds")]
    elapsed: Option<u64>,
    #[serde(rename = "etaSeconds")]
    eta: Option<u64>,
}
#[derive(Deserialize)]
struct Progress {
    #[serde(default)]
    done: u64,
    #[serde(default)]
    total: u64,
}

#[derive(Deserialize)]
struct Scans {
    #[serde(default)]
    scans: Vec<Scan>,
}
#[derive(Deserialize)]
struct Scan {
    #[serde(default)]
    app: String,
    status: String,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    #[serde(default)]
    areas: Vec<serde_json::Value>,
}

/// 보안 점검을 단독으로 시작하면 `/api/automation` 에는 안 나오고 `/api/security/scans` 에만 나온다.
/// 진행률은 끝낸 영역 수 / 전체 7영역(서버 점검 영역 수, 완료된 점검이 항상 7개) 추정치이며 99% 상한.
pub fn parse_scans(body: &str, now_epoch: u64) -> Vec<CiJob> {
    let Ok(r) = serde_json::from_str::<Scans>(body) else { return Vec::new() };
    r.scans
        .into_iter()
        .filter(|s| s.status == "running")
        .map(|s| {
            let started = s
                .started_at
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                .map(|d| d.timestamp().max(0) as u64)
                .unwrap_or(now_epoch);
            CiJob {
                project: "TestHub".into(),
                name: format!("{} SECURITY", s.app),
                state: CiState::Running,
                percent: Some((s.areas.len() * 100 / 7).min(99) as u8),
                started_at: started,
                finished_at: None,
            }
        })
        .collect()
}

fn url_path() ->Option<std::path::PathBuf> {
    dirs_next::home_dir().map(|h| h.join(".config/ci-status/testhub.url"))
}

/// `active` 가 true 인 run 의 running 단계만 변환한다. (서버가 끝난 run 의 단계를 "running" 으로 남겨두는 경우가 있어 `active` 로 거른다.)
/// 진행률: eta 가 있으면 경과/(경과+eta), 없으면 done/total. 둘 다 모르면 None(CYD 는 정적 링). 99% 로 상한.
pub fn parse(body: &str, now_epoch: u64) -> Vec<CiJob> {
    let Ok(r) = serde_json::from_str::<Resp>(body) else { return Vec::new() };
    let Some(run) = r.run.filter(|_| r.active) else { return Vec::new() };
    run.steps
        .into_iter()
        .filter(|s| s.status == "running")
        .map(|s| {
            let el = s.elapsed.unwrap_or(0);
            let percent = match (s.eta, &s.progress) {
                (Some(eta), _) if el + eta > 0 && eta > 0 => Some((el * 100 / (el + eta)).min(99) as u8),
                (_, Some(p)) if p.total > 0 => Some((p.done * 100 / p.total).min(99) as u8),
                _ => None,
            };
            // 한글 label 은 CYD 에서 깨지므로 key 로 영문 이름을 만든다.
            let step = match s.key.as_str() {
                "api" => "API TEST".to_string(),
                "ui" => "UI TEST".to_string(),
                "scan" => "SECURITY".to_string(),
                k => k.to_uppercase(),
            };
            CiJob {
                project: "TestHub".into(),
                name: format!("{} {}", run.app, step),
                state: CiState::Running,
                percent,
                started_at: now_epoch.saturating_sub(el),
                finished_at: None,
            }
        })
        .collect()
}

/// 5초마다 폴링해 `out` 을 갱신한다. 설정 파일은 매번 다시 읽어 앱 재시작 없이 켜고 끌 수 있다.
pub fn spawn_poller(out: Shared) {
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build();
        let Ok(client) = client else { return };
        loop {
            let jobs = match url_path().and_then(|p| std::fs::read_to_string(p).ok()) {
                Some(u) => {
                    let base = u.trim().trim_end_matches('/').to_string();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    // 응답 JSON 에 제어문자가 섞이므로 파싱 전에 걷어낸다.
                    let get = |path: &str| {
                        let req = client.get(format!("{base}{path}"));
                        async move {
                            match req.send().await {
                                Ok(r) => r.text().await.map(|t| t.replace(|c: char| c.is_control(), "")).unwrap_or_default(),
                                Err(_) => String::new(),
                            }
                        }
                    };
                    let mut jobs = parse(&get("/api/automation").await, now);
                    // 자동화 안의 보안 점검 단계와 중복되지 않게, 자동화에 SECURITY 가 없을 때만 단독 점검을 더한다.
                    if !jobs.iter().any(|j| j.name.ends_with("SECURITY")) {
                        jobs.extend(parse_scans(&get("/api/security/scans").await, now));
                    }
                    jobs
                }
                None => Vec::new(),
            };
            *out.lock().unwrap() = jobs;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(active: bool) -> String {
        format!(
            r#"{{"active":{active},"run":{{"app":"Seqnex","status":"running","steps":[
            {{"key":"api","label":"API 테스트","status":"done","elapsedSeconds":null,"etaSeconds":null}},
            {{"key":"ui","label":"UI 테스트","status":"running","progress":{{"done":0,"total":1}},"elapsedSeconds":30,"etaSeconds":70}},
            {{"key":"scan","label":"보안 점검","status":"running","progress":{{"done":3,"total":7}},"elapsedSeconds":null,"etaSeconds":null}}]}}}}"#
        )
    }

    #[test]
    fn running_steps_become_jobs_with_percent() {
        let j = parse(&body(true), 1000);
        assert_eq!(j.len(), 2, "done 단계는 제외");
        assert_eq!((j[0].name.as_str(), j[0].percent, j[0].started_at), ("Seqnex UI TEST", Some(30), 970));
        assert_eq!((j[1].name.as_str(), j[1].percent), ("Seqnex SECURITY", Some(42)));
        assert!(j.iter().all(|x| x.state == CiState::Running && x.project == "TestHub"));
    }

    #[test]
    fn standalone_running_scan_becomes_job() {
        let b = r#"{"scans":[{"app":"Seqnex","status":"running","startedAt":"2026-11-11T02:37:07.365Z","areas":[{},{},{}]},
                    {"app":"Seqnex","status":"done","startedAt":"2026-11-11T00:27:17.555Z","areas":[{}]}]}"#;
        let j = parse_scans(b, 5);
        assert_eq!(j.len(), 1, "running 만");
        assert_eq!((j[0].name.as_str(), j[0].percent, j[0].started_at), ("Seqnex SECURITY", Some(42), 1794364627));
        assert!(parse_scans("garbage", 5).is_empty());
    }

    #[test]
    fn inactive_or_garbage_yields_nothing() {
        assert!(parse(&body(false), 1000).is_empty());
        assert!(parse("not json", 1000).is_empty());
        assert!(parse(r#"{"active":true,"run":null}"#, 1000).is_empty());
    }
}
