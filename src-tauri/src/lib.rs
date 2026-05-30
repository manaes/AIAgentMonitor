mod aggregator;
mod clock;
mod emitter;
mod otel_receiver;
mod quota_proxy;
mod scheduler;
mod tray;
mod types;
mod watchers;

use aggregator::Aggregator;
use clock::SystemClock;
use emitter::EmitGate;
use otel_receiver::{OtelReceiver, OtelState};
use scheduler::{ScheduleRule, Scheduler};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use serde::Serialize;
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex};
use types::{AgentKind, TokenEvent};

fn home() -> PathBuf {
    dirs_next::home_dir().expect("home dir")
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let log_dir = home().join("Library/Logs/AIMonitor");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("[tracing] 로그 디렉토리 생성 실패, 파일 로깅 건너뜀: {e}");
        return;
    }
    let file_appender = tracing_appender::rolling::daily(&log_dir, "app.log");
    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // init() 대신 try_init() — 이미 설정됐으면 무시 (panic 방지)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env)
        .with_writer(file_appender)
        .try_init();
    tracing::info!(?log_dir, "tracing initialized");
}

#[derive(Serialize)]
struct OtelStatusResult {
    port_bound: bool,
    data_received: bool,
}

#[tauri::command]
fn otel_status(state: tauri::State<'_, Arc<OtelState>>) -> OtelStatusResult {
    OtelStatusResult {
        port_bound: state.port_bound.load(Ordering::Relaxed),
        data_received: state.data_received.load(Ordering::Relaxed),
    }
}

#[tauri::command]
async fn open_detail_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("detail") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
    // detail을 열면 popover는 닫는다 (백엔드 hide는 capability 권한이 필요 없음)
    if let Some(p) = app.get_webview_window("popover") {
        let _ = p.hide();
    }
    Ok(())
}

#[tauri::command]
async fn list_trigger_rules(
    state: tauri::State<'_, Arc<Mutex<Scheduler>>>,
) -> Result<Vec<ScheduleRule>, String> {
    let s = state.lock().await;
    Ok(s.list_rules())
}

// HH(0-23), MM(0-59)을 받아 6필드 cron "0 MM HH * * *"으로 변환 후 룰 추가
#[tauri::command]
async fn add_trigger_rule(
    state: tauri::State<'_, Arc<Mutex<Scheduler>>>,
    agent: String,
    hour: u8,
    minute: u8,
    working_dir: String,
    prompt: String,
) -> Result<ScheduleRule, String> {
    let cron = format!("0 {minute} {hour} * * *");
    let mut s = state.lock().await;
    s.add_rule(agent, cron, working_dir, prompt)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_trigger_rule(
    state: tauri::State<'_, Arc<Mutex<Scheduler>>>,
    id: String,
) -> Result<(), String> {
    let mut s = state.lock().await;
    s.remove_rule(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn toggle_trigger_rule(
    state: tauri::State<'_, Arc<Mutex<Scheduler>>>,
    id: String,
) -> Result<ScheduleRule, String> {
    let mut s = state.lock().await;
    s.toggle_rule(&id).await.map_err(|e| e.to_string())
}

// 즉시 실행: id에 해당하는 룰을 지금 바로 spawn
#[tauri::command]
async fn fire_trigger_now(
    state: tauri::State<'_, Arc<Mutex<Scheduler>>>,
    id: String,
) -> Result<(), String> {
    let s = state.lock().await;
    let rule = s
        .rules
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("id를 찾을 수 없습니다: {id}"))?
        .clone();
    drop(s);
    scheduler::runner::run_trigger(&rule.agent, &rule.prompt, &rule.working_dir).await;
    Ok(())
}

// claude를 프록시(:4319) 경유로 1회 핑 → 응답 ratelimit 헤더에서 5h 사용률/리셋을 캡처.
// ANTHROPIC_BASE_URL은 이 자식 프로세스에만 설정되므로 사용자의 일반 Claude Code 세션은
// 프록시를 거치지 않는다(상시 경유 footgun 회피). GUI 앱 PATH 대비 절대경로 우선.
fn spawn_quota_ping() {
    let bin = {
        let p = home().join(".local/bin/claude");
        if p.exists() {
            p.into_os_string().into_string().unwrap_or_else(|_| "claude".to_string())
        } else {
            "claude".to_string()
        }
    };
    let r = tokio::process::Command::new(bin)
        .args(["-p", "ping"])
        .current_dir(home())
        .env("ANTHROPIC_BASE_URL", "http://localhost:4319")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = r {
        tracing::warn!(%e, "quota 동기화 핑 실행 실패 (claude 미발견?)");
    }
}

// 수동 동기화 (UI 버튼)
#[tauri::command]
async fn sync_quota() -> Result<(), String> {
    spawn_quota_ping();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let (tx, mut rx) = mpsc::unbounded_channel::<TokenEvent>();

    // OTEL 상태 먼저 생성 (Watcher에 전달 필요)
    let otel_state = Arc::new(OtelState {
        port_bound:    AtomicBool::new(false),
        data_received: AtomicBool::new(false),
    });

    // quota 프록시 상태 (실제 ratelimit 헤더에서 캡처한 5h 사용률/리셋)
    let quota_state = Arc::new(quota_proxy::QuotaState::default());

    // Watchers (Claude + Codex). 둘 다 실패해도 앱은 띄움.
    let claude_root = home().join(".claude/projects");
    let _ = watchers::claude::ClaudeWatcher::spawn(claude_root, tx.clone(), otel_state.clone());
    let codex_db = home().join(".codex/state_5.sqlite");
    let _ = watchers::codex::CodexWatcher::spawn(codex_db, tx.clone());

    // OTEL 리시버 spawn (포트 4318)
    {
        let otel_tx  = tx.clone();
        let otel_ref = otel_state.clone();
        tauri::async_runtime::spawn(async move {
            match OtelReceiver::spawn(otel_tx, otel_ref).await {
                Ok(port) => tracing::info!(port, "OTEL 리시버 준비"),
                Err(e)   => tracing::warn!(%e, "OTEL 리시버 비활성 (포트 4318 사용 불가)"),
            }
        });
    }

    // quota 프록시 spawn (포트 4319). Claude Code에 ANTHROPIC_BASE_URL 설정 시에만 트래픽이 흐른다.
    {
        let q = quota_state.clone();
        tauri::async_runtime::spawn(async move {
            match quota_proxy::QuotaProxy::spawn(q).await {
                Ok(port) => tracing::info!(port, "quota 프록시 준비"),
                Err(e)   => tracing::warn!(%e, "quota 프록시 비활성 (포트 4319 사용 불가)"),
            }
        });
    }

    drop(tx); // 모든 producer 등록 완료 후 원본 drop

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));

    // 주기적 자동 동기화: 최근(15분 내) 활동이 있으면 10분마다 프록시 핑으로 사용량을 보정.
    // 핑만 프록시를 거치므로 일반 세션엔 영향 없고, 유휴 시엔 quota 낭비를 막기 위해 생략한다.
    {
        let agg = aggregator.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await; // 시작 시 startup replay 완료 대기
            let mut ticker = tokio::time::interval(Duration::from_secs(600)); // 10분
            loop {
                ticker.tick().await;
                let recent = { agg.lock().await.last_event_at() };
                let active = recent
                    .and_then(|t| t.elapsed().ok())
                    .map(|e| e < Duration::from_secs(900))
                    .unwrap_or(false);
                if active {
                    tracing::info!("주기 자동 동기화 핑");
                    spawn_quota_ping();
                }
            }
        });
    }
    let gate = Arc::new(Mutex::new(EmitGate::new(Duration::from_millis(500))));

    // OTEL 활성화 시 별도 aggregator 초기화 불필요.
    // startup replay로 복원한 5h 과거 데이터를 유지하고,
    // 이후 OTEL delta를 쌓아가면 과거+현재가 모두 반영됨.
    // live jsonl 이벤트만 중단(ClaudeWatcher 내부)해서 이중계산 방지.

    // Scheduler 초기화 — tauri async_runtime 위에서 block_on
    let scheduler = Arc::new(Mutex::new(
        tauri::async_runtime::block_on(async {
            Scheduler::new().await.expect("Scheduler 초기화 실패")
        })
    ));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(scheduler)
        .manage(otel_state)
        .invoke_handler(tauri::generate_handler![
            open_detail_window,
            otel_status,
            list_trigger_rules,
            add_trigger_rule,
            remove_trigger_rule,
            toggle_trigger_rule,
            fire_trigger_now,
            sync_quota,
        ])
        .setup({
            let aggregator = aggregator.clone();
            let gate = gate.clone();
            let quota_state = quota_state.clone();
            move |app| {
                // Dock 아이콘 숨김 — setup 초반에 호출
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);

                tray::install(app.handle())?;

                let app_handle = app.handle().clone();
                let agg_for_ingest = aggregator.clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(ev) = rx.recv().await {
                        let mut g = agg_for_ingest.lock().await;
                        g.push(ev);
                    }
                });

                let agg_for_tick = aggregator.clone();
                let gate_for_tick = gate.clone();
                let quota_for_tick = quota_state.clone();
                tauri::async_runtime::spawn(async move {
                    let clock = SystemClock;
                    let mut ticker = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        ticker.tick().await;
                        let mut a = agg_for_tick.lock().await;
                        let mut snap = a.snapshot(&clock);
                        drop(a);
                        // 프록시가 캡처한 실제 quota(%/리셋)를 Claude 카드에 주입 (있을 때만)
                        let used = *quota_for_tick.used_pct.lock().unwrap();
                        let reset = *quota_for_tick.reset_at.lock().unwrap();
                        if used.is_some() || reset.is_some() {
                            if let Some(c) = snap.agents.iter_mut().find(|a| a.kind == AgentKind::Claude) {
                                if used.is_some() { c.quota_used_pct = used; }
                                if let Some(r) = reset { c.quota_reset_at = Some(r); }
                            }
                        }
                        let mut g = gate_for_tick.lock().await;
                        if g.should_emit(&snap, std::time::SystemTime::now()) {
                            let _ = app_handle.emit("snapshot", &snap);
                        }
                    }
                });

                Ok(())
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
