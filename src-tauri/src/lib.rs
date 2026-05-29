mod aggregator;
mod clock;
mod emitter;
mod otel_receiver;
mod scheduler;
mod tray;
mod types;
mod watchers;

use aggregator::Aggregator;
use clock::SystemClock;
use emitter::EmitGate;
use otel_receiver::OtelReceiver;
use scheduler::{ScheduleRule, Scheduler};
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex};
use types::TokenEvent;

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

#[tauri::command]
fn otel_status(state: tauri::State<'_, Arc<AtomicBool>>) -> bool {
    state.load(Ordering::Relaxed)
}

#[tauri::command]
async fn open_detail_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("detail") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let (tx, mut rx) = mpsc::unbounded_channel::<TokenEvent>();

    // Watchers (Claude + Codex). 둘 다 실패해도 앱은 띄움.
    let claude_root = home().join(".claude/projects");
    let _ = watchers::claude::ClaudeWatcher::spawn(claude_root, tx.clone());
    let codex_db = home().join(".codex/state_5.sqlite");
    let _ = watchers::codex::CodexWatcher::spawn(codex_db, tx.clone());

    // OTEL 리시버 spawn (포트 4318) — drop(tx) 이전에 clone
    let otel_active = Arc::new(AtomicBool::new(false));
    {
        let otel_tx = tx.clone();
        let flag = otel_active.clone();
        tauri::async_runtime::spawn(async move {
            match OtelReceiver::spawn(otel_tx, flag).await {
                Ok(port) => tracing::info!(port, "OTEL 리시버 준비"),
                Err(e)   => tracing::warn!(%e, "OTEL 리시버 비활성 (포트 4318 사용 불가)"),
            }
        });
    }

    drop(tx); // 모든 producer 등록 완료 후 원본 drop

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let gate = Arc::new(Mutex::new(EmitGate::new(Duration::from_millis(500))));

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
        .manage(otel_active)
        .invoke_handler(tauri::generate_handler![
            open_detail_window,
            otel_status,
            list_trigger_rules,
            add_trigger_rule,
            remove_trigger_rule,
            toggle_trigger_rule,
            fire_trigger_now,
        ])
        .setup({
            let aggregator = aggregator.clone();
            let gate = gate.clone();
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
                tauri::async_runtime::spawn(async move {
                    let clock = SystemClock;
                    let mut ticker = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        ticker.tick().await;
                        let mut a = agg_for_tick.lock().await;
                        let snap = a.snapshot(&clock);
                        drop(a);
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
