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
    atomic::{AtomicBool, AtomicU64, Ordering},
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
    settings_state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
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
    if enabled {
        pairing.lock().await.begin_pairing(std::time::SystemTime::now());
    } else {
        let mut p = pairing.lock().await;
        p.end_sessions(&served);
        p.reset_pairing_window();
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
    // 설정 저장
    let mut guard = settings_state.lock().await;
    guard.ble_enabled = enabled;
    let _ = settings::SettingsStore::save_to(&settings::SettingsStore::path(), &guard);
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

/// LAN 전송은 **모든 플랫폼에서 지원된다** — `NETWORK_SUPPORTED` 와 같은 무조건
/// `true` 이고, `BLE_SUPPORTED` 처럼 `cfg` 로 갈라지지 않는다.
///
/// BLE 가 macOS 에서만 `true` 인 이유는 CoreBluetooth 가 그 OS 에만 있고 다른
/// 곳에서는 `FakePeripheral` 이라 토글이 성공을 보고해도 아무 일도 일어나지 않기
/// 때문이다. LAN 에는 그런 사정이 없다: 여는 것은 그냥 WebSocket 리스너이고,
/// 게시(`lan::discovery`)와 주소 조회(`local_ipv4`)도 서브프로세스 없이 표준
/// 라이브러리만 쓴다 — 윈도우에서 `ipconfig` 출력을 파싱하지 않기로 한 결정이
/// 바로 이 값을 무조건 `true` 로 둘 수 있게 하려던 것이다. 여기에 `cfg` 를
/// 붙이면 그 작업이 통째로 버려진다.
///
/// 윈도우에서 실제로 돌려 본 사람은 아직 없다 — 그 확인은
/// `docs/ble-protocol/DEVICE-TEST.md` §8-4 에 있다.
const LAN_SUPPORTED: bool = true;

#[derive(Clone, serde::Serialize)]
pub struct LanStatus {
    pub supported: bool,
    pub enabled: bool,
    /// 사용자가 기기에 **손으로 넣을 주소**. `192.168.0.12:4320` 처럼 **포트까지
    /// 붙은 완성된 문자열**이다.
    ///
    /// 포트를 여기서 붙이는 이유는 프론트가 `4320` 을 베껴 적지 않게 하기
    /// 위해서다 — 그 값은 `lan::server::PORT` 한 곳에서만 나와야 하고, 상수가
    /// 움직이는 날 패널이 거짓말을 하면 안 된다.
    ///
    /// **리스너가 서 있을 때만** 값이 있다 — `enabled` 와 갈라진다: bind 에
    /// 실패하면 토글은 켜진 채로 여기가 `None` 이 된다(`lan_address` 의 doc).
    /// 라우팅 가능한 IPv4 를 못 찾아도 `None` 이다(`local_ipv4` 의 한계 참고).
    pub address: Option<String>,
    pub last_error: Option<String>,
}

/// 패널에 띄울 주소 문자열을 만든다.
///
/// **리스너가 서 있지 않으면 `None`.** 토글이 아니라 리스너를 따른다 — 둘은
/// 갈라진다: `BindFailed` 는 `enabled` 를 켠 채로 리스너만 없앤다. 그 상태에서
/// 주소를 계속 보여주면 패널이 같은 화면에서 "포트를 열지 못했습니다"와 "이
/// 주소를 직접 넣으세요"를 동시에 말하고, 사용자는 **열려 있지 않은 포트**를
/// 기기에 손으로 넣는다. 기기 쪽에는 왜 안 붙는지 알려 줄 화면이 없다.
///
/// 이것은 이 전송이 광고에 대해 이미 정해 둔 규칙과 같다
/// (`lan::LanBridge::advertise` 의 doc — "광고는 토글이 아니라 리스너를 따른다").
/// mDNS 와 손으로 넣는 길이 향하는 곳은 같은 포트이므로 규칙도 같아야 한다.
/// 호출부는 `lan::LanBridge::is_listening` 을 넘긴다.
///
/// 리스너가 서 있는 동안에는 **mDNS 게시가 성공했든 실패했든 언제나** 값을 준다 —
/// 방화벽이 5353 을 막은 경우는 아무 신호도 남기지 않으므로(`lan::discovery`
/// 모듈 doc), 손으로 넣는 길을 게시 실패 표시에 매달면 가장 필요한 순간에 아무
/// 말도 하지 않게 된다(DEVICE-TEST §8-3 의 요구사항). 즉 이 함수가 보는 것은
/// "리스너가 있는가" 하나이지 "광고가 나가는가"가 아니다.
///
/// `port` 를 인자로 받는 것은 테스트가 하드코딩된 리터럴을 잡아내게 하기
/// 위해서다 — 호출부는 언제나 `lan::server::PORT` 를 넘긴다.
fn lan_address(listening: bool, ip: Option<String>, port: u16) -> Option<String> {
    if !listening {
        return None;
    }
    Some(format!("{}:{}", ip?, port))
}

/// Claude/Codex/Antigravity 쿼터 %는 실시간 계산이 아니라 마지막으로
/// **관측된**(프록시 헤더, rollout rate_limits, gen_metadata) 값의 캐시다 —
/// 그 창이 리셋된 뒤에도 다음 실제 API 응답이 관측될 때까지는 옛 값이 그대로
/// 남는다(2026-09-02, 리셋 후 한참 지나도 100%가 안 내려가는 것을 실사용으로
/// 확인). `reset_at` 이 이미 지났으면 그건 만료된 창의 값이니 0%로 보여준다
/// (새 창을 "이미 다 썼다"고 보여주는 것보다 "아직 안 썼다"고 보여주는 쪽이
/// 훨씬 덜 위험하다).
///
/// `reset_at` 자체는 캐시가 여전히 옛 창을 가리키고 있을 수 있으므로 여기서
/// 덮어쓰지 않는다 — 아그리게이터의 실시간 anchor 추정(`current_window_anchor`)
/// 이 새 창을 이미 더 정확히 잡고 있어, 호출부가 `None` 을 받으면 그 추정치를
/// 그대로 둔다.
fn quota_pct_for_tick(
    cached_pct: Option<f32>,
    cached_reset_at: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> (Option<f32>, Option<std::time::SystemTime>) {
    match cached_reset_at {
        Some(r) if r > now => (cached_pct, Some(r)),
        Some(_) => (Some(0.0), None),
        None => (cached_pct, None),
    }
}

#[tauri::command]
async fn lan_status(state: tauri::State<'_, Arc<LanHandle>>) -> Result<LanStatus, String> {
    let bridge = state.bridge.lock().await;
    Ok(LanStatus {
        supported: LAN_SUPPORTED,
        // 토글의 상태 그대로다. `BindFailed` 가 리스너를 내려도 사용자가 켠 것은
        // 켠 것이고, 패널은 그것을 켜짐으로 보여주면서 빨간 오류를 함께 띄운다.
        enabled: bridge.is_enabled(),
        // 주소는 **리스너를 따른다**(`lan_address` 의 doc). 리스너가 없을 때도
        // `local_ipv4()` 는 불리지만(인자가 먼저 평가된다) 그 비용은 UDP 소켓 하나를
        // 묶고 경로를 묻는 것뿐이고 패킷은 나가지 않는다 — 판단을 두 곳에 두지 않는
        // 쪽을 택했다.
        address: lan_address(
            bridge.is_listening(),
            lan::discovery::local_ipv4(),
            lan::server::PORT,
        ),
        // BLE·network 와 달리 오류는 핸들이 아니라 브리지가 들고 있다.
        last_error: bridge.last_error(),
    })
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
/// **LAN 에도 같은 창이 있고, 오히려 더 넓다.** LAN 도 봉인(`prepare_snapshot`,
/// 페어링 잠금 안)과 쓰기(`send_prepared`, 잠금 밖)가 나뉘어 있고, 틱은 그 둘
/// 사이에 network 의 쓰기를 **통째로** 끼워 넣는다 — `drop(p)` 다음에
/// `network.send_prepared(...).await` 가 돌고, `lan.send_prepared(...)` 는 그
/// 뒤에야 돈다. 그 간격에 해제가 끼어들면 해제 **이전에** 봉인된 스냅샷 한 장이
/// 해제 **이후에** 나간다. "`revoke_peer` 가 인가를 먼저 지운다"는 이 창을 닫는
/// 근거가 되지 못한다 — network 에도 똑같이 참이고, 어느 쪽이든 봉인은 그보다
/// 앞서 이미 끝나 있기 때문이다.
///
/// 실피해는 작다: 한 장, 그 기기가 이미 가진 키로 봉인된 것, 다음 틱에 저절로
/// 낫는다(`Outbound::Close` 가 먼저 큐에 들어간 순서라면 tungstenite 가 그
/// 프레임을 아예 막기도 하지만, 그 순서는 보장되지 않는다). 그래서 **오늘의
/// 배치가 맞고 여기서 고칠 것은 없다.** 다만 "LAN 은 안전하니 저 줄만 저장 뒤로
/// 밀어도 된다"는 근거로는 쓸 수 없다는 뜻이다. 자리 회수는 이 배치와 인과가
/// 없다: 저장 뒤로 밀려도 `drop_sessions` 는 디스크 쓰기 바로 다음에 도니 자리는
/// 밀리초 늦게 돌아올 뿐이다.
///
/// 그런데도 세 줄을 한자리에 붙여 두는 이유는 **배선이 갈라지면 아무도 눈치채지
/// 못하기 때문**이다. 이 함수는 통째로 테스트 사각지대에 있다 — 세 줄 중 무엇을
/// 지워도 전체 스위트가 그대로 통과한다(실측). 그러니 어느 한 줄이 저장 뒤로
/// 밀리는 날 그것을 잡아 줄 것은 "셋이 나란히 있다"는 이 모양뿐이다.
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
    settings_state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
) -> Result<(), String> {
    let mut bridge = network_state.bridge.lock().await;
    // 끄기 전에 받아둔다 — set_enabled(false) 가 snapshot_senders 를 비운다.
    let served = if enabled { Vec::new() } else { bridge.served_centrals() };
    bridge.set_enabled(enabled);
    drop(bridge);
    if enabled {
        pairing.lock().await.begin_pairing(std::time::SystemTime::now());
    } else {
        let mut p = pairing.lock().await;
        p.end_sessions(&served);
        p.reset_pairing_window();
        *network_state.last_error.lock().unwrap() = None;
    }
    // 설정 저장
    let mut guard = settings_state.lock().await;
    guard.network_enabled = enabled;
    let _ = settings::SettingsStore::save_to(&settings::SettingsStore::path(), &guard);
    Ok(())
}

/// 세 토글은 서로 독립이다 — LAN 을 켜고 끄는 것이 BLE·network 를 건드리지
/// 않는다. `network_set_enabled` 와 같은 모양을 일부러 유지한다.
///
/// **실패는 여기서 나오지 않는다.** `LanBridge::set_enabled(true)` 는 리스너
/// 태스크를 띄우고 곧바로 돌아오므로, 포트 점유(`EADDRINUSE`) 같은 실패는 잠시 뒤
/// `ServerEvent::BindFailed` 로 온다. 그 경로는 이미 이어져 있다 — `apply_event`
/// 가 `last_error` 를 채우고 배선이 `lan_status` 이벤트를 쏘면 패널이 다시 읽는다
/// (setup 의 LAN 이벤트 루프). 그래서 여기서 성공을 돌려준 뒤에 빨간 오류가 뜨는
/// 것이 정상이고, 프론트가 폴링할 이유가 없다.
#[tauri::command]
async fn lan_set_enabled(
    enabled: bool,
    lan_state: tauri::State<'_, Arc<LanHandle>>,
    pairing: tauri::State<'_, SharedPairing>,
    settings_state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
) -> Result<(), String> {
    let mut bridge = lan_state.bridge.lock().await;
    // 끄기 전에 받아둔다 — set_enabled(false) 가 목록을 비운다. 공유 매니저이므로
    // **이 전송의 세션만** 내려야 BLE·network 세션이 살아남는다(스펙 4장).
    let served = if enabled { Vec::new() } else { bridge.served_centrals() };
    bridge.set_enabled(enabled);
    drop(bridge);
    if enabled {
        pairing.lock().await.begin_pairing(std::time::SystemTime::now());
    } else {
        let mut p = pairing.lock().await;
        p.end_sessions(&served);
        p.reset_pairing_window();
        // `last_error` 를 지우는 것은 브리지가 스스로 한다(`set_enabled` 의 doc) —
        // BLE·network 처럼 배선이 기억해야 하는 정리를 여기 두지 않는다.
    }
    // 설정 저장
    let mut guard = settings_state.lock().await;
    guard.lan_enabled = enabled;
    let _ = settings::SettingsStore::save_to(&settings::SettingsStore::path(), &guard);
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
    let mut guard = state.lock().await;
    let mut updated = guard.clone();
    updated.enabled_agents = agents.into_iter().collect();
    settings::SettingsStore::save_to(&settings::SettingsStore::path(), &updated)
        .map_err(|e| e.to_string())?;
    *guard = updated;
    Ok(())
}

#[tauri::command]
async fn set_antigravity_poll_interval(
    seconds: u64,
    state: tauri::State<'_, Arc<Mutex<settings::AppSettings>>>,
    poll_interval: tauri::State<'_, Arc<AtomicU64>>,
) -> Result<(), String> {
    if !(60..=3600).contains(&seconds) {
        return Err("Antigravity 갱신 주기는 1분~60분 사이여야 합니다.".into());
    }
    let mut guard = state.lock().await;
    let mut updated = guard.clone();
    updated.antigravity_poll_interval_secs = seconds;
    settings::SettingsStore::save_to(&settings::SettingsStore::path(), &updated)
        .map_err(|e| e.to_string())?;
    *guard = updated;
    poll_interval.store(seconds, Ordering::Relaxed);
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn hide_console_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn hide_console_window(cmd: &mut Command) -> &mut Command {
    cmd
}

pub(crate) fn home() -> PathBuf {
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

// claude -p "/usage" 로 계정 전체(다른 기기·claude.ai 사용량까지 포함) 5h/주간
// 사용률을 읽어온다. 이전엔 -p ping(실제 채팅 완성, 진짜 토큰 소모)으로 프록시
// 트래픽을 만들어 응답 헤더를 훔쳐봤지만, /usage 는 계정 한도 조회 전용 커맨드라
// 연속 호출해도 quota %가 안 움직인다(2026-09-03 확인) — 활동 여부와 무관하게
// 상시 돌려도 quota를 갉아먹지 않는다.
//
// 2026-09-03 리뷰 반영: 이 커맨드는 stdout 텍스트를 직접 파싱하지, 응답 헤더를
// 훔쳐보는 게 아니므로 ANTHROPIC_BASE_URL(로컬 프록시)을 거칠 이유가 없다 —
// 오히려 그러면 유휴 시 상시 갱신이라는 이 함수의 존재 이유 자체가 프록시
// 가용성에 묶여버린다(프록시가 안 떠 있으면 /usage 도 실패). 그래서 이 호출은
// 프록시를 거치지 않고 실제 Anthropic 엔드포인트로 바로 나간다.
async fn run_claude_usage_ping(quota: Arc<quota_proxy::QuotaState>, running: Arc<AtomicBool>) {
    // 수동 동기화 버튼과 10분 주기 자동 동기화가 겹치면 claude 프로세스가
    // 중복 실행될 수 있어(2026-09-03 리뷰), Codex 쪽과 같은 in-flight 가드를 쓴다.
    if running.swap(true, Ordering::SeqCst) {
        tracing::debug!("quota 동기화(/usage) 이미 진행 중 — 건너뜀");
        return;
    }
    run_claude_usage_ping_inner(quota).await;
    running.store(false, Ordering::SeqCst);
}

async fn run_claude_usage_ping_inner(quota: Arc<quota_proxy::QuotaState>) {
    // Windows: claude는 .cmd 래퍼이므로 cmd /C 경유 필요
    // macOS/Linux: 절대경로 우선, 없으면 PATH에서 검색
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "claude", "-p", "/usage"]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let bin = find_binary(
            "claude",
            &[
                home().join(".local/bin/claude"),
                PathBuf::from("/opt/homebrew/bin/claude"),
                PathBuf::from("/usr/local/bin/claude"),
            ],
        );
        let mut c = Command::new(bin);
        c.args(["-p", "/usage"]);
        c
    };
    cmd.current_dir(home())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    hide_console_window(&mut cmd);

    let output = match tokio::time::timeout(Duration::from_secs(60), cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(%e, "quota 동기화(/usage) 실행 실패 (claude 미발견?)");
            *quota.last_error.lock().unwrap() =
                Some(format!("claude 실행 실패 — 설치돼 있나요? ({e})"));
            return;
        }
        Err(_) => {
            tracing::warn!("quota 동기화(/usage) timeout");
            *quota.last_error.lock().unwrap() = Some("claude /usage 응답 시간 초과".to_string());
            return;
        }
    };
    if !output.status.success() {
        tracing::warn!(status = ?output.status, "quota 동기화(/usage) 실패");
        *quota.last_error.lock().unwrap() =
            Some("Claude 한도 조회 실패 — 로그인 상태를 확인하세요 (`claude /login`)".to_string());
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (session_pct, week_pct) = quota_proxy::parse_usage_pct(&stdout);
    if session_pct.is_none() && week_pct.is_none() {
        // CLI 출력 포맷이 바뀌면 여기서 조용히 계속 실패할 수 있다 — 매번 로그를
        // 남겨 무음 회귀를 감지할 수 있게 한다(2026-09-03 리뷰).
        tracing::warn!(%stdout, "quota 동기화(/usage) 출력 파싱 실패 — CLI 출력 포맷이 바뀌었을 수 있다");
        *quota.last_error.lock().unwrap() =
            Some("claude /usage 출력을 해석하지 못했습니다 (CLI 업데이트?)".to_string());
        return;
    }
    quota.apply_usage_pct(session_pct, week_pct);
    tracing::info!(?session_pct, ?week_pct, "quota 동기화(/usage) 완료");
}

// Codex quota는 rollout의 token_count.rate_limits에서만 갱신된다 — 사용자가 유휴
// 상태면 새 rollout 이벤트가 없어 카드의 %가 계속 늙는다.
//
// 2026-09-04까지는 `codex exec "Reply exactly with the word ok"` 로 억지 턴을 만들어
// 갱신했는데, 그건 **quota를 재려고 quota를 태우는** 짓이었다(게다가 rollout에 잡동사니
// 세션이 쌓인다). 지금은 app-server의 조회 전용 RPC `account/rateLimits/read`로 토큰을
// 쓰지 않고 같은 스냅샷을 받는다 — Claude `/usage` 안전망과 같은 자리지만 이쪽은
// stdout 텍스트가 아니라 타입 있는 JSON이라 CLI 출력 포맷 변화에 조용히 깨지지 않는다.
// 자세한 내용은 watchers/codex_rpc.rs 문서 참고.
fn codex_binary() -> String {
    find_binary(
        "codex",
        &[
            home().join(".local/bin/codex"),
            PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
        ],
    )
}

/// Codex quota 조회 in-flight 가드를 tauri 상태로 넣기 위한 newtype.
/// `Arc<AtomicBool>` 을 그대로 manage 하면 Claude 쪽 가드와 **타입이 같아 충돌**한다
/// (tauri 상태는 타입으로 찾는다) — 두 번째 manage 는 조용히 무시된다.
pub struct CodexPingGuard(pub Arc<AtomicBool>);

// Claude 쪽과 같은 in-flight 가드 — 수동 버튼과 주기 동기화가 겹쳐도 app-server
// 프로세스가 중복으로 뜨지 않는다.
async fn run_codex_quota_ping(
    quota: Arc<watchers::codex::CodexQuota>,
    running: Arc<AtomicBool>,
) {
    if running.swap(true, Ordering::SeqCst) {
        tracing::debug!("codex quota 동기화 이미 진행 중 — 건너뜀");
        return;
    }
    watchers::codex_rpc::poll_rate_limits(&codex_binary(), &home(), &quota).await;
    running.store(false, Ordering::SeqCst);
}

// 수동 동기화 (UI 버튼). 10분 주기 자동 동기화와 같은 in-flight 가드를 공유해
// 겹쳐 눌러도 claude 프로세스가 중복 실행되지 않는다.
#[tauri::command]
async fn sync_quota(
    quota: tauri::State<'_, Arc<quota_proxy::QuotaState>>,
    running: tauri::State<'_, Arc<AtomicBool>>,
) -> Result<(), String> {
    run_claude_usage_ping(quota.inner().clone(), running.inner().clone()).await;
    Ok(())
}

#[tauri::command]
async fn sync_codex_quota(
    quota: tauri::State<'_, Arc<watchers::codex::CodexQuota>>,
    running: tauri::State<'_, CodexPingGuard>,
) -> Result<(), String> {
    run_codex_quota_ping(quota.inner().clone(), running.0.clone()).await;
    Ok(())
}

#[tauri::command]
async fn sync_antigravity_quota(
    quota: tauri::State<'_, Arc<watchers::antigravity::AntigravityQuota>>,
) -> Result<(), String> {
    let quota = quota.inner().clone();
    tokio::task::spawn_blocking(move || watchers::antigravity::poll_usage(&quota))
        .await
        .map_err(|e| e.to_string())?;
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
    let claude_quota_ping_running = Arc::new(AtomicBool::new(false));
    let codex_quota = Arc::new(watchers::codex::CodexQuota::default());
    let codex_quota_ping_running = Arc::new(AtomicBool::new(false));
    let antigravity_quota = Arc::new(watchers::antigravity::AntigravityQuota::default());
    let initial_settings = settings::SettingsStore::load_from(&settings::SettingsStore::path());
    let antigravity_poll_interval = Arc::new(AtomicU64::new(
        initial_settings.antigravity_poll_interval_secs,
    ));
    let settings_state = Arc::new(Mutex::new(initial_settings));

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
        antigravity_poll_interval.clone(),
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
    // BLE 와 네트워크가 공유하는 페어링 상태(2026-08-25 스펙). 창 하나, 코드 하나,
    // 시도 예산 하나 — 그래야 원 스펙 5.2 의 무차별 대입 근거가 두 전송에 걸쳐
    // 그대로 성립한다(공격자는 어느 전송으로 오든 같은 5회를 나눠 쓴다).
    let shared_pairing: SharedPairing = Arc::new(Mutex::new(ble::pairing::PairingManager::new()));

    // 앱 시작 시 즉시 한 번 핑 (persisted 캐시가 낡았을 수 있으므로)
    {
        let quota = quota_state.clone();
        let running = claude_quota_ping_running.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await; // startup replay 완료 대기
            tracing::info!("시작 시 quota 초기 핑");
            run_claude_usage_ping(quota, running).await;
        });
    }

    // 주기적 자동 동기화: /usage는 어디까지나 보험용 안전망이다 — 실사용 중이면
    // 프록시가 실제 헤더로 이미 훨씬 자주 갱신해주므로, 매번 새로 프로세스를 띄울
    // 필요가 없다(2026-09-03: 문자열 파싱이라 신뢰도도 헤더보다 낮다). 그래서 60초
    // 마다 깨어나 "마지막 갱신 이후 10분이 지났는지"만 확인하고, 그때만 /usage를
    // 부른다 — 완전히 손 놓고 있을 때만 실제로 실행된다.
    const QUOTA_STALE_AFTER: Duration = Duration::from_secs(600);
    {
        let quota = quota_state.clone();
        let running = claude_quota_ping_running.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await; // 시작 시 startup replay 완료 대기
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                if !quota.is_stale(std::time::SystemTime::now(), QUOTA_STALE_AFTER) {
                    continue;
                }
                tracing::info!("10분간 갱신 없음 — 안전망 quota 핑");
                run_claude_usage_ping(quota.clone(), running.clone()).await;
            }
        });
    }

    // Codex는 사용자가 아무 입력을 하지 않으면 새 rate_limits 이벤트가 기록되지 않는다.
    // 10분마다 조회 전용 RPC로 서버 상태를 새로 받아온다(interval 의 첫 틱이 곧
    // 시작 시 1회 조회 역할을 한다).
    //
    // 예전엔 "한 번이라도 quota를 관측한 뒤에만" 핑했다 — exec 핑이 진짜 토큰을
    // 태웠으니 아낄 이유가 있었다. 지금은 공짜 조회라 그 가드가 오히려 해롭다:
    // 로그인이 안 돼 있으면 rollout 에 rate_limits 가 영영 안 찍히고, 그러면
    // 가드 때문에 조회도 안 해서 "왜 안 보이는지"를 끝내 알 수 없다.
    {
        let quota = codex_quota.clone();
        let running = codex_quota_ping_running.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let mut ticker = tokio::time::interval(Duration::from_secs(600));
            loop {
                ticker.tick().await;
                tracing::info!("codex quota 주기 동기화");
                run_codex_quota_ping(quota.clone(), running.clone()).await;
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
            sync_codex_quota,
            sync_antigravity_quota,
            ble_status,
            ble_set_enabled,
            network_status,
            network_set_enabled,
            lan_status,
            lan_set_enabled,
            // 페어링은 두 전송이 공유한다(2026-08-25 스펙) — 전송별 커맨드가
            // 아니라 앱 레벨 커맨드 하나씩이다.
            pairing_status,
            begin_pairing,
            unpair,
            unpair_all,
            get_settings,
            set_enabled_agents,
            set_antigravity_poll_interval,
        ])
        .setup({
            let aggregator = aggregator.clone();
            let gate = gate.clone();
            let quota_state = quota_state.clone();
            let claude_quota_ping_running = claude_quota_ping_running.clone();
            let codex_quota = codex_quota.clone();
            let codex_quota_ping_running = codex_quota_ping_running.clone();
            let antigravity_quota = antigravity_quota.clone();
            let antigravity_poll_interval = antigravity_poll_interval.clone();
            let settings_state = settings_state.clone();
            let shared_pairing = shared_pairing.clone();
            move |app| {
                use tauri::Manager;
                app.manage(quota_state.clone());
                app.manage(claude_quota_ping_running.clone());
                app.manage(codex_quota.clone());
                app.manage(CodexPingGuard(codex_quota_ping_running.clone()));
                app.manage(settings_state.clone());
                app.manage(antigravity_quota.clone());
                app.manage(antigravity_poll_interval.clone());
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
                                    // accept 시점의 검사만으로는 충분하지 않다. 공유를
                                    // 끈 뒤에도 이미 열린 QUIC 연결은 control stream을
                                    // 새로 열 수 있으므로, 인증을 처리하기 직전에 다시
                                    // 확인하고 연결 자체를 폐기한다.
                                    if !h.bridge.lock().await.is_enabled() {
                                        conn.close(0u32.into(), b"sharing disabled");
                                        break;
                                    }
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
                                        && h.bridge.lock().await.is_enabled()
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
                                ble::peripheral::PeripheralEvent::Subscribed(_) => {
                                    h.bridge.lock().await.reset_gate();
                                }
                                ble::peripheral::PeripheralEvent::AuthWrite { central, data } => {
                                    let now = std::time::SystemTime::now();
                                    let granted = {
                                        let mut p = pairing_for_ble.lock().await;
                                        h.bridge.lock().await.handle_auth(central, data, now, &mut p)
                                    };
                                    if granted {
                                        h.bridge.lock().await.reset_gate();
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

                // 앱 시작 시 이전 저장된 공유(BLE, Network, LAN) 활성화 상태 복원
                {
                    let ble_h = ble_handle.clone();
                    let net_h = network_handle.clone();
                    let lan_h = lan_handle.clone();
                    let s_state = settings_state.clone();
                    tauri::async_runtime::spawn(async move {
                        let s = s_state.lock().await.clone();
                        if s.ble_enabled {
                            let mut b = ble_h.bridge.lock().await;
                            let _ = b.set_enabled(true);
                        }
                        if s.network_enabled {
                            let mut b = net_h.bridge.lock().await;
                            b.set_enabled(true);
                        }
                        if s.lan_enabled {
                            let mut b = lan_h.bridge.lock().await;
                            b.set_enabled(true);
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
                        let now = std::time::SystemTime::now();
                        // 실제 quota 주입(있을 때만): Claude=프록시 헤더, Codex=rollout rate_limits, Antigravity=gen_metadata 버킷.
                        // 캐시된 reset_at 이 이미 지났으면 만료된 창의 %이므로 0%로 보여주고
                        // reset_at 자체는 안 덮어쓴다(quota_pct_for_tick 문서 참고).
                        if let Some(c) =
                            snap.agents.iter_mut().find(|a| a.kind == AgentKind::Claude)
                        {
                            let (pct, reset) = quota_pct_for_tick(
                                *quota_for_tick.used_pct.lock().unwrap(),
                                *quota_for_tick.reset_at.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct { c.quota_used_pct = Some(p); }
                            if let Some(r) = reset { c.quota_reset_at = Some(r); }
                            let (pct_wk, reset_wk) = quota_pct_for_tick(
                                *quota_for_tick.used_pct_weekly.lock().unwrap(),
                                *quota_for_tick.reset_weekly.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct_wk { c.quota_used_pct_weekly = Some(p); }
                            if let Some(r) = reset_wk { c.quota_reset_at_weekly = Some(r); }
                            c.quota_error = quota_for_tick.last_error.lock().unwrap().clone();
                        }
                        if let Some(c) = snap.agents.iter_mut().find(|a| a.kind == AgentKind::Codex)
                        {
                            let (pct, reset) = quota_pct_for_tick(
                                *codex_for_tick.used_pct_5h.lock().unwrap(),
                                *codex_for_tick.reset_5h.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct { c.quota_used_pct = Some(p); }
                            if let Some(r) = reset { c.quota_reset_at = Some(r); }
                            let (pct_wk, reset_wk) = quota_pct_for_tick(
                                *codex_for_tick.used_pct_weekly.lock().unwrap(),
                                *codex_for_tick.reset_weekly.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct_wk { c.quota_used_pct_weekly = Some(p); }
                            if let Some(r) = reset_wk { c.quota_reset_at_weekly = Some(r); }
                            c.quota_error = codex_for_tick.last_error.lock().unwrap().clone();
                        }
                        if let Some(c) = snap.agents.iter_mut().find(|a| a.kind == AgentKind::Antigravity)
                        {
                            let (pct, reset) = quota_pct_for_tick(
                                *antigravity_for_tick.used_pct_5h.lock().unwrap(),
                                *antigravity_for_tick.reset_5h.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct { c.quota_used_pct = Some(p); }
                            if let Some(r) = reset { c.quota_reset_at = Some(r); }
                            let (pct_wk, reset_wk) = quota_pct_for_tick(
                                *antigravity_for_tick.used_pct_weekly.lock().unwrap(),
                                *antigravity_for_tick.reset_weekly.lock().unwrap(),
                                now,
                            );
                            if let Some(p) = pct_wk { c.quota_used_pct_weekly = Some(p); }
                            if let Some(r) = reset_wk { c.quota_reset_at_weekly = Some(r); }
                            c.quota_error = antigravity_for_tick.last_error.lock().unwrap().clone();
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 리스너가 없으면 주소를 보여주지 않는다. 주소를 내밀면 사용자는 그것을
    /// 기기에 넣고, 기기 쪽에는 왜 안 붙는지 알려 줄 화면이 없다.
    ///
    /// 토글을 껐을 때가 그 경우의 하나지만 **유일한 경우가 아니다** — `BindFailed`
    /// 는 토글을 켠 채로 리스너만 없앤다. 그래서 이 함수는 `enabled` 가 아니라
    /// `listening` 을 받고, 호출부(`lan_status`)가 `is_listening()` 을 넘긴다.
    /// 그 두 값이 실제로 갈라진다는 사실은 `lan::tests` 의
    /// `bind_failure_takes_the_listener_down_but_leaves_the_toggle_on` 이 잡는다.
    /// 여기서 그것을 한 번 더 검사해 봐야 인자가 `false` 로 같아 같은 줄만 다시
    /// 짚을 뿐이라 넣지 않았다.
    #[test]
    fn lan_address_is_hidden_while_sharing_is_off() {
        assert_eq!(lan_address(false, Some("192.168.0.12".into()), 4320), None);
    }

    /// 켜져 있으면 **주소와 포트를 한 문자열로** 준다. 프론트는 이 값을 그대로
    /// 찍기만 하면 되고 포트를 스스로 알 필요가 없다.
    ///
    /// **일부러 4320 이 아닌 포트로 검사한다.** 그래야 함수 안에 리터럴을 박는
    /// 변경을 잡는다 — 운영 포트로 검사하면 `format!("{ip}:4320")` 이라고 고쳐 놔도
    /// 통과해 버리고, 그것이 바로 이 함수가 막으려는 형태다(포트 상수는
    /// `lan::server::PORT` 한 곳에만 있어야 한다).
    #[test]
    fn lan_address_carries_the_port_it_is_given() {
        assert_eq!(
            lan_address(true, Some("10.0.0.5".into()), 9999),
            Some("10.0.0.5:9999".to_string())
        );
    }

    /// 호출부가 넘기는 포트는 언제나 리스너가 실제로 여는 그 포트다. 둘이 갈라지면
    /// 패널이 열려 있지 않은 포트를 안내한다.
    #[test]
    fn lan_address_uses_the_listener_port_in_production() {
        assert_eq!(
            lan_address(true, Some("10.0.0.5".into()), lan::server::PORT),
            Some(format!("10.0.0.5:{}", lan::server::PORT))
        );
    }

    /// 라우팅 가능한 IPv4 가 없으면(랜선이 빠졌다, 기본 경로가 없다) 주소도 없다.
    /// 여기서 "알 수 없음" 같은 문자열을 만들어 내면 패널이 그것을 주소처럼
    /// 보여준다 — 없다는 사실은 패널이 자기 문구로 말한다.
    #[test]
    fn lan_address_is_none_without_a_routable_ip() {
        assert_eq!(lan_address(true, None, lan::server::PORT), None);
    }

    /// 리셋 시각이 아직 안 지났으면 캐시된 값을 그대로 믿는다 — 지금 이
    /// 창의 진짜 최신 관측이니까.
    #[test]
    fn quota_pct_keeps_cached_value_before_reset() {
        let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let reset_at = now + Duration::from_secs(60);
        assert_eq!(
            quota_pct_for_tick(Some(87.0), Some(reset_at), now),
            (Some(87.0), Some(reset_at))
        );
    }

    /// 리셋 시각이 이미 지났으면 캐시된 %는 만료된 창의 값이다 — 새 창은
    /// 아직 관측된 게 없으니 0%로 보여준다(2026-09-01 실사용 중 100%가
    /// 리셋 후에도 계속 남아있던 것을 확인해 추가).
    #[test]
    fn quota_pct_resets_to_zero_after_window_passes() {
        let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let reset_at = now - Duration::from_secs(1);
        assert_eq!(quota_pct_for_tick(Some(100.0), Some(reset_at), now), (Some(0.0), None));
    }

    /// reset_at 이 아예 없으면(아직 한 번도 관측 못 함) 손대지 않는다 —
    /// 판단할 기준 자체가 없다.
    #[test]
    fn quota_pct_leaves_unknown_reset_untouched() {
        let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(quota_pct_for_tick(Some(42.0), None, now), (Some(42.0), None));
    }

    /// reset_at 이 지났을 때 **되돌려주는 reset_at 은 None** 이어야 한다 — 그래야
    /// 호출부가 만료된 옛 reset_at 으로 덮어쓰지 않고 아그리게이터의 실시간
    /// anchor 추정을 그대로 둔다.
    #[test]
    fn quota_pct_does_not_resurrect_the_expired_reset_at() {
        let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let reset_at = now - Duration::from_secs(1);
        let (_, reset) = quota_pct_for_tick(Some(100.0), Some(reset_at), now);
        assert_eq!(reset, None);
    }
}
