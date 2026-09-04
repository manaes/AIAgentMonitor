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


use crate::ble::pairing::{self, PairingManager};
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
    /// `CODE:`/`CODE2:` 로 새 토큰이 발급됐는가. 그 경우 호출부가 갱신된 목록을
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
        // 판정은 `AuthReply` 를 소유한 pairing 모듈이 한다. 예전에는 여기서
        // `matches!` 로 직접 갈랐는데, 같은 판정이 세 전송에 복사돼 있는 것이
        // 바로 v2 갈래를 빠뜨리는 사고의 형태였다(`ReplySignals` 의 doc).
        let s = reply.signals();
        AuthOutcome { payload, now_authorized: s.authorized, granted: s.granted }
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

    /// 스냅샷 틱의 **앞쪽 절반** — 이번 틱에 각 central 에게 나갈 줄을 만든다.
    /// 인가된 central 이 하나도 없거나 꺼져 있으면 빈 목록(BLE 의 "구독자 없으면
    /// 직렬화도 안 함" 과 같은 절약).
    ///
    /// 쓰기와 나눠 둔 이유는 **페어링 잠금 시간**이다. 봉인에는 `&mut
    /// PairingManager` 가 필요하지만(카운터가 전진한다) 실제 쓰기에는 필요 없고,
    /// `SendStream::write_all` 은 QUIC 흐름 제어에 막혀 수십 초씩 걸릴 수 있다 —
    /// 폰이 백그라운드로 가서 읽기를 멈추면 idle timeout 까지 그렇다. 그동안
    /// 잠금을 쥐고 있으면 페어링 시작·인증 처리가 통째로 멈춘다. 그래서 호출부는
    /// 이 함수까지만 잠금 안에서 부르고, 잠금을 놓은 뒤 `send_prepared` 를 부른다.
    pub fn prepare_snapshot(
        &mut self,
        snap: &Snapshot,
        now: SystemTime,
        pairing: &mut PairingManager,
    ) -> Vec<(CentralId, Vec<u8>)> {
        if !self.enabled || self.snapshot_senders.is_empty() {
            return Vec::new();
        }
        if !self.gate.should_emit(snap, now) {
            return Vec::new();
        }

        let dto = MirrorSnapshot::from(snap);
        let json = match serde_json::to_vec(&dto) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("네트워크 스냅샷 직렬화 실패: {e}");
                self.gate.reset();
                return Vec::new();
            }
        };

        // 봉인은 central 마다 다른 키와 카운터를 쓰므로 줄도 central 마다 따로
        // 만든다. 대상 목록을 먼저 뜨는 것은 `pairing` 을 `&mut` 로 빌리는 동안
        // `snapshot_senders` 를 함께 빌릴 수 없기 때문이다.
        let targets: Vec<CentralId> = self.snapshot_senders.keys().cloned().collect();
        let mut out = Vec::with_capacity(targets.len());
        for central in targets {
            match snapshot_line(pairing, &central, &json) {
                Some(mut line) => {
                    line.push(b'\n');
                    out.push((central, line));
                }
                // 인가를 잃은 central 은 스트림에서도 즉시 뺀다 — 다음 틱까지
                // 기다릴 이유가 없고, 남겨두면 매 틱 같은 판단을 반복한다.
                None => {
                    self.snapshot_senders.remove(&central);
                }
            }
        }
        out
    }

    /// 스냅샷 틱의 **뒤쪽 절반** — 준비된 줄을 각 스트림에 쓴다. 페어링 잠금
    /// 없이 돈다(`prepare_snapshot` 의 doc).
    ///
    /// 줄을 만든 뒤 여기 오기까지 잠금이 풀려 있으므로 그 사이에 언페어링이
    /// 끼어들 수 있다. `get_mut` 이 없는 대상을 건너뛰므로 그 경우 프레임은
    /// 버려진다 — 다만 이것이 **보장**이 되려면 언페어링이 그 사이에 실제로
    /// `drop_sessions` 까지 마쳐야 한다. 그래서 `lib.rs::persist_and_drop` 이
    /// `drop_sessions` 를 `save_paired_peers` **앞에서** 부른다. 순서가 반대면
    /// 해제는 디스크 쓰기를 기다리는 동안 이 쓰기보다 늦게 도착하기 쉽고, 그때는
    /// 이 조회가 아무것도 막지 못한다. 여기서 막히는 것은 봉인된 프레임 한
    /// 장뿐이고 평문 유출은 `snapshot_line` 의 인가 검사가 별도로 막는다.
    pub async fn send_prepared(&mut self, lines: Vec<(CentralId, Vec<u8>)>) {
        let mut dead = Vec::new();
        for (central, line) in lines {
            let Some(sender) = self.snapshot_senders.get_mut(&central) else {
                continue;
            };
            if sender.write_all(&line).await.is_err() {
                dead.push(central);
            }
        }
        for central in dead {
            self.snapshot_senders.remove(&central);
        }
    }
}

/// 이 central 에게 실제로 나갈 한 줄(줄바꿈은 붙이지 않는다). 인가되지 않았으면
/// `None` — 한 바이트도 나가면 안 된다.
///
/// **인가 검사가 여기 있어야 하는 이유.** `snapshot_senders` 에 들어 있다는 것은
/// "등록될 당시 인가돼 있었다"는 뜻일 뿐 지금도 그렇다는 보장이 아니다. 언페어링
/// 경로(`unpair` → `persist_and_drop`)는 페어링 잠금을 놓은 채 저장을 기다린 뒤에야
/// `drop_sessions` 를 부르므로, 그 사이에 낀 틱은 이미 인가가 사라진 central 을
/// 여전히 스트림 목록에서 본다. 게다가 그 시점엔 채널도 함께 지워져 있어
/// `channel_mut` 이 `None` 이다 — 아래 v1 분기로 떨어져 **평문 JSON** 이 나간다.
/// 방금 해제한 기기에게, 그것도 평문으로. BLE 는 `authorized_subscribers` 필터가
/// 루프 안에 있어 이 창이 없다. 두 전송이 같은 규칙을 따르게 한다.
///
/// v2 세션이면 봉인하고, v1 세션이면 평문 JSON 그대로 보낸다 — 채널이 없다는
/// 것이 곧 v1 이라는 뜻이고, 전환 기간 동안 두 세대가 공존한다(스펙 8장).
/// **이 등식은 인가가 살아 있을 때만 참이다** — 그래서 인가 검사가 먼저다.
///
/// **v2 는 봉인 프레임을 hex 로 싣는다.** 이 스트림은 줄바꿈으로 프레임 경계를
/// 나누는데(모듈 머리말의 NDJSON) 봉인 프레임은 임의의 이진 바이트라 0x0A 를
/// 그대로 담을 수 있다 — 날 것으로 흘려보내면 프레임 하나가 여러 줄로 쪼개져
/// 수신 측 경계 인식이 무너진다. 스냅샷 한 건 크기라면 사실상 매번 일어난다.
/// hex 는 이 프로토콜이 이미 `Granted2.sealed`·`epk`·`nonce` 에 쓰는 표기이고,
/// `{` 로 시작하지 않으므로 v1 평문 줄과도 한눈에 구분된다.
fn snapshot_line(
    pairing: &mut PairingManager,
    central: &CentralId,
    json: &[u8],
) -> Option<Vec<u8>> {
    if !pairing.is_authorized(central) {
        return None;
    }
    Some(match pairing.channel_mut(central) {
        Some(ch) => pairing::hex_encode_bytes(&ch.seal(json)).into_bytes(),
        None => json.to_vec(),
    })
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
    use crate::ble::pairing::test_client::{self, hex_encode, V2Client};
    use crate::crypto::{self, channel::SealedChannel};
    use iroh::endpoint::{Builder, RelayMode};
    use iroh::{Endpoint, EndpointAddr, SecretKey};
    use std::sync::Arc;
    use std::time::UNIX_EPOCH;
    use tokio::sync::Mutex;

    fn field(bytes: &[u8], key: &str) -> String {
        let v: serde_json::Value = serde_json::from_slice(bytes).expect("응답은 JSON 이다");
        v[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} 필드가 없다: {v}"))
            .to_string()
    }

    /// v2 로 페어링한다. 브릿지의 `handle_auth` 를 그대로 타므로 `AuthOutcome`
    /// 배선까지 함께 지나간다 — I/O 는 없다(제어 스트림 읽고 쓰기는 호출부
    /// 책임이므로 이 메서드는 순수한 동기 상태 기계 호출이다).
    fn v2_pair(
        b: &mut NetworkBridge,
        p: &mut PairingManager,
        central: &CentralId,
        now: SystemTime,
    ) -> (AuthOutcome, SealedChannel) {
        let code = p.begin_pairing(now);
        let mut c = V2Client::new();

        let out = b.handle_auth(
            central,
            format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(),
            now,
            p,
        );
        let (epk, nonce) = (field(&out.payload, "epk"), field(&out.payload, "nonce"));
        let (ss, tr) = c.agree(&epk);

        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let out = b.handle_auth(central, format!("CODE2:{cbind}").as_bytes(), now, p);
        let sealed = field(&out.payload, "sealed");
        let (_token, ch) = test_client::open_pairing_and_session(&ss, &nonce, &sealed);
        (out, ch)
    }

    /// v2 페어링이 성공해도 `AuthOutcome` 이 그것을 알리지 않으면, accept 루프는
    /// 스냅샷 uni-stream 을 열지 않고 토큰도 디스크에 쓰지 않는다 — 암호는 전부
    /// 맞는데 아이폰 화면만 영원히 비어 있게 된다.
    #[test]
    fn v2_grant_is_reported_as_authorized_and_worth_persisting() {
        let mut b = NetworkBridge::new();
        let mut p = PairingManager::new();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let (out, _ch) = v2_pair(&mut b, &mut p, &CentralId("N".into()), now);

        assert!(out.now_authorized, "v2 페어링 성공은 인가로 이어져야 한다");
        assert!(out.granted, "v2 도 새 토큰을 발급한다 — 저장하지 않으면 재부팅 후 사라진다");
        assert_eq!(p.issued_peers().len(), 1);
    }

    /// 재연결(`AUTH2`/`PROOF2`)도 인가다. 여기가 빠지면 저장된 토큰으로 자동
    /// 재연결한 아이폰이 스냅샷 스트림을 못 받는다.
    #[test]
    fn v2_reconnect_is_reported_as_authorized_but_not_persisted() {
        let mut b = NetworkBridge::new();
        let mut p = PairingManager::new();
        let token = "aa".repeat(16);
        p.load_peers(vec![(token.clone(), 900)]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let central = CentralId("N".into());
        let mut c = V2Client::new();

        let out = b.handle_auth(
            &central,
            format!("AUTH2:{}", hex_encode(&c.public)).as_bytes(),
            now,
            &mut p,
        );
        let (epk, nonce) = (field(&out.payload, "epk"), field(&out.payload, "nonce"));
        let (_ss, tr) = c.agree(&epk);
        let proof = hex_encode(&crypto::session_proof(
            &test_client::hex_decode(&token),
            &test_client::hex_decode(&nonce),
            &tr,
        ));

        let out = b.handle_auth(&central, format!("PROOF2:{proof}").as_bytes(), now, &mut p);
        assert!(out.now_authorized, "재연결 성공은 인가로 이어져야 한다");
        assert!(
            !out.granted,
            "재연결은 새 토큰을 발급하지 않는다 — 저장할 것이 없다"
        );
        assert!(p.is_authorized(&central));
    }

    /// BLE 와 같은 성질 — v2 세션에는 평문 JSON 이 나가면 안 된다.
    /// `prepare_snapshot` 은 실제 `SendStream` 을 요구하지만 줄을 만드는 판단
    /// 자체는 `snapshot_line` 에 있으므로, I/O 없이 그 판단만 곧바로 확인한다.
    #[test]
    fn v2_session_gets_a_sealed_line_the_client_can_open() {
        let mut b = NetworkBridge::new();
        let mut p = PairingManager::new();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let central = CentralId("N".into());
        let (_out, mut client) = v2_pair(&mut b, &mut p, &central, now);

        let json = br#"{"v":1,"agents":[]}"#;
        let line = snapshot_line(&mut p, &central, json).expect("인가된 세션이다");

        assert!(!line.starts_with(b"{"), "평문 JSON 이 나갔다");
        assert!(
            !line.contains(&b'\n'),
            "줄바꿈이 프레임 경계인 스트림에 0x0A 가 섞이면 수신 측이 프레임을 쪼갠다"
        );
        let frame = test_client::hex_decode(&String::from_utf8(line).expect("hex 는 ASCII 다"));
        assert_eq!(
            client.open(&frame).expect("세션 키로 열려야 한다"),
            json,
            "열고 나면 원래 JSON 이어야 한다"
        );
    }

    /// v1 세션은 전환 기간 동안 평문 그대로다(스펙 8장).
    #[test]
    fn v1_session_still_gets_a_plaintext_json_line() {
        let mut p = PairingManager::new();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let central = CentralId("N".into());
        let code = p.begin_pairing(now);
        p.handle(&central, pairing::AuthRequest::Code(code), now);
        assert!(p.is_authorized(&central));

        let json = br#"{"v":1,"agents":[]}"#;
        assert_eq!(snapshot_line(&mut p, &central, json), Some(json.to_vec()));
    }

    /// **인가되지 않은 central 은 0바이트다.** 이 성질을 지키는 코드가 여태
    /// `lib.rs` 의 accept 루프(uni-stream 을 아예 열지 않는 것)에만 있어서
    /// 테스트로 닿을 수 없었다. 이제 `snapshot_line` 이 직접 막으므로 검증된다.
    #[test]
    fn an_unauthorized_central_gets_no_line_at_all() {
        let mut p = PairingManager::new();
        assert_eq!(
            snapshot_line(&mut p, &CentralId("N".into()), br#"{"v":1}"#),
            None,
            "인가되지 않은 기기에는 한 바이트도 나가면 안 된다"
        );
    }

    /// I-1 회귀 테스트. `unpair` 는 페어링 잠금을 놓은 채 `save_paired_peers` 를
    /// 기다린 **뒤에야** `drop_sessions` 를 부른다. 그 사이(250ms 틱이 충분히
    /// 들어간다)에 이 central 은 여전히 `snapshot_senders` 에 있고, 인가와 함께
    /// v2 채널도 이미 지워져 `channel_mut` 이 `None` 이다 — 인가 검사가 없으면
    /// v1 분기로 떨어져 방금 해제한 기기에게 **평문 스냅샷 한 장**이 나간다.
    #[test]
    fn a_just_unpaired_v2_central_gets_nothing_not_plaintext() {
        let mut b = NetworkBridge::new();
        let mut p = PairingManager::new();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let central = CentralId("N".into());
        v2_pair(&mut b, &mut p, &central, now);

        let peer_id = p.paired_peers()[0].peer_id.clone();
        p.revoke_peer(&peer_id);
        // 여기가 `save_paired_peers` 를 기다리는 그 순간이다 — `drop_sessions` 는
        // 아직 불리지 않았다.

        let json = br#"{"v":1,"agents":[]}"#;
        let line = snapshot_line(&mut p, &central, json);
        assert_eq!(line, None, "해제 직후의 틱은 아무것도 내보내면 안 된다");
        assert_ne!(
            line,
            Some(json.to_vec()),
            "특히 평문이어서는 안 된다 — 채널이 함께 지워져 v1 로 오인되는 자리다"
        );
    }

    /// 카운터는 줄마다 전진해야 한다 — 같은 (키, 논스) 로 두 번 봉인하면
    /// ChaCha20-Poly1305 의 보장이 무너진다.
    #[test]
    fn each_v2_line_advances_the_counter() {
        let mut b = NetworkBridge::new();
        let mut p = PairingManager::new();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let central = CentralId("N".into());
        let (_out, mut client) = v2_pair(&mut b, &mut p, &central, now);

        let json = br#"{"v":1,"agents":[]}"#;
        let first = snapshot_line(&mut p, &central, json).expect("인가된 세션이다");
        let second = snapshot_line(&mut p, &central, json).expect("인가된 세션이다");
        assert_ne!(first, second, "같은 내용이라도 카운터가 달라 바이트가 달라야 한다");

        let de = |l: Vec<u8>| test_client::hex_decode(&String::from_utf8(l).unwrap());
        assert!(client.open(&de(first)).is_ok());
        assert!(client.open(&de(second)).is_ok(), "재전송으로 오인되면 안 된다");
    }

    fn snap(rate: f32, at: u64) -> Snapshot {
        use crate::types::{AgentKind, AgentState, TokenCounts};
        Snapshot {
            emitted_at: UNIX_EPOCH + Duration::from_secs(at),
            agents: vec![AgentState {
                kind: AgentKind::Claude,
                rate_tok_per_sec: rate,
                tokens_5h: TokenCounts::default(),
                quota_limit: None,
                quota_reset_at: None,
                quota_used_pct: None,
                quota_reset_at_weekly: None,
                quota_used_pct_weekly: None,
                quota_error: None,
                projects: vec![],
            }],
        }
    }

    /// 위의 순수 테스트들은 `snapshot_line` 만 본다. 이 테스트는 `prepare_snapshot`
    /// 이 실제로 그 판단을 쓰는지, 그리고 인가를 잃은 central 을 스트림 목록에서
    /// 빼는지까지 확인한다 — `snapshot_senders` 에 넣으려면 진짜 `SendStream` 이
    /// 필요해서 로컬 엔드포인트 두 개를 쓴다(위 왕복 테스트와 같은 의도적 예외).
    #[tokio::test]
    async fn prepare_snapshot_drops_a_central_that_lost_authorization() {
        let server_ep = local_endpoint().await;
        let client_ep = local_endpoint().await;
        let server_addr = wait_for_direct_addr(&server_ep).await;

        let client_task = tokio::spawn(async move {
            let conn = client_ep.connect(server_addr, ALPN).await.expect("client connect");
            // 연결을 살려 둔다 — 여기서 떨어뜨리면 서버의 open_uni 가 실패한다.
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(conn);
        });

        let incoming = server_ep.accept().await.expect("incoming");
        let conn = incoming.await.expect("connection established");
        let central = CentralId(conn.remote_id().to_string());
        let snap_send = conn.open_uni().await.expect("open_uni");

        let mut b = NetworkBridge::new();
        b.set_enabled(true);
        b.register_snapshot_sender(central.clone(), snap_send);

        let mut p = PairingManager::new();
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        let code = p.begin_pairing(t0);
        p.handle(&central, pairing::AuthRequest::Code(code), t0);
        assert!(p.is_authorized(&central));

        let lines = b.prepare_snapshot(&snap(1.0, 1000), t0, &mut p);
        assert_eq!(lines.len(), 1, "인가된 동안에는 줄이 나온다");
        assert_eq!(lines[0].0, central);
        assert!(lines[0].1.ends_with(b"\n"), "줄바꿈이 프레임 경계다");

        // 언페어링. `drop_sessions` 는 아직 부르지 않는다 — 저장을 기다리는
        // 그 창을 그대로 흉내낸다(I-1).
        let peer_id = p.paired_peers()[0].peer_id.clone();
        p.revoke_peer(&peer_id);

        let lines = b.prepare_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2), &mut p);
        assert!(lines.is_empty(), "인가를 잃으면 한 줄도 나오지 않는다");
        assert!(
            !b.has_snapshot_sender(&central),
            "그 자리에서 스트림 목록에서도 빠져야 한다"
        );

        client_task.abort();
        server_ep.close().await;
    }

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
