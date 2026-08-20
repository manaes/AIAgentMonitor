//! BLE 주변장치 추상화.
//!
//! 트레이트를 두는 이유는 구현 교체가 아니라 **BLE 하드웨어 없이 BleBridge 를 테스트하기 위함**이다.
//! 실기기 의존을 줄이는 것이 이 프로젝트의 가장 큰 개발 비용 절감 수단이다.
use std::sync::Mutex;

pub const SERVICE_UUID: &str = "07A98A35-16C7-4BBA-A296-E28B78B7E683";
pub const INFO_UUID: &str = "F494FC3B-ED50-4561-AADE-1A310C5732E6";
pub const AUTH_UUID: &str = "1403603A-4C78-4899-A2B8-FDA198101900";
pub const SNAPSHOT_UUID: &str = "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5";
pub const TRIGGERS_UUID: &str = "4F60A8C2-F181-4717-AEE3-07C4D7846597";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharId {
    Info,
    Auth,
    Snapshot,
    Triggers,
}

impl CharId {
    pub fn uuid(self) -> &'static str {
        match self {
            CharId::Info => INFO_UUID,
            CharId::Auth => AUTH_UUID,
            CharId::Snapshot => SNAPSHOT_UUID,
            CharId::Triggers => TRIGGERS_UUID,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CentralId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    pub id: CentralId,
    /// CBCentral.maximumUpdateValueLength — central 마다 다르므로 구독 시점에 실측한다.
    pub max_notify_len: usize,
}

#[derive(Debug, Clone)]
pub enum PeripheralEvent {
    PoweredOn,
    PoweredOff,
    Subscribed(Subscriber),
    Unsubscribed(CentralId),
    AdvertisingStarted,
    Error(String),
    /// central 이 Auth 특성에 무언가 썼다. 해석은 pairing 모듈이 한다.
    AuthWrite { central: CentralId, data: Vec<u8> },
    /// 링크가 끊겼다. 인가 상태를 그 자리에서 지우기 위해 필요하다.
    Disconnected(CentralId),
}

pub trait BlePeripheral: Send + Sync {
    fn start(&self) -> anyhow::Result<()>;
    fn stop(&self);
    /// 프레임을 넘긴다. 실제 전송과 백프레셔는 구현체가 책임진다(fire-and-forget).
    /// `authorized` 는 이 프레임을 받아도 되는 central id 목록이다 — 구현체는
    /// 실제 notify 를 이 목록으로만 좁혀야 한다. 그 특성을 구독했더라도 이
    /// 목록에 없는 central 은 받아서는 안 된다(스펙 5.1).
    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>, authorized: &[CentralId]);
    fn subscribers(&self) -> Vec<Subscriber>;
    /// 모든 구독자가 받을 수 있는 최대 청크 크기. 구독자가 없으면 None.
    fn min_notify_len(&self) -> Option<usize> {
        self.subscribers().iter().map(|s| s.max_notify_len).min()
    }
    /// Auth 특성으로 한 central 에만 응답한다.
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>);

    /// 인가된 구독자만 추린다. 청크 크기 계산도 이 목록으로 해야
    /// 미인가 기기의 작은 MTU 에 끌려가지 않는다.
    fn authorized_subscribers(&self, is_authorized: &dyn Fn(&CentralId) -> bool) -> Vec<Subscriber> {
        self.subscribers()
            .into_iter()
            .filter(|s| is_authorized(&s.id))
            .collect()
    }
}

/// 테스트용 구현. 넘어온 프레임을 기록만 한다.
#[derive(Debug, Default)]
pub struct FakePeripheral {
    frames: Mutex<Vec<(CharId, Vec<Vec<u8>>, Vec<CentralId>)>>,
    subs: Mutex<Vec<Subscriber>>,
    started: Mutex<bool>,
    /// Some 이면 start() 가 이 메시지로 실패한다(오류 전파 테스트용).
    start_error: Mutex<Option<String>>,
    auth_replies: Mutex<Vec<(CentralId, Vec<u8>)>>,
}

impl FakePeripheral {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_subscribers(&self, subs: Vec<Subscriber>) {
        *self.subs.lock().unwrap() = subs;
    }
    /// 기록된 프레임을 꺼내고 비운다.
    pub fn taken_frames(&self) -> Vec<(CharId, Vec<Vec<u8>>, Vec<CentralId>)> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }
    pub fn is_started(&self) -> bool {
        *self.started.lock().unwrap()
    }
    pub fn set_start_error(&self, msg: Option<String>) {
        *self.start_error.lock().unwrap() = msg;
    }
    /// 기록된 Auth 응답을 꺼내고 비운다.
    pub fn taken_auth_replies(&self) -> Vec<(CentralId, Vec<u8>)> {
        std::mem::take(&mut *self.auth_replies.lock().unwrap())
    }
}

impl BlePeripheral for FakePeripheral {
    fn start(&self) -> anyhow::Result<()> {
        if let Some(msg) = self.start_error.lock().unwrap().clone() {
            return Err(anyhow::anyhow!(msg));
        }
        *self.started.lock().unwrap() = true;
        Ok(())
    }
    fn stop(&self) {
        *self.started.lock().unwrap() = false;
    }
    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>, authorized: &[CentralId]) {
        self.frames.lock().unwrap().push((ch, chunks, authorized.to_vec()));
    }
    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs.lock().unwrap().clone()
    }
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>) {
        self.auth_replies.lock().unwrap().push((central.clone(), payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_offered_frames() {
        let p = FakePeripheral::new();
        p.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 20,
        }]);
        p.offer_frame(CharId::Snapshot, vec![vec![1, 2, 3], vec![4]], &[CentralId("A".into())]);
        let frames = p.taken_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, CharId::Snapshot);
        assert_eq!(frames[0].1, vec![vec![1, 2, 3], vec![4]]);
        assert_eq!(frames[0].2, vec![CentralId("A".into())]);
    }

    #[test]
    fn fake_reports_smallest_subscriber_mtu() {
        let p = FakePeripheral::new();
        p.set_subscribers(vec![
            Subscriber { id: CentralId("A".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("B".into()), max_notify_len: 23 },
        ]);
        assert_eq!(
            p.min_notify_len(),
            Some(23),
            "가장 작은 구독자에 맞춰야 모두가 받을 수 있다"
        );
    }

    #[test]
    fn min_notify_len_is_none_without_subscribers() {
        let p = FakePeripheral::new();
        assert_eq!(p.min_notify_len(), None);
    }

    #[test]
    fn uuids_match_spec() {
        assert_eq!(SERVICE_UUID, "07A98A35-16C7-4BBA-A296-E28B78B7E683");
        assert_eq!(SNAPSHOT_UUID, "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5");
        assert_eq!(TRIGGERS_UUID, "4F60A8C2-F181-4717-AEE3-07C4D7846597");
        assert_eq!(AUTH_UUID, "1403603A-4C78-4899-A2B8-FDA198101900");
        assert_eq!(INFO_UUID, "F494FC3B-ED50-4561-AADE-1A310C5732E6");
    }
}
