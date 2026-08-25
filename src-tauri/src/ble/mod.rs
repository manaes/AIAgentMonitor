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

    /// 스냅샷 틱마다 호출한다. 게이트·구독자·직렬화·청킹을 모두 여기서 판단한다.
    pub fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime, pairing: &pairing::PairingManager) {
        if !self.enabled {
            return;
        }
        // 인가된 구독자만 대상으로 삼는다. 미인가 기기가 붙어 있어도
        // 스냅샷은 만들지 않는다(스펙 5.1). 청크 크기도 인가된 구독자
        // 기준으로만 정해야, 미인가 기기의 작은 MTU 에 끌려가지 않는다.
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
        let granted = matches!(reply, pairing::AuthReply::Granted { .. });
        self.peripheral.notify_auth(central, reply.to_json_bytes());
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
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &p);
        assert!(fake.taken_frames().is_empty(), "꺼져 있으면 아무것도 보내지 않는다");
    }

    #[test]
    fn does_nothing_without_subscribers() {
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &p);
        assert!(fake.taken_frames().is_empty(), "구독자가 없으면 직렬화도 하지 않는다");
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
        b.on_snapshot(&snap(1.0, 1000), now, &p);

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
        b.on_snapshot(&snap(1.0, 1000), t0, &p);
        assert_eq!(fake.taken_frames().len(), 1);

        // 내용이 바뀌어도 1초가 안 지나면 보내지 않는다
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_millis(400), &p);
        assert!(fake.taken_frames().is_empty());

        b.on_snapshot(&snap(3.0, 1000), t0 + Duration::from_millis(1100), &p);
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
        b.on_snapshot(&snap(1.0, 1000), t0, &p);
        let a = fake.taken_frames()[0].1[0][0];
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2), &p);
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
        b.on_snapshot(&content, t0, &p);
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
        b.on_snapshot(&content, t1, &p);
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

        b.on_snapshot(&content, t0, &p);
        assert!(fake.taken_frames().is_empty(), "청킹 실패로 프레임이 나가지 않는다");

        // 청킹이 성공할 수 있는 크기로 구독자를 바꾼 뒤, 스로틀 시간이 지난 시점에
        // 동일한 내용으로 다시 호출한다. 게이트가 리셋되지 않았다면 해시가 그대로라
        // "unchanged"로 영구 억제되어 이번에도 프레임이 비어 있을 것이다.
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 100,
        }]);
        b.on_snapshot(&content, t0 + Duration::from_millis(1100), &p);
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
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000), &p);
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

        b.on_snapshot(&snap(1.0, 1000), now, &p);
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

        b.on_snapshot(&snap(1.0, 1000), now, &p);
        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        // 청크 크기는 **인가된** 구독자만 보고 정해야 한다.
        // 미인가 B(23)를 섞으면 청크가 불필요하게 잘게 쪼개진다.
        assert!(frames[0].1[0].len() > 23, "미인가 구독자의 MTU 에 끌려가면 안 된다");
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
        b.on_snapshot(&snap(1.0, 1000), now, &p);
        assert_eq!(fake.taken_frames().len(), 1);

        let dropped = p.revoke_all(); b.drop_sessions(&dropped);
        b.on_snapshot(&snap(2.0, 1000), now + Duration::from_secs(2), &p);
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
        let (mut b, fake, mut p) = bridge();
        b.set_enabled(true).unwrap();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        authorize(&mut b, &mut p, "A", now);
        b.on_snapshot(&snap(1.0, 1000), now, &p); // authorized_targets 에 "A" 가 기록된다
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
        b.on_snapshot(&snap(1.0, 1000), now, &p);
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
        b.on_snapshot(&snap(1.0, 1000), now, &p);
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

        b.on_snapshot(&snap(1.0, 1000), now, &p);

        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1, "인가된 기기가 있으니 프레임은 만들어진다");
        let (_, _, targets) = &frames[0];
        assert_eq!(targets, &[CentralId("AUTHORIZED".into())],
                   "미인가 구독자는 같은 특성을 구독하고 있어도 대상 목록에 들어가면 안 된다");
    }
}
