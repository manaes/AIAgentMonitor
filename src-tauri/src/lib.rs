mod aggregator;
mod clock;
mod emitter;
mod quota_proxy;
mod scheduler;
mod tray;
mod types;
mod watchers;

use aggregator::Aggregator;
use clock::SystemClock;
use emitter::EmitGate;
use scheduler::{ScheduleRule, Scheduler};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::{mpsc, Mutex};
use types::{AgentKind, TokenEvent};

fn home() -> PathBuf {
    dirs_next::home_dir().expect("home dir")
}

fn find_binary(name: &str, candidates: &[PathBuf]) -> String {
    for p in candidates {
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }
    name.to_string()
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
    let bin = find_binary("claude", &[home().join(".local/bin/claude")]);
    let r = tokio::process::Command::new(bin)
        .args(["-p", "ping"])
        .current_dir(home())
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:4319")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = r {
        tracing::warn!(%e, "quota 동기화 핑 실행 실패 (claude 미발견?)");
    }
}

// Codex quota는 rollout의 token_count.rate_limits에서만 갱신된다. 사용자가 유휴 상태면
// 새 rollout 이벤트가 없으므로, 아주 가벼운 exec를 주기적으로 실행해 서버 보고값을 새로 받는다.
fn spawn_codex_quota_ping(running: Arc<AtomicBool>) {
    if running.swap(true, Ordering::SeqCst) {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let bin = find_binary(
            "codex",
            &[
                home().join(".local/bin/codex"),
                PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
                PathBuf::from("/opt/homebrew/bin/codex"),
                PathBuf::from("/usr/local/bin/codex"),
            ],
        );
        let home_dir = home();
        let home_arg = home_dir.to_string_lossy().into_owned();

        let mut child = match tokio::process::Command::new(bin)
            .args([
                "exec",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "-C",
                home_arg.as_str(),
                "Reply exactly with the word ok and do not run commands.",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::warn!(%e, "codex quota 동기화 실행 실패 (codex 미발견?)");
                running.store(false, Ordering::SeqCst);
                return;
            }
        };

        match tokio::time::timeout(Duration::from_secs(120), child.wait()).await {
            Ok(Ok(status)) => tracing::info!(?status, "codex quota 동기화 완료"),
            Ok(Err(e)) => tracing::warn!(%e, "codex quota 동기화 대기 실패"),
            Err(_) => {
                let _ = child.kill().await;
                tracing::warn!("codex quota 동기화 timeout");
            }
        }
        running.store(false, Ordering::SeqCst);
    });
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

    // quota 상태 — Claude: anthropic-ratelimit 헤더(프록시), Codex: rollout rate_limits
    let quota_state = Arc::new(quota_proxy::QuotaState::default());
    quota_state.load_persisted();
    let codex_quota = Arc::new(watchers::codex::CodexQuota::default());
    let codex_quota_ping_running = Arc::new(AtomicBool::new(false));

    // Watchers (Claude + Codex). 둘 다 실패해도 앱은 띄움.
    let claude_root = home().join(".claude/projects");
    let _ = watchers::claude::ClaudeWatcher::spawn(claude_root, tx.clone());
    let codex_db = home().join(".codex/state_5.sqlite");
    let _ = watchers::codex::CodexWatcher::spawn(codex_db, tx.clone(), codex_quota.clone());

    // quota 프록시 spawn (포트 4319). Claude Code에 ANTHROPIC_BASE_URL 설정 시에만 트래픽이 흐른다.
    {
        let q = quota_state.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match quota_proxy::QuotaProxy::spawn(q.clone()).await {
                    Ok(port) => {
                        tracing::info!(port, "quota 프록시 준비");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(%e, "quota 프록시 바인딩 실패, 5초 뒤 재시도");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
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

    // Codex는 사용자가 아무 입력을 하지 않으면 새 rate_limits 이벤트가 기록되지 않는다.
    // 한 번이라도 Codex quota를 관측한 뒤에는 10분마다 가벼운 exec로 서버 상태를 새로 받아온다.
    {
        let quota = codex_quota.clone();
        let running = codex_quota_ping_running.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let mut ticker = tokio::time::interval(Duration::from_secs(600));
            loop {
                ticker.tick().await;
                let seen_codex_quota = quota.used_pct_5h.lock().unwrap().is_some()
                    || quota.reset_5h.lock().unwrap().is_some()
                    || quota.used_pct_weekly.lock().unwrap().is_some()
                    || quota.reset_weekly.lock().unwrap().is_some();
                if seen_codex_quota {
                    tracing::info!("codex quota 주기 동기화 핑");
                    spawn_codex_quota_ping(running.clone());
                }
            }
        });
    }
    let gate = Arc::new(Mutex::new(EmitGate::new(Duration::from_millis(500))));

    // Scheduler 초기화 — tauri async_runtime 위에서 block_on
    let scheduler = Arc::new(Mutex::new(tauri::async_runtime::block_on(async {
        Scheduler::new().await.expect("Scheduler 초기화 실패")
    })));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(scheduler)
        .invoke_handler(tauri::generate_handler![
            open_detail_window,
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
            let codex_quota = codex_quota.clone();
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
                let codex_for_tick = codex_quota.clone();
                tauri::async_runtime::spawn(async move {
                    let clock = SystemClock;
                    let mut ticker = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        ticker.tick().await;
                        let mut a = agg_for_tick.lock().await;
                        let mut snap = a.snapshot(&clock);
                        drop(a);
                        // 실제 quota 주입(있을 때만): Claude=프록시 헤더, Codex=rollout rate_limits
                        if let Some(c) =
                            snap.agents.iter_mut().find(|a| a.kind == AgentKind::Claude)
                        {
                            if let Some(u) = *quota_for_tick.used_pct.lock().unwrap() {
                                c.quota_used_pct = Some(u);
                            }
                            if let Some(r) = *quota_for_tick.reset_at.lock().unwrap() {
                                c.quota_reset_at = Some(r);
                            }
                            if let Some(u) = *quota_for_tick.used_pct_weekly.lock().unwrap() {
                                c.quota_used_pct_weekly = Some(u);
                            }
                            if let Some(r) = *quota_for_tick.reset_weekly.lock().unwrap() {
                                c.quota_reset_at_weekly = Some(r);
                            }
                        }
                        if let Some(c) = snap.agents.iter_mut().find(|a| a.kind == AgentKind::Codex)
                        {
                            if let Some(u) = *codex_for_tick.used_pct_5h.lock().unwrap() {
                                c.quota_used_pct = Some(u);
                            }
                            if let Some(r) = *codex_for_tick.reset_5h.lock().unwrap() {
                                c.quota_reset_at = Some(r);
                            }
                            if let Some(u) = *codex_for_tick.used_pct_weekly.lock().unwrap() {
                                c.quota_used_pct_weekly = Some(u);
                            }
                            if let Some(r) = *codex_for_tick.reset_weekly.lock().unwrap() {
                                c.quota_reset_at_weekly = Some(r);
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
