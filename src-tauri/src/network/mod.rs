//! 네트워크(iroh P2P/QUIC) 미러 전송 계층. `ble::` 와 나란한 형제 모듈이다 —
//! `BlePeripheral` 트레이트는 구현하지 않는다(그 트레이트는 "하드웨어 없이
//! BleBridge 를 테스트하기 위한" 것이지 전송 교체용이 아니다, ble/peripheral.rs
//! 주석 참고). 페어링 인증 프로토콜(`ble::pairing::PairingManager`)과 스냅샷
//! DTO(`ble::wire::MirrorSnapshot`)는 전송 계층과 무관하게 짜여 있어 그대로
//! 재사용한다 — 이 모듈이 새로 만드는 것은 "같은 바이트를 QUIC 스트림으로
//! 나르는 법" 뿐이다.
//!
//! ## 프레이밍이 BLE 와 다른 이유
//! BLE 는 GATT notify 의 좁은 MTU 때문에 `framing.rs` 로 수동 청킹한다. QUIC
//! 스트림은 이미 순서 보장되는 임의 길이 바이트 스트림이라 청킹이 필요 없다:
//! - 제어 메시지(HELLO/CODE/AUTH/PROOF)는 **요청마다 새 bidirectional
//!   스트림**을 연다 — `finish()` 로 쓰기 쪽을 닫는 것 자체가 메시지 경계다.
//! - 스냅샷 푸시는 인가된 central 당 **장수명 unidirectional 스트림 하나**에
//!   줄바꿈으로 구분된 JSON(NDJSON)을 흘려보낸다 — `MirrorSnapshot` 의 compact
//!   JSON 은 raw `\n` 을 포함하지 않는다.
pub mod identity;


use crate::ble::pairing::{self, AuthReply, PairingManager};
use crate::ble::peripheral::CentralId;
use crate::ble::wire::MirrorSnapshot;
use crate::emitter::EmitGate;
use crate::types::Snapshot;
use iroh::endpoint::SendStream;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// BLE 의 `SERVICE_UUID` 에 대응하는 프로토콜 식별자. GATT 특성이 없으므로
/// ALPN(QUIC 연결 협상 시 주고받는 프로토콜 이름)이 그 역할을 한다.
pub const ALPN: &[u8] = b"aim/mirror/1";

/// BLE 는 GATT notify 대역 때문에 1000ms 로 묶여 있다(ble/mod.rs 참고). QUIC
/// 은 그 제약이 없지만, 화면 갱신 빈도 자체는 다를 이유가 없으므로 같은
/// 값을 쓴다 — 두 전송을 나란히 켜고 끌 때 체감 차이가 없어야 한다.
const NETWORK_THROTTLE: Duration = Duration::from_millis(1000);

/// `handle_auth` 한 번의 결과. 페이로드(제어 스트림에 그대로 쓸 바이트)와,
/// 이 응답으로 central 이 "지금부터 인가됨" 상태가 됐는지를 함께 돌려준다 —
/// 호출부(accept 루프)가 후자를 보고 스냅샷 전송용 uni-stream 을 열지 결정한다.
pub struct AuthOutcome {
    pub payload: Vec<u8>,
    pub now_authorized: bool,
    /// `CODE:` 로 새 토큰이 발급됐는가. 그 경우 호출부가 갱신된 목록을
    /// 디스크에 쓴다(저장소는 이제 앱이 소유한다 — 2026-08-25 스펙 5장).
    pub granted: bool,
}

pub struct NetworkBridge {
    gate: EmitGate,
    enabled: bool,
    /// 인가된 central 당 스냅샷을 흘려보내는 장수명 uni-stream. `Connection`
    /// 자체는 여기서 갖지 않는다 — 연결의 실제 수명은 accept 루프 태스크가
    /// 쥐고 있고, 이 브릿지는 "누구에게 무엇을 보낼지"만 안다(BLE 의
    /// `peripheral: Arc<dyn BlePeripheral>` 과 대칭되는 역할 분리).
    snapshot_senders: HashMap<CentralId, SendStream>,
}

impl NetworkBridge {
    pub fn new() -> Self {
        Self {
            gate: EmitGate::new(NETWORK_THROTTLE),
            enabled: false,
            snapshot_senders: HashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// BLE 의 `set_enabled` 와 같은 이유로 세션 인가를 정리한다 — 꺼졌다
    /// 켜졌을 때 예전 세션이 여전히 인가된 것으로 남지 않게.
    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if on {
            self.gate.reset();
        } else {
            self.snapshot_senders.clear();
        }
    }

    /// 이 전송이 지금 서비스 중인 central 목록. BLE 의 `served_centrals` 와 같은
    /// 목적 — 공유 `PairingManager` 에서 이 전송의 세션만 정리하기 위해, 호출자가
    /// `set_enabled(false)` **전에** 받아 간다(2026-08-25 스펙 4장).
    pub fn served_centrals(&self) -> Vec<CentralId> {
        self.snapshot_senders.keys().cloned().collect()
    }

    /// 연결이 끊긴 central 의 전송 자원을 정리한다. 세션 인가는 이제 앱이
    /// 공유 `PairingManager` 에서 지운다.
    pub fn forget_central(&mut self, central: &CentralId) {
        self.snapshot_senders.remove(central);
    }

    /// BLE 의 `drop_sessions` 와 같다 — 앱이 언페어링 후 두 브릿지 모두에
    /// 같은 목록을 넘기며, 모르는 id 는 무시된다.
    pub fn drop_sessions(&mut self, ids: &[CentralId]) {
        for id in ids {
            self.snapshot_senders.remove(id);
        }
    }

    /// 제어 스트림(bi-stream) 하나에서 읽은 바이트를 처리한다. I/O 는 호출부
    /// 책임이다 — 이 메서드는 `ble::mod::BleBridge::handle_auth` 와 마찬가지로
    /// 동기 상태 기계 호출일 뿐이다.
    pub fn handle_auth(
        &mut self,
        central: &CentralId,
        data: &[u8],
        now: SystemTime,
        pairing: &mut PairingManager,
    ) -> AuthOutcome {
        let req = pairing::parse_auth_request(data);
        let reply = pairing.handle(central, req, now);
        let payload = reply.to_json_bytes();
        let now_authorized = matches!(reply, AuthReply::Granted { .. } | AuthReply::Authorized);
        let granted = matches!(reply, AuthReply::Granted { .. });
        AuthOutcome { payload, now_authorized, granted }
    }

    /// `Granted`/`Authorized` 응답 직후, accept 루프가 새로 연 uni-stream 을
    /// 등록한다. 이미 등록돼 있으면(같은 central 이 AUTH 를 다시 보낸 경우)
    /// 새 스트림으로 교체한다.
    pub fn register_snapshot_sender(&mut self, central: CentralId, sender: SendStream) {
        self.snapshot_senders.insert(central, sender);
    }

    pub fn has_snapshot_sender(&self, central: &CentralId) -> bool {
        self.snapshot_senders.contains_key(central)
    }

    /// 스냅샷 틱마다 호출한다. 인가된 central 이 하나도 없거나 꺼져 있으면
    /// 즉시 반환 — BLE 의 "구독자 없으면 직렬화도 안 함" 과 같은 절약.
    pub async fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime) {
        if !self.enabled || self.snapshot_senders.is_empty() {
            return;
        }
        if !self.gate.should_emit(snap, now) {
            return;
        }

        let dto = MirrorSnapshot::from(snap);
        let mut line = match serde_json::to_vec(&dto) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("네트워크 스냅샷 직렬화 실패: {e}");
                self.gate.reset();
                return;
            }
        };
        line.push(b'\n');

        let mut dead = Vec::new();
        for (central, sender) in self.snapshot_senders.iter_mut() {
            if sender.write_all(&line).await.is_err() {
                dead.push(central.clone());
            }
        }
        for central in dead {
            self.snapshot_senders.remove(&central);
        }
    }
}

impl Default for NetworkBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// 이 Mac 의 iroh 엔드포인트를 만든다.
///
/// **`address_lookup(PkarrPublisher)` 가 반드시 있어야 한다.** 이게 빠져 있으면
/// Mac 은 자기 주소를 discovery 에 게시하지 않고, 폰이 Mac 을 찾을 방법은 QR 을
/// 스캔하던 순간에 박제된 주소 스냅샷(relay URL + direct 주소)뿐이 된다. Mac 앱이
/// 재시작하면 UDP 포트가 바뀌므로(릴레이 배정·LAN IP 도 바뀔 수 있다) 그 스냅샷이
/// 죽고, 폰은 죽은 주소로 3초마다 영원히 재시도하다 `IrohError` 로 실패한다 —
/// "어제는 됐는데 다음날 앱을 새로 켜니 연결이 안 된다" 의 정체가 이것이었다.
///
/// 게시가 성립하는 근거는 `identity::load_or_create` 가 비밀키를 영속화해
/// `EndpointId` 가 재시작 후에도 같다는 점이다 — 폰은 저장해둔 그 id 로 Mac 의
/// **현재** 주소를 조회한다. 폰 쪽은 이미 `applyN0()` 로 resolver 를 갖고 있고,
/// iroh 는 앱이 준 주소가 있어도(`handle_msg_resolve_remote`) 선택된 경로가 없으면
/// address lookup 을 돌리므로, 낡은 주소가 조회를 막지도 않는다.
///
/// `Builder::empty()` 를 쓰는 이유는 crypto provider 를 직접 지정하기 위해서다
/// (`N0` 프리셋 문서가 권하는 방식). 그 대신 프리셋이 넣어주는 address lookup
/// 서비스가 빠지므로 여기서 명시적으로 다시 넣는다.
pub fn build_endpoint_builder(secret: iroh::SecretKey) -> iroh::endpoint::Builder {
    use iroh::address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver};

    iroh::endpoint::Builder::empty()
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(iroh::endpoint::RelayMode::Default)
        .crypto_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        // 아래 셋은 `presets::N0` 이 넣어주는 것과 같다. 게시(Publisher)가 이
        // 버그의 핵심이고, 나머지 둘은 프리셋과 대칭을 맞추기 위해 함께 둔다.
        .address_lookup(PkarrPublisher::n0_dns())
        .address_lookup(PkarrResolver::n0_dns())
        .address_lookup(DnsAddressLookup::n0_dns())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::{Builder, RelayMode};
    use iroh::{Endpoint, EndpointAddr, SecretKey};
    use std::sync::Arc;
    use std::time::UNIX_EPOCH;
    use tokio::sync::Mutex;

    /// 회귀 방지: Mac 이 자기 주소를 discovery 에 **게시**하지 않으면, 폰은 QR 을
    /// 스캔하던 순간의 주소 스냅샷만 갖게 되고 Mac 이 재시작해 UDP 포트가 바뀌는
    /// 순간 영영 못 찾는다("다음날 앱을 새로 켜면 연결 안 됨" 버그).
    ///
    /// `Builder::empty()` 는 문서상 "no address lookup services" 라 그냥 쓰면 이
    /// 서비스 목록이 비어 있다 — 그게 정확히 버그였던 상태다. bind 하지 않고
    /// 빌더만 확인하므로 네트워크를 타지 않는다.
    #[tokio::test]
    async fn endpoint_publishes_its_address_to_discovery() {
        let ep = build_endpoint_builder(SecretKey::generate())
            .bind_addr("127.0.0.1:0")
            .expect("valid bind addr")
            .bind()
            .await
            .expect("bind endpoint");

        let services = ep.address_lookup().expect("endpoint is open");
        assert!(
            !services.is_empty(),
            "address lookup 서비스가 하나도 없으면 Mac 주소가 게시되지 않는다 — \
             폰은 재시작한 Mac 을 영영 찾지 못한다"
        );
        // N0 프리셋과 같은 3종(Publisher/Resolver/DnsAddressLookup).
        assert_eq!(services.len(), 3, "실제 서비스: {services:?}");

        ep.close().await;
    }

    async fn local_endpoint() -> Endpoint {
        Builder::empty()
            .secret_key(SecretKey::generate())
            .alpns(vec![ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .bind_addr("127.0.0.1:0")
            .expect("valid bind addr")
            .bind()
            .await
            .expect("bind local endpoint")
    }

    async fn wait_for_direct_addr(ep: &Endpoint) -> EndpointAddr {
        for _ in 0..100 {
            let addr = ep.addr();
            if !addr.addrs.is_empty() {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ep.addr()
    }

    /// 실제 로컬 iroh Endpoint 2개로 HELLO→CODE→Granted 왕복을 검증한다.
    /// `BlePeripheral` 처럼 하드웨어 없는 순수 로직 테스트로 못 뽑아내는
    /// 새 영역이라(iroh 연결 수립 자체), `FakePeripheral` 대신 진짜 로컬
    /// 엔드포인트를 쓰는 걸 의도적 예외로 둔다(계획 문서 참고).
    #[tokio::test]
    async fn hello_then_code_grants_a_token_over_real_local_endpoints() {
        let server_ep = local_endpoint().await;
        let client_ep = local_endpoint().await;
        let server_addr = wait_for_direct_addr(&server_ep).await;

        let bridge = Arc::new(Mutex::new(NetworkBridge::new()));
        bridge.lock().await.set_enabled(true);
        // 페어링은 앱이 소유하고 두 전송이 공유한다(2026-08-25 스펙 3장) —
        // 테스트도 그 모양 그대로 주입한다.
        let pairing = Arc::new(Mutex::new(PairingManager::new()));
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let code = pairing.lock().await.begin_pairing(now);

        let server_ep_for_task = server_ep.clone();
        let bridge_for_task = bridge.clone();
        let pairing_for_task = pairing.clone();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let server_task = tokio::spawn(async move {
            let incoming = server_ep_for_task.accept().await.expect("incoming");
            let conn = incoming.await.expect("connection established");
            let central = CentralId(conn.remote_id().to_string());

            // HELLO
            let (mut send, mut recv) = conn.accept_bi().await.expect("accept_bi #1");
            let req = recv.read_to_end(4096).await.expect("read HELLO");
            let outcome = {
                let mut p = pairing_for_task.lock().await;
                bridge_for_task.lock().await.handle_auth(&central, &req, now, &mut p)
            };
            send.write_all(&outcome.payload).await.expect("write HELLO reply");
            send.finish().expect("finish HELLO reply");

            // CODE:<code>
            let (mut send, mut recv) = conn.accept_bi().await.expect("accept_bi #2");
            let req = recv.read_to_end(4096).await.expect("read CODE");
            let outcome = {
                let mut p = pairing_for_task.lock().await;
                bridge_for_task.lock().await.handle_auth(&central, &req, now, &mut p)
            };
            send.write_all(&outcome.payload).await.expect("write CODE reply");
            send.finish().expect("finish CODE reply");
            assert!(outcome.now_authorized, "CODE 성공은 인가로 이어져야 한다");

            let _ = done_rx.await;
        });

        let conn = client_ep.connect(server_addr, ALPN).await.expect("client connect");

        let (mut send, mut recv) = conn.open_bi().await.expect("open_bi HELLO");
        send.write_all(b"HELLO").await.expect("write HELLO");
        send.finish().expect("finish HELLO");
        let reply = recv.read_to_end(4096).await.expect("read HELLO reply");
        assert_eq!(reply, br#"{"ok":false,"await":"code"}"#);

        let (mut send, mut recv) = conn.open_bi().await.expect("open_bi CODE");
        send.write_all(format!("CODE:{code}").as_bytes())
            .await
            .expect("write CODE");
        send.finish().expect("finish CODE");
        let reply = recv.read_to_end(4096).await.expect("read CODE reply");
        let reply_text = String::from_utf8(reply).unwrap();
        assert!(reply_text.contains(r#""ok":true"#), "토큰 발급 응답이어야 한다: {reply_text}");
        assert!(reply_text.contains(r#""token""#));

        let _ = done_tx.send(());
        server_task.await.expect("server task join");

        let stored = pairing.lock().await.issued_peers();
        assert_eq!(stored.len(), 1, "페어링 성공 후 저장할 피어가 하나 있어야 한다");
    }
}
