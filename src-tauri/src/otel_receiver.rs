/// OpenTelemetry HTTP 리시버 (OTLP/HTTP JSON, 포트 4318)
///
/// Claude Code 환경변수:
///   CLAUDE_CODE_ENABLE_TELEMETRY=1
///   OTEL_METRICS_EXPORTER=otlp
///   OTEL_LOGS_EXPORTER=otlp
///   OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use crate::types::{AgentKind, TokenCounts, TokenEvent};

/// OTEL 상태 (Tauri State로 공유)
pub struct OtelState {
    pub port_bound: AtomicBool,
    pub data_received: AtomicBool,
}

// ── OTLP JSON 구조 (protobuf → JSON 변환 스펙 기반) ──────────────────

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct OtlpMetricsBody {
    resource_metrics: Vec<ResourceMetrics>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct ResourceMetrics {
    resource: Option<Resource>,
    scope_metrics: Vec<ScopeMetrics>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Resource {
    attributes: Vec<Attribute>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct ScopeMetrics {
    metrics: Vec<Metric>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Metric {
    name: String,
    sum: Option<DataPoints>,
    gauge: Option<DataPoints>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct DataPoints {
    data_points: Vec<DataPoint>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct DataPoint {
    attributes: Vec<Attribute>,
    time_unix_nano: Option<serde_json::Value>, // string or number
    as_int: Option<serde_json::Value>,
    as_double: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct Attribute {
    key: String,
    value: AttrVal,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct AttrVal {
    string_value: Option<String>,
    int_value: Option<serde_json::Value>,
    double_value: Option<f64>,
    bool_value: Option<bool>,
}

impl AttrVal {
    fn as_str(&self) -> Option<&str> { self.string_value.as_deref() }
}

fn attr_map(attrs: &[Attribute]) -> HashMap<&str, &AttrVal> {
    attrs.iter().map(|a| (a.key.as_str(), &a.value)).collect()
}

fn parse_nanos(v: &serde_json::Value) -> u64 {
    v.as_u64().or_else(|| v.as_str()?.parse().ok()).unwrap_or(0)
}

fn nano_to_systime(nanos: u64) -> SystemTime {
    if nanos == 0 { return SystemTime::now(); }
    UNIX_EPOCH + Duration::from_nanos(nanos)
}

// ── 메트릭 → TokenEvent 변환 ─────────────────────────────────────────

fn process_metrics(body: &OtlpMetricsBody, tx: &mpsc::UnboundedSender<TokenEvent>) {
    for rm in &body.resource_metrics {
        let res_attrs = rm.resource.as_ref()
            .map(|r| attr_map(&r.attributes))
            .unwrap_or_default();

        let session_id = res_attrs.get("session.id")
            .and_then(|v| v.as_str())
            .unwrap_or("otel-unknown")
            .to_string();

        // Claude Code는 working_directory 혹은 cwd로 프로젝트 경로를 보낼 수 있음
        let working_dir = ["working_directory", "working.directory", "cwd", "project.path"]
            .iter()
            .find_map(|k| res_attrs.get(*k)?.as_str())
            .unwrap_or("unknown");

        for sm in &rm.scope_metrics {
            for metric in &sm.metrics {
                let data_points: &[DataPoint] = metric.sum.as_ref()
                    .map(|s| s.data_points.as_slice())
                    .or_else(|| metric.gauge.as_ref().map(|g| g.data_points.as_slice()))
                    .unwrap_or(&[]);

                // 비용 메트릭 로깅 (나중에 UI에 추가 예정)
                if metric.name == "claude_code.cost.usage" {
                    for dp in data_points {
                        if let Some(cost) = dp.as_double {
                            let attrs = attr_map(&dp.attributes);
                            let model = attrs.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                            tracing::info!(cost, model, session_id = %session_id, "OTEL 비용");
                        }
                    }
                    continue;
                }

                // Claude Code 실제 메트릭명: claude_code.token.usage
                let is_token_metric = metric.name == "claude_code.token.usage"
                    || metric.name.contains("token");
                if !is_token_metric { continue; }

                for dp in data_points {
                    let attrs = attr_map(&dp.attributes);

                    let token_type = attrs.get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("input");

                    let model = attrs.get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("claude-unknown")
                        // Claude Code 모델명에 붙는 [1m] 같은 suffix 제거
                        .split('[').next().unwrap_or("claude-unknown")
                        .to_string();

                    // Claude Code는 asDouble로 전송 (정수여도)
                    let count = dp.as_double
                        .map(|v| v as u32)
                        .or_else(|| dp.as_int.as_ref()
                            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                            .map(|v| v as u32))
                        .unwrap_or(0);

                    if count == 0 { continue; }

                    let ts = dp.time_unix_nano.as_ref()
                        .map(|v| nano_to_systime(parse_nanos(v)))
                        .unwrap_or_else(SystemTime::now);

                    // Claude Code 실제 type 값: input, output, cacheRead, cacheCreation
                    let counts = match token_type {
                        "input"                    => TokenCounts { tokens_in: count, ..Default::default() },
                        "output"                   => TokenCounts { tokens_out: count, ..Default::default() },
                        "cacheRead"   | "cache_read"   | "cache_read_input"   => TokenCounts { tokens_cache_read: count, ..Default::default() },
                        "cacheCreation" | "cache_creation" | "cache_creation_input" => TokenCounts { tokens_cache_create: count, ..Default::default() },
                        other => {
                            // 미상 token type은 input으로 흡수하지 않고 건너뛴다 (input 과대계상 방지)
                            tracing::warn!(metric_name = %metric.name, token_type = other, count, "OTEL 알 수 없는 token type — 건너뜀");
                            continue;
                        }
                    };

                    let ev = TokenEvent {
                        agent: AgentKind::Claude,
                        project_path: PathBuf::from(working_dir),
                        session_id: session_id.clone(),
                        model,
                        ts,
                        counts,
                    };
                    tracing::info!(token_type, count, "OTEL TokenEvent → aggregator");
                    let _ = tx.send(ev);
                }
            }
        }
    }
}

// ── 공유 상태 ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    tx: Arc<mpsc::UnboundedSender<TokenEvent>>,
    otel: Arc<OtelState>,
}

// ── 핸들러 ────────────────────────────────────────────────────────────

async fn handle_metrics(
    State(st): State<AppState>,
    body: Json<serde_json::Value>,
) -> impl IntoResponse {
    st.otel.data_received.store(true, Ordering::Relaxed);
    tracing::info!("OTEL /v1/metrics 수신");
    tracing::info!(payload = %body.0, "OTEL raw payload");

    match serde_json::from_value::<OtlpMetricsBody>(body.0) {
        Ok(parsed) => process_metrics(&parsed, &st.tx),
        Err(e) => tracing::warn!(%e, "OTEL metrics 파싱 실패"),
    }
    StatusCode::OK
}

async fn handle_logs(body: Json<serde_json::Value>) -> impl IntoResponse {
    tracing::debug!("OTEL /v1/logs 수신 (현재 파싱 안 함)");
    tracing::trace!(payload = %body.0, "OTEL raw logs");
    StatusCode::OK
}

async fn handle_traces(_body: Json<serde_json::Value>) -> impl IntoResponse {
    StatusCode::OK
}

// ── 공개 인터페이스 ──────────────────────────────────────────────────

pub struct OtelReceiver;

impl OtelReceiver {
    /// OTLP HTTP 리시버를 백그라운드에 spawn.
    /// 성공 시 바인딩된 포트 반환.
    /// otel_active: 첫 데이터 수신 시 true로 설정됨 (UI 상태 표시용)
    pub async fn spawn(
        tx: mpsc::UnboundedSender<TokenEvent>,
        otel: Arc<OtelState>,
    ) -> Result<u16, String> {
        let state = AppState { tx: Arc::new(tx), otel: otel.clone() };

        let app = Router::new()
            .route("/v1/metrics", post(handle_metrics))
            .route("/v1/logs",    post(handle_logs))
            .route("/v1/traces",  post(handle_traces))
            .with_state(state);

        let addr = SocketAddr::from(([127, 0, 0, 1], 4318));

        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(4318);
                otel.port_bound.store(true, Ordering::Relaxed);
                tracing::info!(port, "OTEL 리시버 시작");
                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, app).await {
                        tracing::error!(%e, "OTEL 리시버 종료");
                    }
                });
                Ok(port)
            }
            Err(e) => {
                tracing::warn!(%e, "포트 4318 바인딩 실패 — OTEL 비활성");
                Err(format!("{e}"))
            }
        }
    }
}
