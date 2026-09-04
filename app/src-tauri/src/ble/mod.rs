//! BLE 미러 전송 계층. 조립 지점은 `BleBridge`.
pub mod framing;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod peripheral;
pub mod pairing;

pub mod send_queue;
pub mod wire;

use crate::emitter::EmitGate;
use crate::types::Snapshot;
use peripheral::{BlePeripheral, CentralId, CharId};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wire::MirrorSnapshot;

/// 스냅샷 송출 상한. 기존 EmitGate(500ms)보다 느슨하게 잡아 BLE 대역을 아낀다.
const BLE_THROTTLE: Duration = Duration::from_millis(1000);
/// Auth 응답 청크의 식별자. JSON은 `{`로 시작하므로 0xFF 프레임은 레거시 한-패킷
/// 응답과 충돌하지 않는다. Snapshot과 재조립 규칙은 같되, 채널은 분리한다.
const AUTH_FRAME_ID: u8 = 0xFF;
const DEFAULT_AUTH_NOTIFY_LEN: usize = 185;

pub struct BleBridge {
    peripheral: Arc<dyn BlePeripheral>,
    gate: EmitGate,
    enabled: bool,
    next_frame_id: u8,
}

impl BleBridge {
    pub fn new(peripheral: Arc<dyn BlePeripheral>) -> Self {
        Self {
            peripheral,
            gate: EmitGate::new(BLE_THROTTLE),
            enabled: false,
            next_frame_id: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 게이트 해시를 리셋하여 다음 틱에 무조건 스냅샷을 송출하도록 한다.
    pub fn reset_gate(&mut self) {
        self.gate.reset();
    }

    /// 실패를 호출자(Tauri 커맨드)에게 그대로 넘긴다. 이 크레이트에는 tracing subscriber 가
    /// 없어 로그가 아무 데도 남지 않으므로, 오류가 UI 까지 도달하는 유일한 경로다.
    pub fn set_enabled(&mut self, on: bool) -> anyhow::Result<()> {
        if on == self.enabled {
            return Ok(());
        }
        self.enabled = on;
        if on {
            // 게이트는 내용 해시만 보고 억제하므로(emitted_at 은 해시에 없다), 껐다 켠 뒤
            // 내용이 그대로면 첫 프레임이 영구히 억제된다. 재개 시엔 반드시 한 번 내보낸다.
            self.gate.reset();
            if let Err(e) = self.peripheral.start() {
                self.enabled = false;
                return Err(e);
            }
        } else {
            self.peripheral.stop();
        }
        Ok(())
    }

    /// 이 전송이 지금 서비스 중인 central 목록. 공유 `PairingManager` 에서
    /// **이 전송의 세션만** 정리할 때 쓴다 — `end_all_sessions` 를 쓰면 BLE 를
    /// 끄는 순간 네트워크 세션까지 죽는다(2026-08-25 스펙 4장).
    ///
    /// `set_enabled(false)`/`PoweredOff` 처럼 `did_unsubscribe` 를 거치지 않는
    /// 경로에서 호출자가 먼저 이 목록을 받아 `PairingManager::end_sessions` 에
    /// 넘긴다 — 그러지 않으면 인가가 실제 연결보다 오래 살아남는다
    /// (전체 브랜치 리뷰 I-2).
    pub fn served_centrals(&self) -> Vec<CentralId> {
        self.peripheral.subscribers().into_iter().map(|s| s.id).collect()
    }

    /// 이 이벤트가 구독자 사본을 비워버리는 종류라면, 비워지기 **전에** 정리
    /// 대상을 찍어 돌려준다.
    ///
    /// `PoweredOff` 하나만 해당한다 — macOS 구현의 `apply_event` 가 그 이벤트에서
    /// 사본을 통째로 비우기 때문이다. 이벤트 루프는 `apply_event` 를 `match` 보다
    /// 먼저 부르므로, `PoweredOff` 분기에서 `served_centrals()` 를 그때 읽으면
    /// 언제나 빈 목록이고 세션 정리가 조용히 아무 일도 하지 않는다. 판단을
    /// 여기 두는 이유는 그 순서 자체가 이벤트 루프 클로저 안에서는 테스트할 수
    /// 없기 때문이다.
    pub fn sessions_to_end_before(
        &self,
        ev: &peripheral::PeripheralEvent,
    ) -> Option<Vec<CentralId>> {
        matches!(ev, peripheral::PeripheralEvent::PoweredOff).then(|| self.served_centrals())
    }

    /// 스냅샷 틱마다 호출한다. 게이트·구독자·직렬화·봉인·청킹을 모두 여기서
    /// 판단한다.
    ///
    /// `pairing` 이 `&mut` 인 이유는 봉인이 채널의 송신 카운터를 전진시키기
    /// 때문이다 — (키, 논스) 쌍은 절대 재사용하지 않는다(crypto/channel.rs).
    pub fn on_snapshot(
        &mut self,
        snap: &Snapshot,
        now: SystemTime,
        pairing: &mut pairing::PairingManager,
    ) {
        if !self.enabled {
            return;
        }
        // 인가된 구독자만 대상으로 삼는다. 미인가 기기가 붙어 있어도
        // 스냅샷은 만들지 않는다(스펙 5.1).
        let authorized = self
            .peripheral
            .authorized_subscribers(&|id| pairing.is_authorized(id));
        if authorized.is_empty() {
            return;
        }
        if !self.gate.should_emit(snap, now) {
            return;
        }

        let dto = MirrorSnapshot::from(snap);
        let json = match serde_json::to_vec(&dto) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("스냅샷 직렬화 실패: {e}");
                // 게이트가 이미 "송출함"으로 커밋했으므로, 실패했다면 되돌려서
                // 다음 틱에 동일한 내용이라도 다시 시도되게 한다.
                self.gate.reset();
                return;
            }
        };
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);

        let mut sent_any = false;
        for sub in &authorized {
            // v2 세션이면 봉인하고, v1 세션이면 평문 그대로 보낸다 — 전환 기간
            // 동안 두 세대가 공존한다(스펙 8장). 채널이 없다는 것이 곧 v1 이라는
            // 뜻이다. 봉인은 청킹 **직전에** 한다: 청크 헤더는 재조립을 위해
            // 평문이어야 하고, 봉인 프레임 하나가 청크 여럿으로 쪼개져야 한다.
            //
            // 청킹이 실패해도 카운터는 이미 전진한 뒤다. 그래도 괜찮은 이유는
            // 수신 측이 카운터의 빈 칸을 견디기 때문이다
            // (`tolerates_a_gap_in_counters`) — 반대로 실패했다고 카운터를
            // 되돌리면 같은 (키, 논스) 로 두 번 봉인하게 되어 훨씬 위험하다.
            let payload = match pairing.channel_mut(&sub.id) {
                Some(ch) => ch.seal(&json),
                None => json.clone(),
            };
            // 청크 크기는 이 central 의 MTU 로 정한다. 단, ESP32 수신 버퍼 안정성을 위해 최대 240바이트로 제한한다.
            let max_chunk = usize::min(sub.max_notify_len, 240);
            match framing::chunk(frame_id, &payload, max_chunk) {
                Ok(chunks) => {
                    self.peripheral
                        .offer_frame_to(CharId::Snapshot, &sub.id, chunks);
                    sent_any = true;
                }
                Err(e) => {
                    tracing::error!("청킹 실패({}): {e:?}", sub.id.0);
                }
            }
        }
        if !sent_any {
            // 아무도 못 받았으면 게이트가 이미 "송출함"으로 커밋한 것을 되돌려
            // 다음 틱에 같은 내용이라도 다시 시도되게 한다. 한 명이라도 받았다면
            // 내용은 실제로 나갔으므로 되돌리지 않는다.
            self.gate.reset();
        }
    }

    /// Auth 특성 쓰기를 처리하고 응답을 보낸다. 페어링 상태 기계는 앱이
    /// 소유하므로(2026-08-25 스펙 3장) 여기서는 넘겨받아 쓴다 — 이 메서드에
    /// 남은 전송 고유의 일은 "응답 바이트를 Auth 특성으로 notify 하는 것"뿐이다.
    ///
    /// `CODE:` 로 새 토큰이 발급됐는지를 돌려준다. 그 경우 호출부가 갱신된
    /// 목록을 디스크에 쓴다.
    pub fn handle_auth(
        &mut self,
        central: &CentralId,
        data: &[u8],
        now: SystemTime,
        pairing: &mut pairing::PairingManager,
    ) -> bool {
        let req = pairing::parse_auth_request(data);
        let reply = pairing.handle(central, req, now);
        // v2 도 `Granted2` 로 새 토큰을 발급한다 — v1 만 보면 v2 로 페어링한
        // 기기의 토큰이 디스크에 남지 않아 맥을 껐다 켜는 순간 사라진다.
        // 그 판정은 이제 `AuthReply` 옆에 한 벌만 있다(`ReplySignals` 의 doc).
        let granted = reply.signals().granted;
        let payload = reply.to_json_bytes();
        // Auth 특성도 MTU보다 긴 notify 하나에 담을 수 없다. 기존 클라이언트와의
        // 호환을 위해 한 패킷에 들어갈 때는 JSON을 그대로 보내고, 초과할 때만
        // 명시적 프레임으로 쪼갠다.
        let max_notify_len = self
            .peripheral
            .subscribers()
            .into_iter()
            .find(|sub| sub.id == *central)
            .map(|sub| usize::min(sub.max_notify_len, 240))
            .unwrap_or(DEFAULT_AUTH_NOTIFY_LEN);
        let packets = if payload.len() <= max_notify_len {
            vec![payload]
        } else {
            match framing::chunk(AUTH_FRAME_ID, &payload, max_notify_len) {
                Ok(packets) => packets,
                Err(error) => {
                    tracing::error!(central = %central.0, ?error, "Auth 응답 청킹 실패");
                    return granted;
                }
            }
        };
        for packet in packets {
            self.peripheral.notify_auth(central, packet);
        }
        granted
    }

    /// 링크가 끊긴 central 의 전송 자원을 정리한다. 세션 인가 자체는 이제
    /// 앱이 공유 `PairingManager` 에서 지운다 — 여기서는 macOS pump 대상에서만
    /// 빼낸다.
    pub fn forget_central(&mut self, central: &CentralId) {
        self.peripheral.revoke_targets(std::slice::from_ref(central));
    }

    /// 앱이 언페어링/세션 종료 후 "이 central 들을 전송 대상에서 빼라"고 알린다.
    /// 모르는 id 는 무시된다 — 앱은 그 central 이 어느 전송에 붙어 있었는지
    /// 모르는 채로 두 브릿지 모두에 같은 목록을 넘긴다.
    pub fn drop_sessions(&mut self, ids: &[CentralId]) {
        self.peripheral.revoke_targets(ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{self, channel::SealedChannel};
    use crate::types::{AgentKind, AgentState, Snapshot, TokenCounts};
    use pairing::test_client::{self, hex_encode, V2Client};
    use peripheral::{CentralId, FakePeripheral, Subscriber};
    use std::time::{Duration, UNIX_EPOCH};

    fn snap(rate: f32, at: u64) -> Snapshot {
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

    /// 청킹이 255 청크를 넘어 반드시 실패하도록 에이전트를 여럿 담은 큰 스냅샷.
    fn big_snap(rate: f32, at: u64) -> Snapshot {
        Snapshot {
            emitted_at: UNIX_EPOCH + Duration::from_secs(at),
            agents: (0..10)
                .map(|_| AgentState {
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
                })
                .collect(),
        }
    }

    /// 페어링은 이제 앱이 소유하므로(2026-08-25 스펙 3장) 테스트도 함께 만들어
    /// 주입한다 — 브릿지가 자기 PairingManager 를 들고 있던 시절의 헬퍼를
    /// 그대로 옮긴 것이다.
    fn bridge() -> (BleBridge, Arc<FakePeripheral>, pairing::PairingManager) {
        let fake = Arc::new(FakePeripheral::new());
        (BleBridge::new(fake.clone()), fake, pairing::PairingManager::new())
    }

    /// 테스트 편의: 사용자가 페어링 창을 열고(begin_pairing), 그 central 이
    /// HELLO → 올바른 코드로 인가받는 과정을 흉내낸다. 인가 필터가 들어간
    /// 뒤로는 on_snapshot 이 이 central 에게 실제로 프레임을 보내려면
    /// 먼저 이 과정을 거쳐야 한다.
    fn authorize(
        b: &mut BleBridge,
        p: &mut pairing::PairingManager,
        central: &str,
        now: SystemTime,
    ) {
        let code = p.begin_pairing(now);
        b.handle_auth(&CentralId(central.to_string()), b"HELLO", now, p);
        b.handle_auth(&CentralId(central.to_string()), format!("CODE:{code}").as_bytes(), now, p);
    }

    /// Auth 특성으로 방금 나간 응답을 JSON 으로 꺼낸다. v2 테스트는
    /// `AuthReply` 를 직접 보지 못한다 — 브릿지는 이미 직렬화된 바이트만
    /// 내놓기 때문이고, 그게 실기기에서 클라이언트가 보는 것과 같다.
    fn last_auth_reply(fake: &FakePeripheral, central: &str) -> serde_json::Value {
        let replies = fake.taken_auth_replies();
        let packets: Vec<&Vec<u8>> = replies
            .iter()
            .filter(|(id, _)| id.0 == central)
            .map(|(_, bytes)| bytes)
            .collect();
        let first_chunk = packets
            .iter()
            .rposition(|packet| packet.first() == Some(&AUTH_FRAME_ID) && packet.get(1) == Some(&0))
            .unwrap_or(packets.len());
        let bytes = if first_chunk < packets.len() {
            let mut reassembler = framing::Reassembler::new();
            packets[first_chunk..]
                .iter()
                .find_map(|packet| reassembler.push(packet))
                .expect("Auth 청크는 완성돼야 한다")
        } else {
            (*packets.last().expect("그 central 에게 간 Auth 응답이 있어야 한다")).clone()
        };
        serde_json::from_slice(&bytes).expect("응답은 JSON 이다")
    }

    fn field(v: &serde_json::Value, key: &str) -> String {
        v[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} 필드가 없다: {v}"))
            .to_string()
    }

    /// `authorize` 의 v2 판. **브릿지의 `handle_auth` 를 그대로 탄다** — 그래야
    /// 페어링 상태 기계만이 아니라 전송 계층 배선(저장 신호·notify)까지 함께
    /// 검증된다. 돌려주는 것은 (호출부에 "저장하라"고 신호했는지, 클라이언트
    /// 쪽 세션 채널)이다.
    fn authorize_v2(
        b: &mut BleBridge,
        fake: &FakePeripheral,
        p: &mut pairing::PairingManager,
        central: &str,
        now: SystemTime,
    ) -> (bool, SealedChannel) {
        let id = CentralId(central.to_string());
        let code = p.begin_pairing(now);
        let mut c = V2Client::new();

        b.handle_auth(&id, format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(), now, p);
        let reply = last_auth_reply(fake, central);
        let (epk, nonce) = (field(&reply, "epk"), field(&reply, "nonce"));
        let (ss, tr) = c.agree(&epk);

        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let persist = b.handle_auth(&id, format!("CODE2:{cbind}").as_bytes(), now, p);
        let sealed = field(&last_auth_reply(fake, central), "sealed");
        let (_token, ch) = test_client::open_pairing_and_session(&ss, &nonce, &sealed);
        (persist, ch)
    }

    /// 공유를 끌 때 세션 인가가 내려가는 배선. 페어링이 앱으로 옮겨가면서
    /// (2026-08-25 스펙 3장) 이 조립도 `ble_set_enabled` 로 옮겨갔다 — 여기서는
    /// 그 커맨드가 하는 것과 **같은 순서**로 브릿지가 내놓는 조각을 검증한다:
    /// 끄기 **전에** `served_centrals()` 를 받아 `end_sessions` 에 넘긴다.
    #[test]
    fn served_centrals_feeds_session_cleanup_on_disable() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        assert!(p.paired_peers()[0].connected, "인가 직후에는 연결됨이어야 한다");

        // stop() 뒤에는 구독자 목록이 비어 알 수 없으므로 반드시 먼저 받는다.
        let served = b.served_centrals();
        assert_eq!(served, vec![CentralId("A".into())], "이 전송이 서비스 중이던 central");
        b.set_enabled(false).unwrap();
        p.end_sessions(&served);

        assert!(
            !p.paired_peers()[0].connected,
            "공유를 끄면 didUnsubscribe 없이도 즉시 연결됨 표시가 내려가야 한다(I-2)"
        );
        assert_eq!(
            p.paired_peers().len(),
            1,
            "세션 인가만 지워야 한다 — 저장된 페어링(토큰) 자체는 남아야 한다"
        );
    }

    /// 전원이 꺼질 때의 세션 정리는 **구독자 사본이 비워지기 전에** 목록을
    /// 찍어둬야만 성립한다. 이벤트 루프는 `apply_event` 를 `match` 보다 먼저
    /// 부르고 `apply_event(PoweredOff)` 가 사본을 비우므로, 분기 안에서
    /// `served_centrals()` 를 그때 읽으면 `end_sessions` 가 빈 목록을 받아
    /// 아무 일도 하지 않는다 — 그 상태에서는 블루투스를 껐다 켠 뒤 같은
    /// central 이 아무것도 다시 증명하지 않고 인가된 채로 돌아온다.
    #[test]
    fn power_off_captures_the_served_list_before_the_mirror_is_cleared() {
        let (mut b, fake, _p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);

        assert_eq!(
            b.sessions_to_end_before(&peripheral::PeripheralEvent::PoweredOff),
            Some(vec![CentralId("A".into())]),
            "전원 꺼짐에서는 사본이 비기 전의 목록이 나와야 한다"
        );

        // apply_event(PoweredOff) 가 하는 일 — 사본을 비운다.
        fake.set_subscribers(vec![]);
        assert!(
            b.served_centrals().is_empty(),
            "비워진 뒤에 읽으면 빈 목록이다 — 이게 이 방어가 무력했던 원인이다"
        );
    }

    /// 사본을 비우지 않는 이벤트까지 미리 찍으면, 정리 대상이 아닌 것을 정리
    /// 대상으로 넘기게 된다.
    #[test]
    fn other_events_do_not_capture_a_cleanup_list() {
        let (mut b, fake, _p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        for ev in [
            peripheral::PeripheralEvent::PoweredOn,
            peripheral::PeripheralEvent::AdvertisingStarted,
            peripheral::PeripheralEvent::Unsubscribed(CentralId("A".into())),
            peripheral::PeripheralEvent::Disconnected(CentralId("A".into())),
        ] {
            assert_eq!(b.sessions_to_end_before(&ev), None, "{ev:?} 는 사본을 비우지 않는다");
        }
    }

    /// 공유 매니저의 핵심 성질 — BLE 를 꺼도 네트워크 세션은 살아 있어야 한다.
    /// `end_all_sessions` 를 쓰던 예전 코드로 되돌리면 이 테스트가 잡는다
    /// (2026-08-25 스펙 4장).
    #[test]
    fn disabling_ble_leaves_another_transports_session_alone() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("BLE-A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "BLE-A", now);
        // 네트워크로 붙은 기기를 흉내낸다 — 같은 매니저를 쓰지만 BLE 구독자는 아니다.
        let code = p.begin_pairing(now);
        p.handle(&CentralId("NET-B".into()), pairing::AuthRequest::Code(code), now);
        assert!(p.is_authorized(&CentralId("NET-B".into())));

        let served = b.served_centrals();
        b.set_enabled(false).unwrap();
        p.end_sessions(&served);

        assert!(!p.is_authorized(&CentralId("BLE-A".into())), "BLE 세션은 내려간다");
        assert!(
            p.is_authorized(&CentralId("NET-B".into())),
            "네트워크 세션은 살아 있어야 한다 — 이게 공유 매니저에서 가장 깨지기 쉬운 지점이다"
        );
    }

    #[test]
    fn does_nothing_while_disabled() {
        let (mut b, fake, mut p) = bridge();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &mut p);
        assert!(fake.taken_frames().is_empty(), "꺼져 있으면 아무것도 보내지 않는다");
    }

    #[test]
    fn does_nothing_without_subscribers() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &mut p);
        assert!(fake.taken_frames().is_empty(), "구독자가 없으면 직렬화도 하지 않는다");
    }

    #[test]
    fn chunks_oversized_auth_reply_for_a_small_mtu() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("tiny".into()),
            max_notify_len: 20,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        p.begin_pairing(now);
        let client = V2Client::new();
        b.handle_auth(
            &CentralId("tiny".into()),
            format!("HELLO2:{}", hex_encode(&client.public)).as_bytes(),
            now,
            &mut p,
        );

        let replies = fake.taken_auth_replies();
        let packets: Vec<_> = replies
            .iter()
            .filter(|(id, _)| id.0 == "tiny")
            .map(|(_, packet)| packet)
            .collect();
        assert!(packets.len() > 1, "작은 MTU에서는 Auth 응답을 청킹해야 한다");
        assert!(packets.iter().all(|packet| packet.first() == Some(&AUTH_FRAME_ID)));
        let mut reassembler = framing::Reassembler::new();
        let reply = packets
            .iter()
            .find_map(|packet| reassembler.push(packet))
            .expect("Auth 청크를 재조립할 수 있어야 한다");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&reply).unwrap()["v"], 2);
    }

    #[test]
    fn emits_chunked_snapshot_frame() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, CharId::Snapshot);

        // 재조립하면 원래 JSON 이 나와야 한다
        let mut r = framing::Reassembler::new();
        let mut msg = None;
        for c in &frames[0].1 {
            if let Some(m) = r.push(c) {
                msg = Some(m);
            }
        }
        let json = String::from_utf8(msg.expect("프레임이 완성되어야 한다")).unwrap();
        assert!(json.starts_with("{\"v\":1"), "실제 JSON: {json}");
    }

    #[test]
    fn throttles_to_one_hz() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", t0);
        b.on_snapshot(&snap(1.0, 1000), t0, &mut p);
        assert_eq!(fake.taken_frames().len(), 1);

        // 내용이 바뀌어도 1초가 안 지나면 보내지 않는다
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_millis(400), &mut p);
        assert!(fake.taken_frames().is_empty());

        b.on_snapshot(&snap(3.0, 1000), t0 + Duration::from_millis(1100), &mut p);
        assert_eq!(fake.taken_frames().len(), 1);
    }

    #[test]
    fn frame_id_increments_per_frame() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", t0);
        b.on_snapshot(&snap(1.0, 1000), t0, &mut p);
        let a = fake.taken_frames()[0].1[0][0];
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2), &mut p);
        let c = fake.taken_frames()[0].1[0][0];
        assert_eq!(c, a.wrapping_add(1), "frame_id 는 프레임마다 증가한다");
    }

    #[test]
    fn disabling_stops_the_peripheral() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        assert!(fake.is_started());
        b.set_enabled(false).unwrap();
        assert!(!fake.is_started());
    }

    #[test]
    fn re_enabling_sends_a_frame_even_if_content_is_unchanged() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", t0);
        let content = snap(1.0, 1000);
        b.on_snapshot(&content, t0, &mut p);
        assert_eq!(fake.taken_frames().len(), 1);

        // 껐다 켜면 내용이 그대로여도 다시 보내야 한다 — iOS 는 재구독 직후 화면이 비어 있다.
        // 공유를 끄면 세션 인가도 함께 지워지므로(전체 브랜치 리뷰 I-2 —
        // end_all_sessions), 다시 켠 뒤에는 재인가가 먼저 필요하다. 실제로는
        // 저장된 토큰으로 AUTH→PROOF 가 자동으로 일어나지만, 여기서는 코드
        // 페어링으로 같은 효과(다시 인가됨)를 낸다.
        b.set_enabled(false).unwrap();
        b.set_enabled(true).unwrap();
        let t1 = t0 + Duration::from_millis(1100);
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        authorize(&mut b, &mut p, "A", t1);
        b.on_snapshot(&content, t1, &mut p);
        assert_eq!(
            fake.taken_frames().len(),
            1,
            "재개 후 첫 프레임이 unchanged 로 억제되면 미러가 빈 화면으로 남는다"
        );
    }

    #[test]
    fn failed_start_reports_error_and_rolls_back() {
        let (mut b, fake, mut p) = bridge();
        fake.set_start_error(Some("권한 거부".to_string()));
        let err = b.set_enabled(true).expect_err("start() 실패는 호출자에게 전달되어야 한다");
        assert!(err.to_string().contains("권한 거부"), "실제 오류: {err}");
        assert!(!b.is_enabled(), "실패했으면 enabled 를 되돌려야 한다");

        // 되돌아갔으므로 다시 켜기를 시도할 수 있어야 한다(같은 값이라 무시되면 안 된다).
        fake.set_start_error(None);
        b.set_enabled(true).unwrap();
        assert!(b.is_enabled());
    }

    #[test]
    fn gate_retries_after_failed_frame() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        // max_notify_len 4 → 본문 1바이트. big_snap 은 255바이트를 훌쩍 넘으므로
        // framing::chunk 이 반드시 TooLarge 로 실패한다.
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 4,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", t0);
        let content = big_snap(1.0, 1000);

        b.on_snapshot(&content, t0, &mut p);
        assert!(fake.taken_frames().is_empty(), "청킹 실패로 프레임이 나가지 않는다");

        // 청킹이 성공할 수 있는 크기로 구독자를 바꾼 뒤, 스로틀 시간이 지난 시점에
        // 동일한 내용으로 다시 호출한다. 게이트가 리셋되지 않았다면 해시가 그대로라
        // "unchanged"로 영구 억제되어 이번에도 프레임이 비어 있을 것이다.
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 100,
        }]);
        b.on_snapshot(&content, t0 + Duration::from_millis(1100), &mut p);
        assert!(
            !fake.taken_frames().is_empty(),
            "실패한 프레임이 게이트를 영구 억제해선 안 된다 — 다음 틱에 재시도되어야 한다"
        );
    }

    // ---- 인가 필터(3단계) ----

    #[test]
    fn unauthorized_subscriber_gets_nothing() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &mut p);
        assert!(fake.taken_frames().is_empty(),
                "페어링하지 않은 기기는 한 바이트도 받으면 안 된다");
    }

    #[test]
    fn authorized_subscriber_receives_snapshot() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);

        // 사용자가 창을 연다 → HELLO → 그 코드로 인가
        authorize(&mut b, &mut p, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);
        assert_eq!(fake.taken_frames().len(), 1, "인가 후에는 받는다");
    }

    #[test]
    fn mixed_subscribers_only_authorized_are_targeted() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("A".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("B".into()), max_notify_len: 23 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);
        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        // 청크 크기는 **인가된** 구독자만 보고 정해야 한다.
        // 미인가 B(23)를 섞으면 청크가 불필요하게 잘게 쪼개진다.
        assert!(frames[0].1[0].len() > 23, "미인가 구독자의 MTU 에 끌려가면 안 된다");
    }

    /// MTU 가 작은 기기가 붙어도 큰 기기의 청크가 작아지면 안 된다.
    /// 예전에는 인가된 구독자 전체의 최솟값을 썼다.
    #[test]
    fn each_central_gets_chunks_sized_for_its_own_mtu() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("BIG".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("SMALL".into()), max_notify_len: 23 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "BIG", now);
        authorize(&mut b, &mut p, "SMALL", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames_by_central();
        let big = frames.iter().find(|(c, _, _)| c.0 == "BIG").expect("BIG 에게 갔다");
        let small = frames.iter().find(|(c, _, _)| c.0 == "SMALL").expect("SMALL 에게 갔다");
        assert!(
            big.2[0].len() > small.2[0].len(),
            "MTU 가 큰 기기는 더 큰 청크를 받아야 한다"
        );
    }

    /// 한 기기의 청킹 실패가 다른 기기의 프레임까지 없애면 안 된다. 예전에는
    /// 청크 크기가 전체 최솟값이라 MTU 가 작은 기기 하나 때문에 `TooLarge` 가
    /// 나면 **모두의** 프레임이 사라졌다.
    #[test]
    fn chunking_failure_for_one_central_does_not_silence_the_others() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        // max_notify_len 4 → 본문 1바이트. big_snap 은 255청크를 훌쩍 넘으므로
        // TINY 에 대해서만 framing::chunk 이 TooLarge 로 실패한다.
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("BIG".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("TINY".into()), max_notify_len: 4 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "BIG", now);
        authorize(&mut b, &mut p, "TINY", now);

        b.on_snapshot(&big_snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames_by_central();
        assert_eq!(frames.len(), 1, "실패한 기기만 빠져야 한다");
        assert_eq!(frames[0].0, CentralId("BIG".into()));
    }

    // ---- 봉인(9단계) ----

    /// 이 central 에게 간 프레임을 재조립한다 — 실기기에서 클라이언트가 보는
    /// 바이트와 같다. 청크 헤더는 평문이어야 하므로(재조립이 먼저다) 여기서
    /// 벗겨낸 결과가 곧 봉인 프레임이거나 평문 JSON 이다.
    fn reassembled_for(fake: &FakePeripheral, central: &str) -> Vec<u8> {
        let frames = fake.taken_frames_by_central();
        let (_, _, chunks) = frames
            .iter()
            .find(|(id, _, _)| id.0 == central)
            .unwrap_or_else(|| panic!("{central} 에게 간 프레임이 없다"));
        let mut r = framing::Reassembler::new();
        let mut msg = None;
        for c in chunks {
            if let Some(m) = r.push(c) {
                msg = Some(m);
            }
        }
        msg.expect("프레임이 완성되어야 한다")
    }

    /// v2 세션에는 봉인된 바이트가 가야 한다. 평문 JSON 이 그대로 나가면
    /// 이 스펙 전체가 무의미하다.
    #[test]
    fn v2_session_receives_sealed_bytes_not_plaintext_json() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize_v2(&mut b, &fake, &mut p, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let payload = reassembled_for(&fake, "A");
        assert!(!payload.starts_with(b"{"), "평문 JSON 이 나갔다");
        assert!(payload.len() > 8 + 16, "카운터 8 + 태그 16 보다 길어야 한다");
    }

    /// 위 테스트는 "평문이 아니다"까지만 본다. 정말로 **그 세션의 키로** 봉인
    /// 됐는지는 클라이언트가 실제로 열어봐야 알 수 있다 — 엉뚱한 키로 봉인해도
    /// 위 테스트는 통과하기 때문이다.
    #[test]
    fn the_v2_client_can_open_what_it_receives() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let (_persist, mut client) = authorize_v2(&mut b, &fake, &mut p, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let opened = client
            .open(&reassembled_for(&fake, "A"))
            .expect("세션 키로 열려야 한다");
        assert!(
            String::from_utf8(opened).unwrap().starts_with("{\"v\":1"),
            "열고 나면 원래의 스냅샷 JSON 이어야 한다"
        );
    }

    /// 카운터는 프레임마다 전진해야 한다 — 같은 (키, 논스) 로 두 번 봉인하면
    /// ChaCha20-Poly1305 의 보장이 통째로 무너진다(crypto/channel.rs).
    #[test]
    fn each_v2_frame_advances_the_counter() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        let (_persist, mut client) = authorize_v2(&mut b, &fake, &mut p, "A", t0);

        b.on_snapshot(&snap(1.0, 1000), t0, &mut p);
        let first = reassembled_for(&fake, "A");
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2), &mut p);
        let second = reassembled_for(&fake, "A");

        assert_eq!(&first[..8], &0u64.to_be_bytes(), "첫 프레임의 카운터는 0 이다");
        assert_eq!(&second[..8], &1u64.to_be_bytes(), "두 번째는 1 이어야 한다");
        assert!(client.open(&first).is_ok());
        assert!(client.open(&second).is_ok(), "재전송으로 오인되면 안 된다");
    }

    /// v1 세션은 그대로 평문이어야 한다 — 전환 기간 동안 기존 아이폰이 계속
    /// 동작해야 한다(스펙 8장).
    #[test]
    fn v1_session_still_receives_plaintext_json() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let payload = reassembled_for(&fake, "A");
        assert!(payload.starts_with(b"{"), "v1 은 평문이어야 한다");
    }

    /// 전환 기간에 실제로 일어나는 모양 — 같은 틱에 옛 아이폰과 새 아이폰이
    /// 함께 붙어 있다. 한쪽 규칙을 다른 쪽에 적용하면 그 기기만 조용히 화면이
    /// 빈다.
    #[test]
    fn v1_and_v2_centrals_in_the_same_tick_each_get_their_own_form() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("OLD".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("NEW".into()), max_notify_len: 185 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "OLD", now);
        let (_persist, mut client) = authorize_v2(&mut b, &fake, &mut p, "NEW", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames_by_central();
        assert_eq!(frames.len(), 2, "둘 다 받아야 한다");
        let join = |chunks: &Vec<Vec<u8>>| {
            let mut r = framing::Reassembler::new();
            let mut msg = None;
            for c in chunks {
                if let Some(m) = r.push(c) {
                    msg = Some(m);
                }
            }
            msg.expect("프레임이 완성되어야 한다")
        };
        let old = join(&frames.iter().find(|(c, _, _)| c.0 == "OLD").unwrap().2);
        let new = join(&frames.iter().find(|(c, _, _)| c.0 == "NEW").unwrap().2);
        assert!(old.starts_with(b"{"), "v1 은 평문 그대로다");
        assert!(!new.starts_with(b"{"), "v2 는 봉인돼야 한다");
        assert!(client.open(&new).is_ok(), "v2 는 자기 세션 키로 열린다");
    }

    /// 인가되지 않은 구독자는 v2 배선이 들어온 뒤에도 여전히 0바이트다.
    /// (`unauthorized_subscriber_gets_nothing` 과 같은 성질이지만, 봉인 분기가
    /// 인가 필터보다 **뒤에** 있어야 한다는 점을 여기서 다시 못 박는다.)
    #[test]
    fn unauthorized_subscriber_still_gets_nothing() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &mut p);
        assert!(fake.taken_frames_by_central().is_empty());
    }

    /// Task 8 이 세운 성질을 봉인이 들어온 뒤에도 지킨다 — 한 기기의 청킹
    /// 실패가 다른 기기를 침묵시키면 안 된다. 봉인이 for 문 안으로 들어오면서
    /// 실패 지점이 하나 늘었으므로 v2 로 다시 확인한다.
    #[test]
    fn a_v2_centrals_chunking_failure_does_not_silence_the_others() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        // max_notify_len 4 → 본문 1바이트. big_snap 을 봉인하면 255청크를 훌쩍
        // 넘으므로 TINY 에 대해서만 framing::chunk 이 TooLarge 로 실패한다.
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("BIG".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("TINY".into()), max_notify_len: 4 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize_v2(&mut b, &fake, &mut p, "BIG", now);
        authorize_v2(&mut b, &fake, &mut p, "TINY", now);

        b.on_snapshot(&big_snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames_by_central();
        assert_eq!(frames.len(), 1, "실패한 기기만 빠져야 한다");
        assert_eq!(frames[0].0, CentralId("BIG".into()));
    }

    #[test]
    fn handle_auth_signals_persist_only_on_grant() {
        let (mut b, _fake, mut p) = bridge();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        // 창이 없으면 HELLO 도 거부된다(스펙 5.1) — begin_pairing 은 사용자
        // 제스처에서만 연다. 거부에는 저장할 것이 없다.
        assert!(!b.handle_auth(&CentralId("A".into()), b"HELLO", now, &mut p),
                "창이 없으면 저장할 것이 없다");

        let code = p.begin_pairing(now);
        assert!(!b.handle_auth(&CentralId("A".into()), b"HELLO", now, &mut p),
                "코드 발급만으로는 저장할 것이 없다");
        assert!(
            b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now, &mut p),
            "인가되면 호출부가 저장하도록 true 를 돌려준다"
        );
        assert_eq!(p.issued_peers().len(), 1, "실제로 토큰이 발급돼 있다");
    }

    /// v2 도 `Granted2` 로 **새 토큰을 발급한다.** 저장 신호를 v1 `Granted` 에만
    /// 걸어두면 v2 로 페어링한 아이폰은 인증에 성공하고도 토큰이 디스크에 남지
    /// 않아, 맥을 다시 켜는 순간 사라진다 — 그 기기는 다음부터 재연결에
    /// 실패하고 매번 코드를 다시 입력해야 한다.
    #[test]
    fn handle_auth_signals_persist_on_v2_grant() {
        let (mut b, fake, mut p) = bridge();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let (persist, _ch) = authorize_v2(&mut b, &fake, &mut p, "A", now);
        assert!(persist, "v2 인가도 호출부가 저장하도록 true 를 돌려줘야 한다");
        assert_eq!(p.issued_peers().len(), 1, "실제로 토큰이 발급돼 있다");
    }

    /// v1 의 `HELLO` 와 같은 이유 — 핸드셰이크 시작만으로는 저장할 것이 없다.
    #[test]
    fn handle_auth_does_not_signal_persist_on_v2_hello() {
        let (mut b, _fake, mut p) = bridge();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        p.begin_pairing(now);
        let c = V2Client::new();
        assert!(
            !b.handle_auth(
                &CentralId("A".into()),
                format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(),
                now,
                &mut p
            ),
            "HELLO2 만으로는 토큰이 없다 — 저장할 것도 없다"
        );
    }

    #[test]
    fn unpair_all_revokes_everyone() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &mut p);
        assert_eq!(fake.taken_frames().len(), 1);

        let dropped = p.revoke_all(); b.drop_sessions(&dropped);
        b.on_snapshot(&snap(2.0, 1000), now + Duration::from_secs(2), &mut p);
        assert!(fake.taken_frames().is_empty(), "해제 후에는 다시 아무것도 못 받는다");
    }

    /// 리뷰(Task 3 전체 리뷰)가 지적한 잔여 위험: `unpair_*`/`forget_central`
    /// 은 `pairing.rs` 의 인가 상태만 지우고, macOS 쪽 전송 자원은 손대지
    /// 않았었다. 큐에 이미 들어간 청크는 `on_snapshot` 을 다시 거치지 않고도
    /// backpressure 해제 재개만으로 마저 나가므로, 인가를 잃은 central 이
    /// 남은 청크를 계속 받을 수 있었다. `revoke_targets` 가 그 경로 없이도
    /// 즉시 큐를 버린다는 것을 확인한다.
    #[test]
    fn forget_central_revokes_stale_pump_targets_immediately() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &mut p); // "A" 의 송신 큐가 생긴다
        fake.taken_frames();

        b.forget_central(&CentralId("A".into()));

        assert_eq!(
            fake.taken_revocations(),
            vec![CentralId("A".into())],
            "on_snapshot 을 다시 부르지 않아도 revoke_targets 로 즉시 반영돼야 한다"
        );
    }

    #[test]
    fn unpair_all_also_revokes_pump_targets() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &mut p);
        fake.taken_frames();

        let dropped = p.revoke_all(); b.drop_sessions(&dropped);

        assert_eq!(fake.taken_revocations(), vec![CentralId("A".into())]);
    }

    /// 재검토가 지적했다: 이름은 "unpair_peer 와 unpair_all 둘 다" 였지만
    /// 본문은 unpair_all 만 불렀다 — unpair_peer 의 revoke_targets 배선은
    /// 어떤 테스트로도 지켜지지 않고 있었다.
    #[test]
    fn unpair_peer_also_revokes_pump_targets() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &mut p);
        fake.taken_frames();

        let peer_id = p.paired_peers()[0].peer_id.clone();
        let dropped = p.revoke_peer(&peer_id);
        b.drop_sessions(&dropped);

        assert_eq!(fake.taken_revocations(), vec![CentralId("A".into())]);
    }

    /// 라운드 2 회귀 테스트: 인가된 central 과 미인가 central 이 **같은
    /// 특성을 동시에 구독**하는 상황. `on_snapshot` 은 인가된 쪽이 있으니
    /// 프레임을 만들지만, 그 프레임의 실제 수신 대상 목록에는 미인가
    /// 구독자가 들어가면 안 된다 — 이게 실기기(macOS `pump()`)에서 도청자가
    /// 스냅샷을 받지 않게 하는 유일한 방어선이다.
    #[test]
    fn unauthorized_subscriber_receives_nothing_even_when_another_is_authorized() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("AUTHORIZED".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("EAVESDROPPER".into()), max_notify_len: 185 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "AUTHORIZED", now);

        b.on_snapshot(&snap(1.0, 1000), now, &mut p);

        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1, "인가된 기기가 있으니 프레임은 만들어진다");
        let (_, _, targets) = &frames[0];
        assert_eq!(targets, &[CentralId("AUTHORIZED".into())],
                   "미인가 구독자는 같은 특성을 구독하고 있어도 대상 목록에 들어가면 안 된다");
    }
}
