mod aggregator;
mod peers;
mod ble;
mod crypto;
mod clock;
mod emitter;
mod lan;
mod network;
mod quota_proxy;
mod settings;
mod tray;
mod types;
mod watchers;

use aggregator::Aggregator;
use clock::SystemClock;
use emitter::EmitGate;
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
}

/// BLE 와 네트워크가 공유하는 페어링 상태(2026-08-25 스펙). 전송별 status 에
/// 중복으로 싣지 않고 여기 한 곳에서만 내보낸다 — 창도 기기 목록도 하나다.
pub type SharedPairing = Arc<Mutex<ble::pairing::PairingManager>>;

#[derive(Clone, serde::Serialize)]
pub struct PairingStatus {
    /// 페어링 창 상태. UI 가 만료와 시도 소진을 구분해 보여줘야 한다 — 소진이
    /// 보인다는 것이 창에 소유자를 두지 않기로 한 근거의 절반이다(스펙 5.1).
    pub pairing_window: ble::pairing::PairingWindow,
    pub paired_peers: Vec<ble::pairing::PairedPeer>,
    /// 폰이 QR 로 스캔할 페이로드. **창이 열려 있고 네트워크 공유가 켜져 있을
    /// 때만** Some 이다.
    ///
    /// 이 값을 `begin_pairing` 응답에 한 번만 실어 보내던 초안은 버그였다 —
    /// 창을 연 **뒤에** 네트워크를 켜면 QR 이 영영 안 나왔다. 상태에서 파생
    /// 시키면 그 순간 바로 따라온다.
    pub qr_payload: Option<String>,
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
    Ok(BleStatus {
        supported: BLE_SUPPORTED,
        enabled: bridge.is_enabled(),
        advertising: state.advertising.load(Ordering::Relaxed),
        peers,
        last_error: state.last_error.lock().unwrap().clone(),
    })
}

#[tauri::command]
async fn ble_set_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<BleHandle>>,
    pairing: tauri::State<'_, SharedPairing>,
) -> Result<(), String> {
    // 네트워크와 동시에 켤 수 있다(2026-08-25 스펙) — 예전의 상호 배타 가드는
    // 페어링 창이 전송별로 쪼개지는 걸 막으려던 것인데, 이제 창을 공유하므로
    // 그 이유가 사라졌다.
    let mut bridge = state.bridge.lock().await;
    // 끄기 전에 이 전송이 서비스 중이던 central 을 받아둔다 — stop() 뒤에는
    // 구독자 목록이 비어 알 수 없다. 공유 매니저에서 **이 전송의 세션만**
    // 내려야 네트워크 세션이 살아남는다(스펙 4장).
    let served = if enabled { Vec::new() } else { bridge.served_centrals() };
    let result = bridge.set_enabled(enabled);
    drop(bridge);
    if !enabled {
        pairing.lock().await.end_sessions(&served);
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

/// BLE와 달리 iroh는 크로스플랫폼이라 macOS/Windows 모두 항상 `true` — 프론트가
/// OS를 추측하지 않고 이 값으로 "네트워크" 옵션 노출 여부를 결정한다(BLE의
/// `supported` 필드와 같은 규약).
const NETWORK_SUPPORTED: bool = true;

#[derive(Clone, serde::Serialize)]
pub struct NetworkStatus {
    pub supported: bool,
    pub enabled: bool,
    /// 이 Mac의 iroh EndpointId(공개키). QR 없이도 디버그용으로 노출한다.
    pub endpoint_id: String,
    pub last_error: Option<String>,
}

pub struct NetworkHandle {
    pub bridge: Mutex<network::NetworkBridge>,
    pub endpoint: iroh::Endpoint,
    pub last_error: std::sync::Mutex<Option<String>>,
}

/// LAN 전송의 앱 쪽 손잡이. `NetworkHandle` 과 달리 엔드포인트도 `last_error` 도
/// 밖에 두지 않는다 — 리스너는 토글이 켜져 있는 동안만 존재하고(스펙 4장),
/// 오류는 브리지가 이미 들고 있다(`LanBridge::last_error`).
pub struct LanHandle {
    pub bridge: Mutex<lan::LanBridge>,
}

#[tauri::command]
async fn network_status(state: tauri::State<'_, Arc<NetworkHandle>>) -> Result<NetworkStatus, String> {
    let bridge = state.bridge.lock().await;
    Ok(NetworkStatus {
        supported: NETWORK_SUPPORTED,
        enabled: bridge.is_enabled(),
        endpoint_id: state.endpoint.id().to_string(),
        last_error: state.last_error.lock().unwrap().clone(),
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `EndpointId`(공개키, raw 32바이트) 하나만 QR 에 실으면 iOS 는 discovery 로만
/// Mac 을 찾아야 하는데, 실기기에서 확인된 대로 그것만으로는
/// `IrohError: no addressing information` 로 실패한다 — discovery 서비스에
/// 우리 주소를 등록하는 절차를 따로 두지 않았기 때문이다(Builder 를 명시적
/// 체인으로 구성해서 preset 의 discovery-publish 를 안 탄다). 그래서 QR 에
/// relay URL 과 direct 주소까지 직접 실어 discovery 없이도 dial 되게 한다.
///
/// bind 직후에는 `endpoint.addr()` 가 비어 있을 수 있어(Phase 0 스파이크에서도
/// 확인) 주소가 채워질 때까지 짧게 폴링한다.
async fn wait_for_addr(ep: &iroh::Endpoint) -> iroh::EndpointAddr {
    for _ in 0..60 {
        let addr = ep.addr();
        if !addr.addrs.is_empty() {
            return addr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    ep.addr()
}

/// 공유 페어링 저장소를 디스크에 쓴다. 실패는 호출자가 last_error 로 올린다.
async fn save_paired_peers(pairing: &SharedPairing) -> Result<(), String> {
    let stored: Vec<peers::StoredPeer> = pairing
        .lock()
        .await
        .issued_peers()
        .into_iter()
        .map(|(token, paired_at)| peers::StoredPeer { token, paired_at })
        .collect();
    peers::PeerStore::save_to(&peers::PeerStore::path(), &stored).map_err(|e| e.to_string())
}

/// QR 페이로드를 만든다. 네트워크 공유가 켜져 있을 때만 부른다.
async fn build_qr_payload(handle: &NetworkHandle, code: &str) -> String {
    // iOS 의 EndpointId 는 raw 32바이트로만 만들 수 있어(fromBytes) hex 로
    // 인코딩한다 — iroh 의 z32 Display 포맷을 Swift 쪽에서 다시 파싱할 필요가
    // 없어지고, 기존 PairingClient.swift 의 Data(hexString:) 를 그대로 재사용한다.
    // relay URL/주소 문자열도 같은 이유로 전부 hex 로 실어 퍼센트 인코딩을
    // 아예 피한다.
    let endpoint_id_hex = hex_encode(handle.endpoint.id().as_bytes());
    let addr = wait_for_addr(&handle.endpoint).await;
    let mut params = vec![format!("endpoint={endpoint_id_hex}"), format!("code={code}")];
    for a in &addr.addrs {
        match a {
            iroh::TransportAddr::Relay(url) => {
                params.push(format!("relay={}", hex_encode(url.to_string().as_bytes())));
            }
            iroh::TransportAddr::Ip(sock) => {
                params.push(format!("addr={}", hex_encode(sock.to_string().as_bytes())));
            }
            _ => {}
        }
    }
    format!("aim://pair?{}", params.join("&"))
}

/// 공유 페어링 상태를 읽는다. 창도 기기 목록도 하나뿐이라 전송별 status 와
/// 분리해 여기서만 내보낸다(2026-08-25 스펙 6장).
#[tauri::command]
async fn pairing_status(
    pairing: tauri::State<'_, SharedPairing>,
    network_state: tauri::State<'_, Arc<NetworkHandle>>,
) -> Result<PairingStatus, String> {
    let now = std::time::SystemTime::now();
    let (pairing_window, paired_peers) = {
        let p = pairing.lock().await;
        (p.pairing_window(now), p.paired_peers())
    };
    // 창이 열려 있고 네트워크가 켜져 있을 때만 QR 을 만든다 — 둘 중 하나라도
    // 아니면 그릴 이유가 없다.
    let qr_payload = match &pairing_window {
        ble::pairing::PairingWindow::Open { code, .. }
            if network_state.bridge.lock().await.is_enabled() =>
        {
            Some(build_qr_payload(&network_state, code).await)
        }
        _ => None,
    };
    Ok(PairingStatus { pairing_window, paired_peers, qr_payload })
}

/// 사용자가 Devices 탭에서 [페어링 시작] 을 눌렀을 때만 호출한다. 이 버튼이
/// 없으면 보안 근거(스펙 5.1: 코드는 사용자 제스처에서만 발급)가 성립하지
/// 않는다 — 코드는 `pairing_status` 로만 화면에 흐르고 링크로는 나가지 않는다.
///
/// 창은 **두 전송이 공유한다.** 코드도 QR 도 `pairing_status` 가 창 상태에서
/// 파생시키므로 여기서는 창만 연다.
#[tauri::command]
async fn begin_pairing(pairing: tauri::State<'_, SharedPairing>) -> Result<(), String> {
    pairing.lock().await.begin_pairing(std::time::SystemTime::now());
    Ok(())
}

/// 내려간 세션을 **세 브릿지 모두**에 알리고, 저장소를 갱신한다. 그 central 이
/// 어느 전송에 붙어 있었는지 앱은 모르고 알 필요도 없다 — 모르는 id 는 무시된다.
///
/// **스트림을 먼저 끊고 디스크는 나중에 쓴다.** 순서가 반대면 디스크 I/O 를
/// 기다리는 동안 250ms 틱이 끼어들 수 있고, 네트워크 전송은 봉인(페어링 잠금
/// 안)과 쓰기(잠금 밖)가 나뉘어 있어 인가가 살아 있을 때 봉인해 둔 프레임 한
/// 장이 해제 뒤에 나갈 수 있다. 평문은 아니고 다음 틱에 저절로 낫지만, 디스크를
/// 먼저 기다릴 이유가 없으므로 그냥 순서를 뒤집는다.
///
/// LAN 도 **같은 이유로 저장 앞이다.** 이쪽이 늦으면 그 창 동안 해제된 기기의
/// 소켓이 계속 열려 있고, 인가된 LAN 연결에는 그 자리를 되돌려 줄 시간 상한이
/// 없다(`lan::LanBridge::drop_sessions` 의 doc).
async fn persist_and_drop(
    pairing: &SharedPairing,
    ble_state: &Arc<BleHandle>,
    network_state: &Arc<NetworkHandle>,
    lan_state: &Arc<LanHandle>,
    dropped: Vec<ble::peripheral::CentralId>,
) -> Result<(), String> {
    ble_state.bridge.lock().await.drop_sessions(&dropped);
    network_state.bridge.lock().await.drop_sessions(&dropped);
    lan_state.bridge.lock().await.drop_sessions(&dropped);
    save_paired_peers(pairing).await
}

/// 기기 하나만 해제한다(스펙 6장). `peer_id` 는 토큰에서 파생된 8자 hex 이며,
/// 토큰 자체는 프론트엔드로 나가지 않는다.
#[tauri::command]
async fn unpair(
    peer_id: String,
    pairing: tauri::State<'_, SharedPairing>,
    ble_state: tauri::State<'_, Arc<BleHandle>>,
    network_state: tauri::State<'_, Arc<NetworkHandle>>,
    lan_state: tauri::State<'_, Arc<LanHandle>>,
) -> Result<(), String> {
    let dropped = pairing.lock().await.revoke_peer(&peer_id);
    persist_and_drop(&pairing, &ble_state, &network_state, &lan_state, dropped).await
}

#[tauri::command]
async fn unpair_all(
    pairing: tauri::State<'_, SharedPairing>,
    ble_state: tauri::State<'_, Arc<BleHandle>>,
    network_state: tauri::State<'_, Arc<NetworkHandle>>,
    lan_state: tauri::State<'_, Arc<LanHandle>>,
) -> Result<(), String> {
    let dropped = pairing.lock().await.revoke_all();
    persist_and_drop(&pairing, &ble_state, &network_state, &lan_state, dropped).await
}

/// BLE 와 동시에 켤 수 있다(2026-08-25 스펙) — 페어링 창을 공유하므로 예전의
/// 상호 배타 가드가 필요 없어졌다.
#[tauri::command]
async fn network_set_enabled(
    enabled: bool,
    network_state: tauri::State<'_, Arc<NetworkHandle>>,
    pairing: tauri::State<'_, SharedPairing>,
) -> Result<(), String> {
    let mut bridge = network_state.bridge.lock().await;
    // 끄기 전에 받아둔다 — set_enabled(false) 가 snapshot_senders 를 비운다.
    let served = if enabled { Vec::new() } else { bridge.served_centrals() };
    bridge.set_enabled(enabled);
    drop(bridge);
    if !enabled {
        pairing.lock().await.end_sessions(&served);
        *network_state.last_error.lock().unwrap() = None;
    }
    Ok(())
}

#[tauri::command]
async fn get_settings(
    state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
) -> Result<settings::AppSettings, String> {
    Ok(state.lock().await.clone())
}

/// 워처는 계속 돈다 — 여기서 켜고 끄는 건 다음 틱부터 화면(과 iOS 미러)에
/// 무엇을 내보낼지일 뿐이다. 그래서 다시 켜도 데이터가 끊김 없이 바로 보인다.
#[tauri::command]
async fn set_enabled_agents(
    agents: Vec<AgentKind>,
    state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
) -> Result<(), String> {
    let updated = settings::AppSettings { enabled_agents: agents.into_iter().collect() };
    settings::SettingsStore::save_to(&settings::SettingsStore::path(), &updated)
        .map_err(|e| e.to_string())?;
    *state.lock().await = updated;
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
    let antigravity_quota = Arc::new(watchers::antigravity::AntigravityQuota::default());

    // Watchers (Claude + Codex + Antigravity). 실패해도 앱은 띄움.
    // Claude Code는 macOS/Windows 모두 ~/.claude/projects 사용 (home()이 OS별 홈 경로 반환)
    let claude_root = home().join(".claude/projects");
    let _ = watchers::claude::ClaudeWatcher::spawn(claude_root, tx.clone());
    let codex_db = home().join(".codex/state_5.sqlite");
    let _ = watchers::codex::CodexWatcher::spawn(codex_db, tx.clone(), codex_quota.clone());
    let gemini_root = home().join(".gemini/antigravity-cli");
    let antigravity_conversations = gemini_root.join("conversations");
    let antigravity_summaries = gemini_root.join("conversation_summaries.db");
    let _ = watchers::antigravity::AntigravityWatcher::spawn(
        antigravity_conversations,
        antigravity_summaries,
        tx.clone(),
        antigravity_quota.clone(),
    );

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
    // 사용자가 설정 탭에서 고른, 화면에 표시할 에이전트 종류. 워처는 선택과
    // 무관하게 계속 돈다 — 매 틱마다 스냅샷을 이 목록으로 걸러낼 뿐이다
    // (간단함 우선, 사용자 확인). BLE/네트워크 미러 페이로드가 이 필터링된
    // Snapshot 에서 만들어지므로 iOS 쪽도 자동으로 같은 필터를 반영한다 —
    // 그쪽 코드는 한 줄도 안 건드린다.
    let settings_state = Arc::new(Mutex::new(
        settings::SettingsStore::load_from(&settings::SettingsStore::path()),
    ));
    // BLE 와 네트워크가 공유하는 페어링 상태(2026-08-25 스펙). 창 하나, 코드 하나,
    // 시도 예산 하나 — 그래야 원 스펙 5.2 의 무차별 대입 근거가 두 전송에 걸쳐
    // 그대로 성립한다(공격자는 어느 전송으로 오든 같은 5회를 나눠 쓴다).
    let shared_pairing: SharedPairing = Arc::new(Mutex::new(ble::pairing::PairingManager::new()));

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
        .invoke_handler(tauri::generate_handler![
            open_detail_window,
            sync_quota,
            ble_status,
            ble_set_enabled,
            network_status,
            network_set_enabled,
            // 페어링은 두 전송이 공유한다(2026-08-25 스펙) — 전송별 커맨드가
            // 아니라 앱 레벨 커맨드 하나씩이다.
            pairing_status,
            begin_pairing,
            unpair,
            unpair_all,
            get_settings,
            set_enabled_agents,
        ])
        .setup({
            let aggregator = aggregator.clone();
            let gate = gate.clone();
            let quota_state = quota_state.clone();
            let codex_quota = codex_quota.clone();
            let antigravity_quota = antigravity_quota.clone();
            let settings_state = settings_state.clone();
            let shared_pairing = shared_pairing.clone();
            move |app| {
                use tauri::Manager;
                app.manage(settings_state.clone());
                app.manage(shared_pairing.clone());
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

                // 네트워크(iroh) Endpoint 는 앱 시작 시 한 번만 bind 한다 — BLE 의
                // advertise on/off 와 달리 iroh 소켓 자체를 껐다 켜는 개념이 없고,
                // "공유"가 꺼져 있는 동안은 아래 accept 루프가 들어오는 연결을
                // 즉시 닫는 것으로 충분하다.
                let network_secret = network::identity::load_or_create(&network::identity::path())
                    .expect("네트워크 신원 로드/생성 실패");
                // 빌더 구성은 network::build_endpoint_builder 가 소유한다 —
                // address_lookup(PkarrPublisher) 가 빠지면 폰이 재시작한 Mac 을
                // 영영 못 찾는 버그가 생기므로, 그 이유와 회귀 테스트를 구성
                // 바로 옆에 두기 위해서다.
                let network_endpoint = tauri::async_runtime::block_on(async {
                    network::build_endpoint_builder(network_secret)
                        .bind()
                        .await
                        .expect("iroh Endpoint bind 실패")
                });
                let network_handle = Arc::new(NetworkHandle {
                    bridge: Mutex::new(network::NetworkBridge::new()),
                    endpoint: network_endpoint,
                    last_error: std::sync::Mutex::new(None),
                });
                {
                    use tauri::Manager;
                    app.manage(network_handle.clone());
                }

                // 앱 시작 시 이미 페어링한 기기 목록을 복원한다. 저장소는 두 전송이
                // 공유하며(2026-08-25 스펙 5장), 통합 파일이 아직 없으면 옛 두 파일을
                // 합쳐 만든다 — 기존 사용자가 재페어링하지 않아도 되도록.
                // 파일이 없으면(첫 실행) 조용히 넘어가고, 손상됐으면 tracing 이 전부
                // 버려지는 이 앱에서 사용자가 알 수 있는 유일한 경로인 last_error 에 싣는다.
                {
                    match peers::PeerStore::load_or_migrate(
                        &peers::PeerStore::path(),
                        &peers::PeerStore::legacy_ble_path(),
                        &peers::PeerStore::legacy_network_path(),
                    ) {
                        peers::LoadOutcome::Missing => {}
                        peers::LoadOutcome::Loaded(stored) => {
                            shared_pairing.blocking_lock().load_peers(
                                stored.into_iter().map(|p| (p.token, p.paired_at)).collect(),
                            );
                        }
                        peers::LoadOutcome::Corrupt { detail } => {
                            ble_handle.last_error.lock().unwrap().replace(format!(
                                "저장된 페어링 목록이 손상돼 초기화됐습니다: {detail}"
                            ));
                        }
                    }
                }

                // 연결 accept 루프 — BLE 의 CoreBluetooth 델리게이트 콜백에 대응하지만,
                // iroh 는 콜백이 아니라 직접 도는 루프다. 연결 하나당 태스크를 spawn 해
                // 그 안에서 control 메시지(HELLO/CODE/AUTH/PROOF)를 bi-stream 하나당
                // 하나씩 처리하고, 인가되면 스냅샷 uni-stream 을 연다.
                {
                    let h = network_handle.clone();
                    let app_for_net = app.handle().clone();
                    let pairing_for_net = shared_pairing.clone();
                    tauri::async_runtime::spawn(async move {
                        loop {
                            let Some(incoming) = h.endpoint.accept().await else {
                                break;
                            };
                            let h = h.clone();
                            let app_for_net = app_for_net.clone();
                            let pairing_for_net = pairing_for_net.clone();
                            tauri::async_runtime::spawn(async move {
                                let conn = match incoming.await {
                                    Ok(c) => c,
                                    Err(_) => return,
                                };
                                if !h.bridge.lock().await.is_enabled() {
                                    conn.close(0u32.into(), b"sharing disabled");
                                    return;
                                }
                                let central =
                                    ble::peripheral::CentralId(conn.remote_id().to_string());
                                loop {
                                    let (mut send, mut recv) = match conn.accept_bi().await {
                                        Ok(s) => s,
                                        Err(_) => break,
                                    };
                                    let req = match recv.read_to_end(4096).await {
                                        Ok(r) => r,
                                        Err(_) => break,
                                    };
                                    let now = std::time::SystemTime::now();
                                    let outcome = {
                                        let mut p = pairing_for_net.lock().await;
                                        h.bridge.lock().await.handle_auth(&central, &req, now, &mut p)
                                    };
                                    let _ = send.write_all(&outcome.payload).await;
                                    let _ = send.finish();

                                    if outcome.granted {
                                        if let Err(e) = save_paired_peers(&pairing_for_net).await {
                                            *h.last_error.lock().unwrap() =
                                                Some(format!("페어링 토큰 저장 실패: {e}"));
                                        }
                                    }
                                    if outcome.now_authorized
                                        && !h.bridge.lock().await.has_snapshot_sender(&central)
                                    {
                                        if let Ok(snap_send) = conn.open_uni().await {
                                            h.bridge.lock().await.register_snapshot_sender(
                                                central.clone(),
                                                snap_send,
                                            );
                                        }
                                    }
                                    let _ = app_for_net.emit("network_status", ());
                                }
                                h.bridge.lock().await.forget_central(&central);
                                let _ = app_for_net.emit("network_status", ());
                            });
                        }
                    });
                }

                // LAN(WebSocket) 전송. iroh 와 달리 여기서 여는 것은 이벤트 통로뿐이다 —
                // 리스너 자체는 토글이 켜져 있는 동안만 존재한다(스펙 4장). 통로는
                // 브리지와 수명을 같이해서 수신 루프를 한 번만 걸면 되게 한다.
                let (lan_tx, mut lan_rx) = mpsc::channel(lan::server::EVENT_QUEUE);
                let lan_handle = Arc::new(LanHandle {
                    bridge: Mutex::new(lan::LanBridge::new(lan_tx)),
                });
                {
                    use tauri::Manager;
                    app.manage(lan_handle.clone());
                }

                // LAN 이벤트 → 인증 처리 + 프론트로 상태 push. BLE 루프와 같은 모양이다:
                // 판단은 전부 브리지·PairingManager 에 있고 여기서는 그 결과를 잇는다.
                {
                    let h = lan_handle.clone();
                    let app_for_lan = app.handle().clone();
                    let pairing_for_lan = shared_pairing.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(ev) = lan_rx.recv().await {
                            let ending = {
                                let mut b = h.bridge.lock().await;
                                // 무엇이 끝나는지는 `apply_event` 가 목록에서 지우기
                                // **전에** 물어야 한다(`sessions_to_end` 의 doc).
                                let ending = b.sessions_to_end(&ev);
                                b.apply_event(&ev);
                                ending
                            };
                            if !ending.is_empty() {
                                // 링크가 끊기면 인가도 즉시 사라진다 — BLE 의
                                // `Disconnected` 와 같은 규칙이다. 공유 매니저이므로
                                // 이 전송이 서비스하던 세션만 내린다.
                                pairing_for_lan.lock().await.end_sessions(&ending);
                            }
                            // 프레임은 인증 이전에 아무나 보낼 수 있으므로, 상태를
                            // 실제로 바꿨을 때만 프론트에 알린다. 나머지 이벤트는
                            // 연결·리스너 상태가 바뀐 것이라 언제나 알린다.
                            let mut notable =
                                !matches!(ev, lan::server::ServerEvent::Frame { .. });
                            if let lan::server::ServerEvent::Frame { id, text } = &ev {
                                let now = std::time::SystemTime::now();
                                let outcome = {
                                    let mut p = pairing_for_lan.lock().await;
                                    h.bridge.lock().await.handle_auth(
                                        id,
                                        text.as_bytes(),
                                        now,
                                        &mut p,
                                    )
                                };
                                // `None` 은 우리가 서비스하지 않는 연결이라는 뜻이다 —
                                // 그 경우 브리지가 pairing 을 아예 건드리지 않았다.
                                if let Some(outcome) = outcome {
                                    notable = outcome.changed_visible_state();
                                    h.bridge.lock().await.send_auth_reply(id, outcome.payload);
                                    // **인가 통지가 디스크보다 먼저다.** 아래 토큰
                                    // 저장과 이 통지는 서로 독립인데, 순서가 반대면
                                    // 보안 타이머(인증 시한)의 해제 시점이 디스크
                                    // 속도에 매달린다. 저장이 오래 걸린 채 150초
                                    // 경계에 걸리면, 방금 페어링에 **성공한** 기기가
                                    // 성공하는 순간 끊긴다. 재연결은 사람 없이
                                    // `AUTH2`/`PROOF2` 로 되므로 잠기지는 않지만,
                                    // 보안 판단이 I/O 지연에 의존하는 관계 자체를
                                    // 두지 않는다.
                                    //
                                    // 통지 자체를 빠뜨리면 정상적으로 페어링한
                                    // 기기가 150초 뒤에 끊긴다 — 그것도 미러가
                                    // 잘 나오는 중에.
                                    //
                                    // network 는 여기서 스냅샷 uni-stream 을 열지만
                                    // LAN 은 열 것이 없다 — 스냅샷은 이미 붙어 있는
                                    // 같은 WebSocket 으로 나간다(틱이
                                    // `prepare_snapshot` 으로 대상을 고른다).
                                    if outcome.now_authorized {
                                        h.bridge.lock().await.mark_authorized(id);
                                        tracing::info!(id = %id.0, "LAN 세션 인가됨");
                                    }
                                    if outcome.granted {
                                        // 새 토큰이 발급됐다. 여기서 디스크에 쓰지 않으면
                                        // 이 세션에서는 멀쩡히 동작하다가 맥을 껐다 켜는
                                        // 순간 사라져 영영 재연결하지 못한다.
                                        if let Err(e) = save_paired_peers(&pairing_for_lan).await {
                                            // 이 경로는 사용자 커맨드가 아니라 이벤트 루프라
                                            // Result 로 알릴 통로가 없다 — BLE 와 같은 방식으로
                                            // last_error 에 싣는다(tracing 은 전부 유실된다).
                                            // 오류가 **바뀌었으면** 그것만으로도 알릴
                                            // 이유다. 같은 오류를 다시 쓰는 것은 새
                                            // 소식이 아니다. 지금은 `granted` 안이라
                                            // 이미 알리게 돼 있지만, 이 분기가 밖으로
                                            // 나가도 판단이 함께 따라가도록 둔다.
                                            notable |= h
                                                .bridge
                                                .lock()
                                                .await
                                                .set_last_error(Some(format!(
                                                    "페어링 토큰 저장 실패: {e}"
                                                )));
                                        }
                                    }
                                }
                            }
                            if notable {
                                let _ = app_for_lan.emit("lan_status", ());
                            }
                        }
                    });
                }

                // BLE 이벤트 → 구독자 사본 갱신 + 프론트로 상태 push
                {
                    let h = ble_handle.clone();
                    let app_for_ble = app.handle().clone();
                    let pairing_for_ble = shared_pairing.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(ev) = ble_rx.recv().await {
                            // `apply_event` 는 `PoweredOff` 에서 구독자 사본을 통째로
                            // 비운다. 그 뒤에 읽으면 언제나 빈 목록이므로 반드시
                            // 여기서 먼저 찍어둔다(`sessions_to_end_before` 의 doc).
                            let served_before_apply =
                                h.bridge.lock().await.sessions_to_end_before(&ev);
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
                                    // 공유 매니저이므로 **BLE 가 서비스 중이던 central 만**
                                    // 내린다 — 전체를 지우면 네트워크 세션까지 죽는다.
                                    // 목록은 `apply_event` 가 사본을 비우기 전에 찍어둔
                                    // 것을 쓴다(이 루프 머리의 주석).
                                    // 전송 자원(큐)은 macOS 쪽 `did_update_state` 의
                                    // !powered 분기가 이미 비운다 — 여기서는 세션
                                    // 인가만 내린다.
                                    let served = served_before_apply.unwrap_or_default();
                                    pairing_for_ble.lock().await.end_sessions(&served);
                                }
                                ble::peripheral::PeripheralEvent::Error(e) => {
                                    h.advertising.store(false, Ordering::Relaxed);
                                    *h.last_error.lock().unwrap() = Some(e.clone());
                                    tracing::error!("BLE 오류: {e}");
                                }
                                ble::peripheral::PeripheralEvent::AuthWrite { central, data } => {
                                    let now = std::time::SystemTime::now();
                                    let granted = {
                                        let mut p = pairing_for_ble.lock().await;
                                        h.bridge.lock().await.handle_auth(central, data, now, &mut p)
                                    };
                                    if granted {
                                        if let Err(e) = save_paired_peers(&pairing_for_ble).await {
                                            // unpair 류는 Result 로 프론트에 실패를 알리지만, 이
                                            // 경로는 사용자 커맨드가 아니라 이벤트 루프라 그 통로가
                                            // 없다 — PeripheralEvent::Error 와 같은 방식으로
                                            // last_error 에 실어야 사용자가 알 수 있다(tracing 출력은
                                            // 이 앱에서 전부 유실된다).
                                            *h.last_error.lock().unwrap() =
                                                Some(format!("페어링 토큰 저장 실패: {e}"));
                                        }
                                    }
                                }
                                ble::peripheral::PeripheralEvent::Disconnected(central) => {
                                    // 세션 인가는 공유 매니저에서, 전송 자원은 브릿지에서.
                                    pairing_for_ble.lock().await.end_session(central);
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
                let antigravity_for_tick = antigravity_quota.clone();
                let ble_for_tick = ble_handle.clone();
                let network_for_tick = network_handle.clone();
                let lan_for_tick = lan_handle.clone();
                let settings_for_tick = settings_state.clone();
                let pairing_for_tick = shared_pairing.clone();
                // **이 루프가 하나의 순차 태스크라는 점이 v2 프레임 순서의 근거다.**
                // 봉인 카운터는 `prepare_snapshot` 에서 전진하고 실제 쓰기는
                // `send_prepared` 에서 일어나는데, 둘 사이에 순서를 지키는 장치가
                // 아무것도 없다 — 잠금도, 시퀀스 검사도 없다. 지금 안전한 이유는
                // 오직 틱 N 의 `send_prepared` 가 끝나야 틱 N+1 의
                // `prepare_snapshot` 이 시작되기 때문이다.
                //
                // 그래서 "느린 폰이 틱을 붙잡지 않게" 쓰기를 별도 `spawn` 으로
                // 빼는 최적화는 **그대로 하면 안 된다.** 프레임이 뒤집히면
                // 수신 측은 `counter <= last` 로 조용히 전부 버린다(재전송으로
                // 오인). 그렇게 바꾸려면 central 별 순서 보장(전송 큐 하나씩)을
                // 먼저 만들어야 한다.
                tauri::async_runtime::spawn(async move {
                    let clock = SystemClock;
                    let mut ticker = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        ticker.tick().await;
                        let mut a = agg_for_tick.lock().await;
                        let mut snap = a.snapshot(&clock);
                        drop(a);
                        // 사용자가 설정 탭에서 고른 종류만 남긴다. 워처는 선택과 무관하게
                        // 계속 돌므로 다시 켜도 데이터가 끊김 없이 바로 보인다. BLE/네트워크
                        // 미러 페이로드가 이 Snapshot 에서 만들어지므로 iOS 쪽도 자동으로
                        // 같은 필터를 반영한다.
                        {
                            let enabled = settings_for_tick.lock().await;
                            snap.agents.retain(|ag| enabled.enabled_agents.contains(&ag.kind));
                        }
                        // 실제 quota 주입(있을 때만): Claude=프록시 헤더, Codex=rollout rate_limits, Antigravity=gen_metadata 버킷
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
                        if let Some(c) = snap.agents.iter_mut().find(|a| a.kind == AgentKind::Antigravity)
                        {
                            if let Some(u) = *antigravity_for_tick.used_pct_5h.lock().unwrap() {
                                c.quota_used_pct = Some(u);
                            }
                            if let Some(r) = *antigravity_for_tick.reset_5h.lock().unwrap() {
                                c.quota_reset_at = Some(r);
                            }
                            if let Some(u) = *antigravity_for_tick.used_pct_weekly.lock().unwrap() {
                                c.quota_used_pct_weekly = Some(u);
                            }
                            if let Some(r) = *antigravity_for_tick.reset_weekly.lock().unwrap() {
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
                        {
                            // 두 전송 모두 공유 페어링 상태를 **가변으로** 받는다 —
                            // 인가 필터에 더해 v2 세션의 봉인 카운터를 전진시켜야
                            // 하기 때문이다. 잠금 순서는 페어링 → 브릿지로,
                            // 인증 경로와 같다.
                            let mut p = pairing_for_tick.lock().await;
                            // BLE 는 큐에 넘기고 끝나는 동기 호출이라 잠금 안에서
                            // 마쳐도 된다. 네트워크와 LAN 은 봉인까지만 여기서 한다.
                            ble_for_tick.bridge.lock().await.on_snapshot(&snap, now, &mut p);
                            let lines = network_for_tick
                                .bridge
                                .lock()
                                .await
                                .prepare_snapshot(&snap, now, &mut p);
                            let lan_frames = lan_for_tick
                                .bridge
                                .lock()
                                .await
                                .prepare_snapshot(&snap, now, &mut p);
                            // **쓰기 전에 반드시 놓는다.** QUIC 흐름 제어에 막히면
                            // write_all 이 수십 초 걸릴 수 있고(폰이 백그라운드로
                            // 가면 흔하다), 그동안 잠금을 쥐고 있으면 페어링
                            // 시작·BLE/네트워크 인증 처리가 전부 멈춘다.
                            drop(p);
                            network_for_tick.bridge.lock().await.send_prepared(lines).await;
                            // LAN 의 쓰기는 오늘 막히지 않지만(무한 채널에 넣고
                            // 끝난다) 잠금 밖이라는 위치는 지킨다 — 세 전송이 같은
                            // 모양이어야 한쪽만 고치는 드리프트가 눈에 띈다.
                            lan_for_tick.bridge.lock().await.send_prepared(lan_frames).await;
                        }
                    }
                });

                Ok(())
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
