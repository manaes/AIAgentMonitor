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
use peripheral::{BlePeripheral, CharId};
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

    /// 스냅샷 틱마다 호출한다. 게이트·구독자·직렬화·청킹을 모두 여기서 판단한다.
    pub fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime) {
        if !self.enabled {
            return;
        }
        // 구독자가 없으면 직렬화조차 하지 않는다(스펙 4.4).
        let Some(max_chunk) = self.peripheral.min_notify_len() else {
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

        match framing::chunk(frame_id, &json, max_chunk) {
            Ok(chunks) => self.peripheral.offer_frame(CharId::Snapshot, chunks),
            Err(e) => {
                tracing::error!("청킹 실패: {e:?}");
                self.gate.reset();
            }
        }
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
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));

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
        let content = snap(1.0, 1000);
        b.on_snapshot(&content, t0);
        assert_eq!(fake.taken_frames().len(), 1);

        // 껐다 켜면 내용이 그대로여도 다시 보내야 한다 — iOS 는 재구독 직후 화면이 비어 있다.
        b.set_enabled(false).unwrap();
        b.set_enabled(true).unwrap();
        b.on_snapshot(&content, t0 + Duration::from_millis(1100));
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
}
