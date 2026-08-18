//! BLE 미러 전송 계층. 조립 지점은 `BleBridge`.
pub mod framing;
pub mod peripheral;
pub mod send_queue;
pub mod wire;

use crate::emitter::EmitGate;
use crate::types::Snapshot;
use peripheral::{BlePeripheral, CentralId, CharId, FakePeripheral, Subscriber};
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

    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if on {
            if let Err(e) = self.peripheral.start() {
                tracing::error!("BLE 시작 실패: {e}");
                self.enabled = false;
            }
        } else {
            self.peripheral.stop();
        }
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
                return;
            }
        };
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);

        match framing::chunk(frame_id, &json, max_chunk) {
            Ok(chunks) => self.peripheral.offer_frame(CharId::Snapshot, chunks),
            Err(e) => tracing::error!("청킹 실패: {e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, AgentState, Snapshot, TokenCounts};
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
        b.set_enabled(true);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(), "구독자가 없으면 직렬화도 하지 않는다");
    }

    #[test]
    fn emits_chunked_snapshot_frame() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
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
        b.set_enabled(true);
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
        b.set_enabled(true);
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
        b.set_enabled(true);
        assert!(fake.is_started());
        b.set_enabled(false);
        assert!(!fake.is_started());
    }
}
