//! BLE 미러 전송 계층. 조립 지점은 `BleBridge`.
pub mod framing;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod peripheral;
pub mod pairing;
pub mod peers;
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

pub struct BleBridge {
    peripheral: Arc<dyn BlePeripheral>,
    gate: EmitGate,
    enabled: bool,
    next_frame_id: u8,
    pairing: pairing::PairingManager,
}

impl BleBridge {
    pub fn new(peripheral: Arc<dyn BlePeripheral>) -> Self {
        Self {
            peripheral,
            gate: EmitGate::new(BLE_THROTTLE),
            enabled: false,
            next_frame_id: 0,
            pairing: pairing::PairingManager::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
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
            // peripheral 이 완전히 내려가므로 authorized_targets 스테일 문제는 없지만,
            // PairingManager.authorized 는 이 호출 없이는 그대로 남는다 — 그러면 다시
            // 공유를 켰을 때 didUnsubscribeFromCharacteristic 이 온 적 없는 central 이
            // 여전히 인가된 것으로 표시된다(전체 브랜치 리뷰 I-2).
            self.pairing.end_all_sessions();
        }
        Ok(())
    }

    /// 블루투스 전원이 꺼졌을 때 호출한다. `set_enabled(false)` 와 같은 이유로
    /// 세션 인가를 정리해야 한다 — `PowerOff` 는 `did_unsubscribe` 를 거치지
    /// 않으므로 그러지 않으면 인가가 실제 연결보다 오래 살아남는다.
    pub fn end_all_sessions(&mut self) {
        self.pairing.end_all_sessions();
    }

    /// 스냅샷 틱마다 호출한다. 게이트·구독자·직렬화·청킹을 모두 여기서 판단한다.
    pub fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime) {
        if !self.enabled {
            return;
        }
        // 인가된 구독자만 대상으로 삼는다. 미인가 기기가 붙어 있어도
        // 스냅샷은 만들지 않는다(스펙 5.1). 청크 크기도 인가된 구독자
        // 기준으로만 정해야, 미인가 기기의 작은 MTU 에 끌려가지 않는다.
        let pairing = &self.pairing;
        let authorized = self
            .peripheral
            .authorized_subscribers(&|id| pairing.is_authorized(id));
        let Some(max_chunk) = authorized.iter().map(|s| s.max_notify_len).min() else {
            return;
        };
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

        let ids: Vec<CentralId> = authorized.iter().map(|s| s.id.clone()).collect();
        match framing::chunk(frame_id, &json, max_chunk) {
            Ok(chunks) => self.peripheral.offer_frame(CharId::Snapshot, chunks, &ids),
            Err(e) => {
                tracing::error!("청킹 실패: {e:?}");
                self.gate.reset();
            }
        }
    }

    /// 사용자가 Mac Devices 탭에서 "페어링 시작"을 누를 때만 호출한다. 다른
    /// 어떤 경로(연결, 구독, HELLO, 앱 재시작)에서도 호출해서는 안 된다 —
    /// 6자리 코드의 무차별 대입 방어 전체가 이 성질에 얹혀 있다(스펙 5.1/5.2).
    pub fn begin_pairing(&mut self, now: SystemTime) -> String {
        self.pairing.begin_pairing(now)
    }

    /// 페어링 창의 현재 상태. 코드·남은 초·남은 시도를 함께 주며, 만료와
    /// 시도 소진을 구분한다.
    pub fn pairing_window(&self, now: SystemTime) -> pairing::PairingWindow {
        self.pairing.pairing_window(now)
    }

    /// Devices 패널에 그릴 기기 목록.
    pub fn paired_peers(&self) -> Vec<pairing::PairedPeer> {
        self.pairing.paired_peers()
    }

    /// 디스크에 저장할 (토큰, 페어링 시각) 목록.
    pub fn stored_peers(&self) -> Vec<peers::StoredPeer> {
        self.pairing
            .issued_peers()
            .into_iter()
            .map(|(token, paired_at)| peers::StoredPeer { token, paired_at })
            .collect()
    }

    /// Auth 특성 쓰기를 처리하고 응답을 보낸다. 인가가 성립하면(`CODE:` 로
    /// 새 토큰이 발급되면) 영속화할 전체 목록을 돌려준다 — 호출부가
    /// `PeerStore::save_to` 로 디스크에 쓴다.
    pub fn handle_auth(
        &mut self,
        central: &CentralId,
        data: &[u8],
        now: SystemTime,
    ) -> Option<Vec<peers::StoredPeer>> {
        let req = pairing::parse_auth_request(data);
        let reply = self.pairing.handle(central, req, now);
        let payload = match &reply {
            // 코드는 Mac 화면에만 보여준다 — central 에게 보내면 페어링이 무의미해진다.
            pairing::AuthReply::AwaitingCode => br#"{"ok":false,"await":"code"}"#.to_vec(),
            pairing::AuthReply::Nonce { nonce } => {
                format!(r#"{{"ok":false,"nonce":"{nonce}"}}"#).into_bytes()
            }
            pairing::AuthReply::Authorized => br#"{"ok":true}"#.to_vec(),
            pairing::AuthReply::Granted { token } => {
                format!(r#"{{"ok":true,"token":"{token}"}}"#).into_bytes()
            }
            pairing::AuthReply::Denied { left } => {
                format!(r#"{{"ok":false,"left":{left}}}"#).into_bytes()
            }
            pairing::AuthReply::Rejected => br#"{"ok":false}"#.to_vec(),
        };
        self.peripheral.notify_auth(central, payload);

        match reply {
            pairing::AuthReply::Granted { .. } => Some(self.stored_peers()),
            _ => None,
        }
    }

    /// 링크가 끊긴(또는 사용자가 세션만 끊으려는) central 의 인가를 지운다.
    /// 같은 식별자가 재사용될 수 있으므로 연결 단위 인가는 연결이 끝나면
    /// 사라져야 한다. 저장된 토큰 자체는 지우지 않는다 — 같은 토큰으로
    /// 재연결하면 즉시 재인가된다.
    pub fn forget_central(&mut self, central: &CentralId) {
        self.pairing.end_session(central);
        self.peripheral.revoke_targets(std::slice::from_ref(central));
    }

    /// 디스크에서 읽은 페어링 목록을 복원한다(앱 시작 시 1회).
    pub fn load_peers(&mut self, peers: Vec<peers::StoredPeer>) {
        self.pairing
            .load_peers(peers.into_iter().map(|p| (p.token, p.paired_at)).collect());
    }

    /// 기기 하나만 골라 해제한다(스펙 6 `ble_unpair`). 내려간 central 을
    /// 돌려준다 — 호출자가 저장소를 갱신하고, 가능하면 실제 연결도 끊는다.
    pub fn unpair_peer(&mut self, peer_id: &str) -> Vec<CentralId> {
        let dropped = self.pairing.revoke_peer(peer_id);
        self.peripheral.revoke_targets(&dropped);
        dropped
    }

    /// 모든 기기의 인가와 토큰을 폐기한다(전체 언페어링). `CBPeripheralManager`
    /// 에는 연결된 central 을 강제로 끊는 API 가 없으므로(1단계에서 확인),
    /// 실제 차단은 "인가가 없으면 notify 하지 않는다" 로 이뤄진다 — 이
    /// 반환값은 호출자가 저장소를 갱신하거나 로그를 남기는 데 쓴다.
    pub fn unpair_all(&mut self) -> Vec<CentralId> {
        let dropped = self.pairing.revoke_all();
        self.peripheral.revoke_targets(&dropped);
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, AgentState, Snapshot, TokenCounts};
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
                projects: vec![],
                triggered_by: None,
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
                    projects: vec![],
                    triggered_by: None,
                })
                .collect(),
        }
    }

    fn bridge() -> (BleBridge, Arc<FakePeripheral>) {
        let fake = Arc::new(FakePeripheral::new());
        (BleBridge::new(fake.clone()), fake)
    }

    /// 테스트 편의: 사용자가 페어링 창을 열고(begin_pairing), 그 central 이
    /// HELLO → 올바른 코드로 인가받는 과정을 흉내낸다. 인가 필터가 들어간
    /// 뒤로는 on_snapshot 이 이 central 에게 실제로 프레임을 보내려면
    /// 먼저 이 과정을 거쳐야 한다.
    fn authorize(b: &mut BleBridge, central: &str, now: SystemTime) {
        let code = b.begin_pairing(now);
        b.handle_auth(&CentralId(central.to_string()), b"HELLO", now);
        b.handle_auth(&CentralId(central.to_string()), format!("CODE:{code}").as_bytes(), now);
    }

    #[test]
    fn set_enabled_false_drops_session_authorization_but_keeps_the_token() {
        // BleBridge::set_enabled(false) 가 실제로 end_all_sessions 를 부르는지 —
        // PairingManager 유닛 테스트는 그 함수 자체만 검증하고 이 배선은 안 본다.
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        assert!(b.paired_peers()[0].connected, "인가 직후에는 연결됨이어야 한다");

        b.set_enabled(false).unwrap();

        assert!(
            !b.paired_peers()[0].connected,
            "공유를 끄면 didUnsubscribe 없이도 즉시 연결됨 표시가 내려가야 한다(I-2)"
        );
        assert_eq!(
            b.paired_peers().len(),
            1,
            "세션 인가만 지워야 한다 — 저장된 페어링(토큰) 자체는 남아야 한다"
        );
    }

    #[test]
    fn does_nothing_while_disabled() {
        let (mut b, fake) = bridge();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(), "꺼져 있으면 아무것도 보내지 않는다");
    }

    #[test]
    fn does_nothing_without_subscribers() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(), "구독자가 없으면 직렬화도 하지 않는다");
    }

    #[test]
    fn emits_chunked_snapshot_frame() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now);

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
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", t0);
        b.on_snapshot(&snap(1.0, 1000), t0);
        assert_eq!(fake.taken_frames().len(), 1);

        // 내용이 바뀌어도 1초가 안 지나면 보내지 않는다
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_millis(400));
        assert!(fake.taken_frames().is_empty());

        b.on_snapshot(&snap(3.0, 1000), t0 + Duration::from_millis(1100));
        assert_eq!(fake.taken_frames().len(), 1);
    }

    #[test]
    fn frame_id_increments_per_frame() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", t0);
        b.on_snapshot(&snap(1.0, 1000), t0);
        let a = fake.taken_frames()[0].1[0][0];
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2));
        let c = fake.taken_frames()[0].1[0][0];
        assert_eq!(c, a.wrapping_add(1), "frame_id 는 프레임마다 증가한다");
    }

    #[test]
    fn disabling_stops_the_peripheral() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        assert!(fake.is_started());
        b.set_enabled(false).unwrap();
        assert!(!fake.is_started());
    }

    #[test]
    fn re_enabling_sends_a_frame_even_if_content_is_unchanged() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", t0);
        let content = snap(1.0, 1000);
        b.on_snapshot(&content, t0);
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
        authorize(&mut b, "A", t1);
        b.on_snapshot(&content, t1);
        assert_eq!(
            fake.taken_frames().len(),
            1,
            "재개 후 첫 프레임이 unchanged 로 억제되면 미러가 빈 화면으로 남는다"
        );
    }

    #[test]
    fn failed_start_reports_error_and_rolls_back() {
        let (mut b, fake) = bridge();
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
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        // max_notify_len 4 → 본문 1바이트. big_snap 은 255바이트를 훌쩍 넘으므로
        // framing::chunk 이 반드시 TooLarge 로 실패한다.
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 4,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", t0);
        let content = big_snap(1.0, 1000);

        b.on_snapshot(&content, t0);
        assert!(fake.taken_frames().is_empty(), "청킹 실패로 프레임이 나가지 않는다");

        // 청킹이 성공할 수 있는 크기로 구독자를 바꾼 뒤, 스로틀 시간이 지난 시점에
        // 동일한 내용으로 다시 호출한다. 게이트가 리셋되지 않았다면 해시가 그대로라
        // "unchanged"로 영구 억제되어 이번에도 프레임이 비어 있을 것이다.
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 100,
        }]);
        b.on_snapshot(&content, t0 + Duration::from_millis(1100));
        assert!(
            !fake.taken_frames().is_empty(),
            "실패한 프레임이 게이트를 영구 억제해선 안 된다 — 다음 틱에 재시도되어야 한다"
        );
    }

    // ---- 인가 필터(3단계) ----

    #[test]
    fn unauthorized_subscriber_gets_nothing() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(),
                "페어링하지 않은 기기는 한 바이트도 받으면 안 된다");
    }

    #[test]
    fn authorized_subscriber_receives_snapshot() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);

        // 사용자가 창을 연다 → HELLO → 그 코드로 인가
        authorize(&mut b, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now);
        assert_eq!(fake.taken_frames().len(), 1, "인가 후에는 받는다");
    }

    #[test]
    fn mixed_subscribers_only_authorized_are_targeted() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("A".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("B".into()), max_notify_len: 23 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);

        b.on_snapshot(&snap(1.0, 1000), now);
        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        // 청크 크기는 **인가된** 구독자만 보고 정해야 한다.
        // 미인가 B(23)를 섞으면 청크가 불필요하게 잘게 쪼개진다.
        assert!(frames[0].1[0].len() > 23, "미인가 구독자의 MTU 에 끌려가면 안 된다");
    }

    #[test]
    fn handle_auth_returns_tokens_to_persist_only_on_grant() {
        let (mut b, _fake) = bridge();
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        // 창이 없으면 HELLO 도 거부된다(스펙 5.1) — begin_pairing 은 사용자
        // 제스처에서만 연다. 거부에는 저장할 것이 없다.
        assert_eq!(b.handle_auth(&CentralId("A".into()), b"HELLO", now), None,
                   "창이 없으면 저장할 것이 없다");

        let code = b.begin_pairing(now);
        assert_eq!(b.handle_auth(&CentralId("A".into()), b"HELLO", now), None,
                   "코드 발급만으로는 저장할 것이 없다");
        let saved = b.handle_auth(&CentralId("A".into()), format!("CODE:{code}").as_bytes(), now);
        assert_eq!(saved.map(|v| v.len()), Some(1), "인가되면 토큰 목록을 돌려준다");
    }

    #[test]
    fn unpair_all_revokes_everyone() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now);
        assert_eq!(fake.taken_frames().len(), 1);

        b.unpair_all();
        b.on_snapshot(&snap(2.0, 1000), now + Duration::from_secs(2));
        assert!(fake.taken_frames().is_empty(), "해제 후에는 다시 아무것도 못 받는다");
    }

    /// 리뷰(Task 3 전체 리뷰)가 지적한 잔여 위험: `unpair_*`/`forget_central`
    /// 은 `pairing.rs` 의 인가 상태만 지우고, macOS 쪽 `authorized_targets`
    /// (characteristic 별 마지막 전송 대상)는 손대지 않았었다. 인가된
    /// 구독자가 0명이 되면 `on_snapshot` 이 `offer_frame` 이전에 조기
    /// 반환하므로 그 뒤로 `authorized_targets` 는 영원히 갱신되지 않고,
    /// 스테일한 목록을 들고 있다가 backpressure 해제 재개 시 방금 철회된
    /// central 에게 계속 보낼 수 있었다. `revoke_targets` 가 그 경로 없이도
    /// 즉시 지운다는 것을 확인한다.
    #[test]
    fn forget_central_revokes_stale_pump_targets_immediately() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now); // authorized_targets 에 "A" 가 기록된다
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
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now);
        fake.taken_frames();

        b.unpair_all();

        assert_eq!(fake.taken_revocations(), vec![CentralId("A".into())]);
    }

    /// 재검토가 지적했다: 이름은 "unpair_peer 와 unpair_all 둘 다" 였지만
    /// 본문은 unpair_all 만 불렀다 — unpair_peer 의 revoke_targets 배선은
    /// 어떤 테스트로도 지켜지지 않고 있었다.
    #[test]
    fn unpair_peer_also_revokes_pump_targets() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now);
        fake.taken_frames();

        let peer_id = b.paired_peers()[0].peer_id.clone();
        b.unpair_peer(&peer_id);

        assert_eq!(fake.taken_revocations(), vec![CentralId("A".into())]);
    }

    /// 라운드 2 회귀 테스트: 인가된 central 과 미인가 central 이 **같은
    /// 특성을 동시에 구독**하는 상황. `on_snapshot` 은 인가된 쪽이 있으니
    /// 프레임을 만들지만, 그 프레임의 실제 수신 대상 목록에는 미인가
    /// 구독자가 들어가면 안 된다 — 이게 실기기(macOS `pump()`)에서 도청자가
    /// 스냅샷을 받지 않게 하는 유일한 방어선이다.
    #[test]
    fn unauthorized_subscriber_receives_nothing_even_when_another_is_authorized() {
        let (mut b, fake) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![
            Subscriber { id: CentralId("AUTHORIZED".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("EAVESDROPPER".into()), max_notify_len: 185 },
        ]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let code = b.begin_pairing(now);
        b.handle_auth(&CentralId("AUTHORIZED".into()), b"HELLO", now);
        b.handle_auth(&CentralId("AUTHORIZED".into()), format!("CODE:{code}").as_bytes(), now);

        b.on_snapshot(&snap(1.0, 1000), now);

        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1, "인가된 기기가 있으니 프레임은 만들어진다");
        let (_, _, targets) = &frames[0];
        assert_eq!(targets, &[CentralId("AUTHORIZED".into())],
                   "미인가 구독자는 같은 특성을 구독하고 있어도 대상 목록에 들어가면 안 된다");
    }
}
