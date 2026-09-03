//! 로컬 API 프록시 (포트 4319).
//!
//! Claude Code의 ANTHROPIC_BASE_URL=http://127.0.0.1:4319 로 지정하면, 이 프록시가
//! 모든 요청을 https://api.anthropic.com 으로 **변형 없이** 포워딩하면서 응답의
//! `anthropic-ratelimit-unified-*` 헤더를 읽어 실제 5h 사용률/리셋을 캡처한다.
//! - TLS는 프록시가 업스트림의 클라이언트로서 처리 → Claude Code↔localhost는 평문,
//!   MITM/사설 CA 불필요.
//! - 응답 바디는 스트리밍 그대로 전달(SSE 유지).
//! - 헤더명이 확정 전이라, 본 모듈은 anthropic-ratelimit-* 헤더를 전부 INFO 로깅한다.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const UPSTREAM: &str = "https://api.anthropic.com";

/// 요청 바디 버퍼링 상한. localhost 전용이지만 무제한 버퍼링으로 인한
/// 메모리 고갈을 막는다 (메시지 요청 JSON은 이보다 훨씬 작다).
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// 프록시가 캡처한 실제 quota 상태. lib.rs 틱 루프가 읽어 스냅샷에 주입한다.
#[derive(Default)]
pub struct QuotaState {
    pub used_pct: Mutex<Option<f32>>, // 0..100 (5h)
    pub reset_at: Mutex<Option<SystemTime>>,
    pub used_pct_weekly: Mutex<Option<f32>>, // 0..100 (7d)
    pub reset_weekly: Mutex<Option<SystemTime>>,
    pub active: Mutex<bool>, // 프록시를 통한 트래픽을 본 적 있는지
    /// 5h/주간 %가 (프록시 헤더든 `/usage` 파싱이든) 마지막으로 갱신된 시각.
    /// lib.rs의 주기 자동 동기화가 "안전망으로 /usage 를 부를지"를
    /// 판단하는 기준이다 — `is_stale` 문서 참고.
    last_updated: Mutex<Option<SystemTime>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedQuota {
    used_pct: Option<f32>,
    reset_at: Option<u64>,
    used_pct_weekly: Option<f32>,
    reset_weekly: Option<u64>,
}

fn persist_path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ai-agent-monitor/claude-quota.json")
}

fn system_time_to_epoch(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn epoch_to_system_time(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

impl QuotaState {
    pub fn load_persisted(&self) {
        let path = persist_path();
        let Ok(json) = std::fs::read_to_string(&path) else { return; };
        let Ok(p) = serde_json::from_str::<PersistedQuota>(&json) else {
            tracing::warn!(?path, "Claude quota 캐시 파싱 실패");
            return;
        };

        *self.used_pct.lock().unwrap() = p.used_pct;
        *self.reset_at.lock().unwrap() = p.reset_at.map(epoch_to_system_time);
        *self.used_pct_weekly.lock().unwrap() = p.used_pct_weekly;
        *self.reset_weekly.lock().unwrap() = p.reset_weekly.map(epoch_to_system_time);
        *self.active.lock().unwrap() = p.used_pct.is_some() || p.used_pct_weekly.is_some();
        tracing::info!(?path, "Claude quota 캐시 로드");
    }

    /// `claude -p "/usage"` 텍스트 파싱 결과를 반영한다. 실 트래픽 헤더(`observe`)와
    /// 달리 reset 시각은 여기서 다루지 않는다(파일 하단 `parse_usage_pct` 문서 참고) —
    /// 값이 있는 쪽만 갱신하고, 기존 reset_at 은 그대로 둔다.
    pub fn apply_usage_pct(&self, session_pct: Option<f32>, week_pct: Option<f32>) {
        if let Some(p) = session_pct {
            *self.used_pct.lock().unwrap() = Some(p.clamp(0.0, 100.0));
        }
        if let Some(p) = week_pct {
            *self.used_pct_weekly.lock().unwrap() = Some(p.clamp(0.0, 100.0));
        }
        if session_pct.is_some() || week_pct.is_some() {
            *self.active.lock().unwrap() = true;
            self.mark_updated();
            self.save_persisted();
        }
    }

    fn mark_updated(&self) {
        *self.last_updated.lock().unwrap() = Some(SystemTime::now());
    }

    /// 마지막 갱신(프록시 헤더든 `/usage` 파싱이든) 이후 `after` 이상 지났으면
    /// true. 한 번도 갱신된 적 없으면(앱을 막 띄웠는데 persisted 캐시도 없는
    /// 경우) 무조건 true — 최대한 빨리 첫 값을 받아야 하니 안전망 쪽으로
    /// 기운다. lib.rs의 주기 자동 동기화가 이걸로 "/usage 를 부를지"
    /// 판단한다: 실사용 중이라 프록시가 이미 자주 갱신해주면 매번 새로
    /// 프로세스를 띄울 필요가 없다.
    pub fn is_stale(&self, now: SystemTime, after: Duration) -> bool {
        match *self.last_updated.lock().unwrap() {
            Some(t) => now.duration_since(t).unwrap_or_default() >= after,
            None => true,
        }
    }

    fn save_persisted(&self) {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let p = PersistedQuota {
            used_pct: *self.used_pct.lock().unwrap(),
            reset_at: self.reset_at.lock().unwrap().and_then(system_time_to_epoch),
            used_pct_weekly: *self.used_pct_weekly.lock().unwrap(),
            reset_weekly: self.reset_weekly.lock().unwrap().and_then(system_time_to_epoch),
        };

        match serde_json::to_string_pretty(&p) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!(%e, ?path, "Claude quota 캐시 저장 실패");
                }
            }
            Err(e) => tracing::warn!(%e, "Claude quota 캐시 직렬화 실패"),
        }
    }
}

#[derive(Clone)]
struct ProxyState {
    client: reqwest::Client,
    quota: Arc<QuotaState>,
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim())
}

fn parse_f64(h: &HeaderMap, name: &str) -> Option<f64> {
    header_str(h, name)?.parse().ok()
}

fn ratio_to_pct(u: f64) -> f32 {
    (u * 100.0) as f32
}

/// `claude -p "/usage"` 표준출력에서 세션(5h)/전체 모델 주간 사용률만 뽑는다.
/// `(Fable)` 같은 모델별 하위 항목은 이 앱이 표시하는 값과 무관하므로 무시한다.
///
/// reset 시각은 일부러 파싱하지 않는다 — 여기 찍히는 시각은
/// "Sep 3 at 6:59pm (Asia/Seoul)" 처럼 로캘 지역명이 붙은 텍스트라, 타임존
/// 데이터베이스 없이는 정확한 UTC로 되돌릴 수 없다(DST 있는 지역이면 특히).
/// reset_at 은 그대로 `observe()`(실제 응답 헤더)에 맡긴다 — 실제로 리셋되기
/// 전까진 값이 안 바뀌므로, 이 폴링에서 못 얻어도 정확도에 문제가 없다.
///
/// 2026-09-03 확인: 이 커맨드는 실제 채팅 완성이 아니라 계정 한도 조회
/// 전용이라 연속으로 여러 번 불러도 %가 안 움직였다(연결 request 카운터만
/// 오름) — 활동 여부와 무관하게 상시 폴링해도 quota 를 갉아먹지 않는다.
pub fn parse_usage_pct(output: &str) -> (Option<f32>, Option<f32>) {
    let mut session_pct = None;
    let mut week_pct = None;
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Current session:") {
            session_pct = session_pct.or_else(|| extract_leading_pct(rest));
        } else if let Some(rest) = line.strip_prefix("Current week (all models):") {
            week_pct = week_pct.or_else(|| extract_leading_pct(rest));
        }
    }
    (session_pct, week_pct)
}

fn extract_leading_pct(s: &str) -> Option<f32> {
    s.trim().split('%').next()?.trim().parse::<f32>().ok()
}

/// 응답 헤더에서 5h 사용률/리셋을 추출. 헤더명 후보를 방어적으로 시도하고 전체를 로깅한다.
fn observe(headers: &HeaderMap, quota: &Arc<QuotaState>) {
    let mut saw_any = false;
    for (name, value) in headers.iter() {
        let n = name.as_str();
        if n.starts_with("anthropic-ratelimit") {
            saw_any = true;
            tracing::info!(header = n, value = value.to_str().unwrap_or("?"), "quota_proxy ratelimit 헤더");
        }
    }

    // utilization은 비율(예: 0.98=98%, 1.01=101%)로 온다. 1.0을 넘는 순간도
    // 100% 초과 사용량이므로 반드시 100을 곱해서 퍼센트로 변환한다.
    let util = parse_f64(headers, "anthropic-ratelimit-unified-5h-utilization")
        .or_else(|| parse_f64(headers, "anthropic-ratelimit-unified-utilization"));
    if let Some(u) = util {
        let pct = ratio_to_pct(u);
        *quota.used_pct.lock().unwrap() = Some(pct.clamp(0.0, 100.0));
        tracing::info!(pct, "quota_proxy 사용률 캡처");
    }

    // reset: epoch seconds 추정 (큰 숫자), 또는 RFC3339는 후속 보정
    let reset = parse_f64(headers, "anthropic-ratelimit-unified-5h-reset")
        .or_else(|| parse_f64(headers, "anthropic-ratelimit-unified-reset"));
    if let Some(r) = reset {
        if r > 1_000_000_000.0 {
            *quota.reset_at.lock().unwrap() = Some(UNIX_EPOCH + Duration::from_secs(r as u64));
        }
    }

    // 주간(7d) 창
    if let Some(u) = parse_f64(headers, "anthropic-ratelimit-unified-7d-utilization") {
        let pct = ratio_to_pct(u);
        *quota.used_pct_weekly.lock().unwrap() = Some(pct.clamp(0.0, 100.0));
    }
    if let Some(r) = parse_f64(headers, "anthropic-ratelimit-unified-7d-reset") {
        if r > 1_000_000_000.0 {
            *quota.reset_weekly.lock().unwrap() = Some(UNIX_EPOCH + Duration::from_secs(r as u64));
        }
    }

    if saw_any {
        *quota.active.lock().unwrap() = true;
        quota.mark_updated();
        quota.save_persisted();
    }
}

/// 모든 요청을 업스트림으로 포워딩하는 fallback 핸들러.
async fn proxy(State(st): State<ProxyState>, req: Request) -> Result<Response, StatusCode> {
    let (parts, body) = req.into_parts();
    let pq = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let url = format!("{UPSTREAM}{pq}");

    // 요청 바디 버퍼링 (메시지 요청은 단일 JSON이라 스트리밍 불필요)
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;

    let mut rb = st.client.request(parts.method.clone(), &url).body(body_bytes.to_vec());
    for (name, value) in parts.headers.iter() {
        // Host는 reqwest가 업스트림 기준으로 다시 설정
        if name.as_str().eq_ignore_ascii_case("host") {
            continue;
        }
        rb = rb.header(name.clone(), value.clone());
    }

    let upstream = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "quota_proxy 업스트림 전송 실패");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    observe(upstream.headers(), &st.quota);

    let status = upstream.status();
    let mut out_headers = HeaderMap::new();
    for (name, value) in upstream.headers().iter() {
        // 프레이밍/홉바이홉 헤더는 hyper가 다시 설정하므로 제외
        let n = name.as_str();
        if n.eq_ignore_ascii_case("transfer-encoding")
            || n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        out_headers.insert(name.clone(), value.clone());
    }

    let mut resp = Response::new(Body::from_stream(upstream.bytes_stream()));
    *resp.status_mut() = status;
    *resp.headers_mut() = out_headers;
    Ok(resp)
}

pub struct QuotaProxy;

impl QuotaProxy {
    /// 프록시를 127.0.0.1:4319에 spawn. 성공 시 포트 반환.
    pub async fn spawn(quota: Arc<QuotaState>) -> Result<u16, String> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| e.to_string())?;
        let state = ProxyState { client, quota };
        let app = Router::new().fallback(proxy).with_state(state);

        let addr = SocketAddr::from(([127, 0, 0, 1], 4319));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(4319);
                tracing::info!(
                    port,
                    "quota 프록시 시작 (Claude Code에 ANTHROPIC_BASE_URL=http://127.0.0.1:4319 설정 시 사용)"
                );
                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, app).await {
                        tracing::error!(%e, "quota 프록시 종료");
                    }
                });
                Ok(port)
            }
            Err(e) => {
                tracing::warn!(%e, "포트 4319 바인딩 실패 — quota 프록시 비활성");
                Err(format!("{e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn never_updated_state_is_always_stale() {
        let quota = QuotaState::default();
        assert!(quota.is_stale(SystemTime::now(), Duration::from_secs(600)));
    }

    #[test]
    fn freshly_updated_state_is_not_stale() {
        let quota = QuotaState::default();
        quota.apply_usage_pct(Some(10.0), None);
        assert!(!quota.is_stale(SystemTime::now(), Duration::from_secs(600)));
    }

    #[test]
    fn state_becomes_stale_after_the_threshold() {
        let quota = QuotaState::default();
        quota.apply_usage_pct(Some(10.0), None);
        let later = SystemTime::now() + Duration::from_secs(601);
        assert!(quota.is_stale(later, Duration::from_secs(600)));
    }

    #[test]
    fn observing_real_headers_also_marks_it_fresh() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            HeaderValue::from_static("0.5"),
        );
        let quota = Arc::new(QuotaState::default());
        observe(&headers, &quota);
        assert!(!quota.is_stale(SystemTime::now(), Duration::from_secs(600)));
    }

    #[test]
    fn utilization_over_one_is_treated_as_ratio() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            HeaderValue::from_static("1.01"),
        );

        let quota = Arc::new(QuotaState::default());
        observe(&headers, &quota);

        assert_eq!(*quota.used_pct.lock().unwrap(), Some(100.0));
    }

    #[test]
    fn parses_session_and_weekly_pct_from_usage_output() {
        // 실제 `claude -p "/usage"` 출력(2026-09-03 사용자 제공).
        let output = "\
You are currently using your subscription to power your Claude Code usage

Current session: 21% used · resets Sep 3 at 6:59pm (Asia/Seoul)
Current week (all models): 64% used · resets Sep 6 at 6:59pm (Asia/Seoul)
Current week (Fable): 98% used · resets Sep 6 at 6:59pm (Asia/Seoul)

What's contributing to your limits usage?
";
        let (session, week) = parse_usage_pct(output);
        assert_eq!(session, Some(21.0), "Current session 값을 읽어야 한다");
        assert_eq!(week, Some(64.0), "(all models) 주간 값을 읽어야 한다 — (Fable) 이 아니라");
    }

    #[test]
    fn missing_lines_yield_none_without_panicking() {
        let (session, week) = parse_usage_pct("무관한 출력\n");
        assert_eq!(session, None);
        assert_eq!(week, None);
    }

    #[test]
    fn apply_usage_pct_only_touches_fields_with_a_value() {
        let quota = QuotaState::default();
        *quota.used_pct.lock().unwrap() = Some(10.0);
        *quota.used_pct_weekly.lock().unwrap() = Some(20.0);

        quota.apply_usage_pct(Some(30.0), None);

        assert_eq!(*quota.used_pct.lock().unwrap(), Some(30.0), "값이 있으면 갱신");
        assert_eq!(*quota.used_pct_weekly.lock().unwrap(), Some(20.0), "None 이면 기존 값 유지");
    }

    #[test]
    fn utilization_under_one_is_converted_to_percent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            HeaderValue::from_static("0.98"),
        );

        let quota = Arc::new(QuotaState::default());
        observe(&headers, &quota);

        assert_eq!(*quota.used_pct.lock().unwrap(), Some(98.0));
    }
}
