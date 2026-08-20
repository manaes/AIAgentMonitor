mod aggregator;
mod ble;
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
use tauri_plugin_updater::UpdaterExt;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use types::{AgentKind, TokenEvent};

#[cfg(target_os = "macos")]
use ble::peripheral::BlePeripheral;
use ble::BleBridge;

#[derive(Clone, serde::Serialize)]
pub struct BlePeer {
    pub id: String,
    pub mtu: usize,
}

/// 이 빌드에 실제 BLE 주변장치 구현이 있는지. macOS 외 플랫폼은 FakePeripheral 이라
/// 토글이 성공을 보고해도 아무 일도 일어나지 않으므로, 프론트가 UI 자체를 숨길 수 있게
/// 백엔드가 사실을 알려준다(프론트에서 OS 를 추측하지 않는다).
#[cfg(target_os = "macos")]
const BLE_SUPPORTED: bool = true;
#[cfg(not(target_os = "macos"))]
const BLE_SUPPORTED: bool = false;

#[derive(Clone, serde::Serialize)]
pub struct BleStatus {
    pub supported: bool,
    pub enabled: bool,
    pub advertising: bool,
    pub peers: Vec<BlePeer>,
    pub last_error: Option<String>,
    /// 페어링 창 상태. UI 가 만료와 시도 소진을 구분해 보여줘야 한다 — 소진이
    /// 보인다는 것이 창에 소유자를 두지 않기로 한 근거의 절반이다(스펙 5.1).
    pub pairing_window: ble::pairing::PairingWindow,
    pub paired_peers: Vec<ble::pairing::PairedPeer>,
}

pub struct BleHandle {
    pub bridge: Mutex<BleBridge>,
    #[cfg(target_os = "macos")]
    pub peripheral: std::sync::Arc<ble::macos::MacPeripheral>,
    pub advertising: AtomicBool,
    // tracing 에는 이 크레이트에 subscriber 가 없어 아무 데도 남지 않는다 —
    // 권한 거부/미지원 같은 오류를 프론트가 표시할 수 있도록 여기 보관한다.
    pub last_error: std::sync::Mutex<Option<String>>,
}

#[tauri::command]
async fn ble_status(state: tauri::State<'_, Arc<BleHandle>>) -> Result<BleStatus, String> {
    let bridge = state.bridge.lock().await;
    #[cfg(target_os = "macos")]
    let peers = state
        .peripheral
        .subscribers()
        .into_iter()
        .map(|s| BlePeer { id: s.id.0, mtu: s.max_notify_len })
        .collect();
    #[cfg(not(target_os = "macos"))]
    let peers = Vec::new();
    let now = std::time::SystemTime::now();
    Ok(BleStatus {
        supported: BLE_SUPPORTED,
        enabled: bridge.is_enabled(),
        advertising: state.advertising.load(Ordering::Relaxed),
        peers,
        last_error: state.last_error.lock().unwrap().clone(),
        pairing_window: bridge.pairing_window(now),
        paired_peers: bridge.paired_peers(),
    })
}

/// 사용자가 Devices 탭에서 [페어링 시작] 을 눌렀을 때만 호출한다. 이 버튼이
/// 없으면 3단계의 보안 근거(스펙 5.1: 코드는 사용자 제스처에서만 발급)가
/// 성립하지 않는다 — 반환된 코드는 `BleStatus.pairing_window` 로만 화면에
/// 흐르고, BLE 로는 절대 나가지 않는다.
#[tauri::command]
async fn ble_begin_pairing(state: tauri::State<'_, Arc<BleHandle>>) -> Result<(), String> {
    let mut bridge = state.bridge.lock().await;
    bridge.begin_pairing(std::time::SystemTime::now());
    Ok(())
}

/// 기기 하나만 해제한다(스펙 6장). `peer_id` 는 토큰에서 파생된 8자 hex 이며,
/// 토큰 자체는 프론트엔드로 나가지 않는다.
#[tauri::command]
async fn ble_unpair(peer_id: String, state: tauri::State<'_, Arc<BleHandle>>) -> Result<(), String> {
    let mut bridge = state.bridge.lock().await;
    bridge.unpair_peer(&peer_id);
    let path = ble::peers::PeerStore::path();
    ble::peers::PeerStore::save_to(&path, &bridge.stored_peers()).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn ble_unpair_all(state: tauri::State<'_, Arc<BleHandle>>) -> Result<(), String> {
    let mut bridge = state.bridge.lock().await;
    bridge.unpair_all();
    let path = ble::peers::PeerStore::path();
    ble::peers::PeerStore::save_to(&path, &[]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn ble_set_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<BleHandle>>,
) -> Result<(), String> {
    let result = state.bridge.lock().await.set_enabled(enabled);
    if !enabled {
        state.advertising.store(false, Ordering::Relaxed);
        *state.last_error.lock().unwrap() = None;
    }
    // 시작 실패는 last_error(패널 표시)와 Err(프론트의 try/catch) 양쪽으로 알린다.
    if let Err(e) = result {
        let msg = format!("BLE 시작 실패: {e}");
        state.advertising.store(false, Ordering::Relaxed);
        *state.last_error.lock().unwrap() = Some(msg.clone());
        return Err(msg);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn hide_console_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(target_os = "windows"))]
fn hide_console_window(cmd: &mut Command) -> &mut Command {
    cmd
}

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
    // 잘못된 값이 cron으로 굳어 저장되면 job 등록이 조용히 실패하므로 여기서 거른다
    if !matches!(agent.as_str(), "claude" | "codex") {
        return Err(format!("지원하지 않는 agent: {agent}"));
    }
    if hour > 23 || minute > 59 {
        return Err(format!("시각이 올바르지 않습니다: {hour:02}:{minute:02}"));
    }
    if prompt.trim().is_empty() {
        return Err("프롬프트가 비어 있습니다".to_string());
    }
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
    // Windows: claude는 .cmd 래퍼이므로 cmd /C 경유 필요
    // macOS/Linux: 절대경로 우선, 없으면 PATH에서 검색
    #[cfg(target_os = "windows")]
    let r = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "claude", "-p", "ping"])
            .current_dir(home())
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:4319")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        hide_console_window(&mut cmd).spawn()
    };
    #[cfg(not(target_os = "windows"))]
    let r = {
        let bin = find_binary(
            "claude",
            &[
                home().join(".local/bin/claude"),
                PathBuf::from("/opt/homebrew/bin/claude"),
                PathBuf::from("/usr/local/bin/claude"),
            ],
        );
        let mut cmd = Command::new(bin);
        cmd.args(["-p", "ping"])
            .current_dir(home())
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:4319")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        hide_console_window(&mut cmd).spawn()
    };
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

        let mut cmd = Command::new(bin);
        cmd.args([
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
        .stderr(std::process::Stdio::null());

        let mut child = match hide_console_window(&mut cmd).spawn() {
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

async fn check_update_on_startup(app: tauri::AppHandle) {
    tokio::time::sleep(Duration::from_secs(5)).await;

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            tracing::warn!(%e, "updater 초기화 실패");
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            tracing::info!("사용 가능한 업데이트 없음");
            return;
        }
        Err(e) => {
            tracing::warn!(%e, "업데이트 확인 실패");
            return;
        }
    };

    tracing::info!(
        version = %update.version,
        current = env!("CARGO_PKG_VERSION"),
        "업데이트 발견"
    );

    // 자동 설치 대신 사용자에게 먼저 확인받는다.
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let version = update.version.clone();
    let app_for_install = app.clone();
    app.dialog()
        .message(format!(
            "새 버전 {version}이(가) 있습니다.\n지금 업데이트하면 앱이 종료되고 설치 후 재시작됩니다."
        ))
        .title("업데이트 가능")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "지금 업데이트".to_string(),
            "나중에".to_string(),
        ))
        .show(move |confirmed| {
            if !confirmed {
                tracing::info!("사용자가 업데이트를 미룸");
                return;
            }
            // 동의 시에만 다운로드/설치/재시작
            tauri::async_runtime::spawn(async move {
                let mut downloaded = 0;
                let install_result = update
                    .download_and_install(
                        |chunk_length, content_length| {
                            downloaded += chunk_length;
                            tracing::info!(downloaded, ?content_length, "업데이트 다운로드 진행");
                        },
                        || tracing::info!("업데이트 다운로드 완료"),
                    )
                    .await;
                match install_result {
                    Ok(_) => {
                        tracing::info!("업데이트 설치 완료, 앱 재시작");
                        app_for_install.restart();
                    }
                    Err(e) => tracing::warn!(%e, "업데이트 설치 실패"),
                }
            });
        });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (tx, mut rx) = mpsc::unbounded_channel::<TokenEvent>();

    // quota 상태 — Claude: anthropic-ratelimit 헤더(프록시), Codex: rollout rate_limits
    let quota_state = Arc::new(quota_proxy::QuotaState::default());
    quota_state.load_persisted();
    let codex_quota = Arc::new(watchers::codex::CodexQuota::default());
    let codex_quota_ping_running = Arc::new(AtomicBool::new(false));

    // Watchers (Claude + Codex). 둘 다 실패해도 앱은 띄움.
    // Claude Code는 macOS/Windows 모두 ~/.claude/projects 사용 (home()이 OS별 홈 경로 반환)
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

    // 앱 시작 시 즉시 한 번 핑 (persisted 캐시가 낡았을 수 있으므로)
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await; // startup replay 완료 대기
        tracing::info!("시작 시 quota 초기 핑");
        spawn_quota_ping();
    });

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 로그인 시 자동 실행: macOS는 LaunchAgent 방식 사용
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // 중복 실행 방지: 두 번째 실행 시 기존 창을 앞으로 가져오고 종료
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            tracing::info!("두 번째 인스턴스 감지 — 기존 창 포커스");
            if let Some(w) = app.get_webview_window("detail") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .manage(scheduler)
        .invoke_handler(tauri::generate_handler![
            open_detail_window,
            list_trigger_rules,
            add_trigger_rule,
            remove_trigger_rule,
            toggle_trigger_rule,
            fire_trigger_now,
            sync_quota,
            ble_status,
            ble_set_enabled,
            ble_begin_pairing,
            ble_unpair,
            ble_unpair_all,
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

                {
                    let app_handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        check_update_on_startup(app_handle).await;
                    });
                }

                // 창 X 버튼 클릭 → 숨김(hide). 트레이 → Quit로만 완전 종료.
                // Windows에서 X 누르면 프로세스가 죽는 기본 동작을 방지한다.
                {
                    use tauri::Manager;
                    for label in ["detail", "popover"] {
                        if let Some(win) = app.handle().get_webview_window(label) {
                            let win_clone = win.clone();
                            win.on_window_event(move |event| {
                                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                                    api.prevent_close();
                                    let _ = win_clone.hide();
                                }
                            });
                        }
                    }
                }

                // Windows: 시작 시 Detail 창 한 번 표시 (트레이 앱임을 인지할 수 있도록)
                #[cfg(target_os = "windows")]
                {
                    use tauri::Manager;
                    if let Some(w) = app.get_webview_window("detail") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }

                let (ble_tx, mut ble_rx) = mpsc::unbounded_channel::<ble::peripheral::PeripheralEvent>();
                #[cfg(target_os = "macos")]
                let ble_handle = {
                    let periph = std::sync::Arc::new(ble::macos::MacPeripheral::new(
                        app.handle().clone(),
                        ble_tx,
                    ));
                    Arc::new(BleHandle {
                        bridge: Mutex::new(BleBridge::new(periph.clone())),
                        peripheral: periph,
                        advertising: AtomicBool::new(false),
                        last_error: std::sync::Mutex::new(None),
                    })
                };
                #[cfg(not(target_os = "macos"))]
                let ble_handle = {
                    drop(ble_tx);
                    Arc::new(BleHandle {
                        bridge: Mutex::new(BleBridge::new(std::sync::Arc::new(
                            ble::peripheral::FakePeripheral::new(),
                        ))),
                        advertising: AtomicBool::new(false),
                        last_error: std::sync::Mutex::new(None),
                    })
                };
                {
                    use tauri::Manager;
                    app.manage(ble_handle.clone());
                }

                // 앱 시작 시 이미 페어링한 기기 목록을 복원한다. 파일이 없으면(첫 실행)
                // 조용히 넘어가고, 손상됐으면(Task 2) tracing 이 전부 버려지는 이
                // 앱에서 사용자가 알 수 있는 유일한 경로인 last_error 에 싣는다.
                {
                    let path = ble::peers::PeerStore::path();
                    match ble::peers::PeerStore::load_from(&path) {
                        ble::peers::LoadOutcome::Missing => {}
                        ble::peers::LoadOutcome::Loaded(peers) => {
                            ble_handle.bridge.blocking_lock().load_peers(peers);
                        }
                        ble::peers::LoadOutcome::Corrupt { detail } => {
                            ble_handle.last_error.lock().unwrap().replace(format!(
                                "저장된 페어링 목록이 손상돼 초기화됐습니다: {detail}"
                            ));
                        }
                    }
                }

                // BLE 이벤트 → 구독자 사본 갱신 + 프론트로 상태 push
                {
                    let h = ble_handle.clone();
                    let app_for_ble = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(ev) = ble_rx.recv().await {
                            #[cfg(target_os = "macos")]
                            h.peripheral.apply_event(&ev);
                            match &ev {
                                ble::peripheral::PeripheralEvent::AdvertisingStarted => {
                                    h.advertising.store(true, Ordering::Relaxed);
                                    *h.last_error.lock().unwrap() = None;
                                }
                                ble::peripheral::PeripheralEvent::PoweredOff => {
                                    h.advertising.store(false, Ordering::Relaxed);
                                    // 전원이 꺼지면 didUnsubscribeFromCharacteristic 이 오지 않아
                                    // 세션 인가가 실제 연결보다 오래 살아남는다(전체 브랜치
                                    // 리뷰 I-2) — 기기 목록의 "연결됨" 배지가 계속 거짓말하고,
                                    // 전원을 반복해서 껐다 켤 때마다 죽은 central id 가 쌓인다.
                                    h.bridge.lock().await.end_all_sessions();
                                }
                                ble::peripheral::PeripheralEvent::Error(e) => {
                                    h.advertising.store(false, Ordering::Relaxed);
                                    *h.last_error.lock().unwrap() = Some(e.clone());
                                    tracing::error!("BLE 오류: {e}");
                                }
                                ble::peripheral::PeripheralEvent::AuthWrite { central, data } => {
                                    let now = std::time::SystemTime::now();
                                    let saved = h.bridge.lock().await.handle_auth(central, data, now);
                                    if let Some(tokens) = saved {
                                        let path = ble::peers::PeerStore::path();
                                        if let Err(e) = ble::peers::PeerStore::save_to(&path, &tokens) {
                                            // ble_unpair 류는 Result 로 프론트에 실패를 알리지만, 이
                                            // 경로는 사용자 커맨드가 아니라 이벤트 루프라 그 통로가
                                            // 없다 — PeripheralEvent::Error 와 같은 방식으로
                                            // last_error 에 실어야 사용자가 알 수 있다(tracing 출력은
                                            // 이 앱에서 전부 유실된다).
                                            *h.last_error.lock().unwrap() =
                                                Some(format!("페어링 토큰 저장 실패: {e}"));
                                            tracing::error!(%e, "ble-peers.json 저장 실패");
                                        }
                                    }
                                }
                                ble::peripheral::PeripheralEvent::Disconnected(central) => {
                                    h.bridge.lock().await.forget_central(central);
                                }
                                _ => {}
                            }
                            let _ = app_for_ble.emit("ble_status", ());
                        }
                    });
                }

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
                let ble_for_tick = ble_handle.clone();
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
                        let now = std::time::SystemTime::now();
                        let mut g = gate_for_tick.lock().await;
                        if g.should_emit(&snap, now) {
                            let _ = app_handle.emit("snapshot", &snap);
                        }
                        drop(g);
                        // BLE 미러는 자체 게이트(1Hz)를 가지며, 꺼져 있거나 구독자가 없으면 즉시 반환한다.
                        ble_for_tick.bridge.lock().await.on_snapshot(&snap, now);
                    }
                });

                Ok(())
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
