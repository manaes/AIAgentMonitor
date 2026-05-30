//! 로컬 API 프록시 (포트 4319).
//!
//! Claude Code의 ANTHROPIC_BASE_URL=http://localhost:4319 로 지정하면, 이 프록시가
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
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const UPSTREAM: &str = "https://api.anthropic.com";

/// 프록시가 캡처한 실제 quota 상태. lib.rs 틱 루프가 읽어 스냅샷에 주입한다.
#[derive(Default)]
pub struct QuotaState {
    pub used_pct: Mutex<Option<f32>>, // 0..100
    pub reset_at: Mutex<Option<SystemTime>>,
    pub active: Mutex<bool>, // 프록시를 통한 트래픽을 본 적 있는지
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

    // utilization: 0..1 또는 0..100 — 후보 헤더명 순차 시도
    let util = parse_f64(headers, "anthropic-ratelimit-unified-5h-utilization")
        .or_else(|| parse_f64(headers, "anthropic-ratelimit-unified-utilization"))
        .or_else(|| parse_f64(headers, "anthropic-ratelimit-unified-5h-used"));
    if let Some(u) = util {
        let pct = if u <= 1.0 { (u * 100.0) as f32 } else { u as f32 };
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

    if saw_any {
        *quota.active.lock().unwrap() = true;
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
    let body_bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

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
                    "quota 프록시 시작 (Claude Code에 ANTHROPIC_BASE_URL=http://localhost:4319 설정 시 사용)"
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
