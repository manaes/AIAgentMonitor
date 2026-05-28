mod aggregator;
mod clock;
mod emitter;
mod tray;
mod types;
mod watchers;

use aggregator::Aggregator;
use clock::SystemClock;
use emitter::EmitGate;
use std::path::PathBuf;
use std::sync::Arc;
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
async fn open_detail_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("detail") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
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
    drop(tx);

    let aggregator = Arc::new(Mutex::new(Aggregator::new()));
    let gate = Arc::new(Mutex::new(EmitGate::new(Duration::from_millis(500))));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![open_detail_window])
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
