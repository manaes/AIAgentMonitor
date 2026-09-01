//! BLE 주변장치 추상화.
//!
//! 트레이트를 두는 이유는 구현 교체가 아니라 **BLE 하드웨어 없이 BleBridge 를 테스트하기 위함**이다.
//! 실기기 의존을 줄이는 것이 이 프로젝트의 가장 큰 개발 비용 절감 수단이다.
use std::sync::Mutex;

pub const SERVICE_UUID: &str = "07A98A35-16C7-4BBA-A296-E28B78B7E683";
pub const INFO_UUID: &str = "F494FC3B-ED50-4561-AADE-1A310C5732E6";
pub const AUTH_UUID: &str = "1403603A-4C78-4899-A2B8-FDA198101900";
pub const SNAPSHOT_UUID: &str = "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharId {
    Info,
    Auth,
    Snapshot,
}

impl CharId {
    pub fn uuid(self) -> &'static str {
        match self {
            CharId::Info => INFO_UUID,
            CharId::Auth => AUTH_UUID,
            CharId::Snapshot => SNAPSHOT_UUID,
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
    /// 한 central 에게 프레임을 넘긴다. 실제 전송과 백프레셔는 구현체가
    /// 책임진다(fire-and-forget).
    ///
    /// **central 마다 따로 부른다.** E2EE v2 에서는 세션 키가 central 마다
    /// 다르므로 바이트도 달라진다. 덤으로 청크 크기를 그 central 의 MTU 에
    /// 맞출 수 있어, MTU 가 작은 기기 하나가 모두의 청크를 잘게 만들던 문제가
    /// 사라진다.
    ///
    /// 대상이 인자로 하나 들어오므로 구현체가 인가 목록을 따로 들고 있을
    /// 필요가 없다 — 호출자(`BleBridge::on_snapshot`)가 인가된 구독자에게만
    /// 부르는 것이 곧 스펙 5.1 의 "인가된 central 에만 notify" 다.
    fn offer_frame_to(&self, ch: CharId, central: &CentralId, chunks: Vec<Vec<u8>>);
    fn subscribers(&self) -> Vec<Subscriber>;
    /// Auth 특성으로 한 central 에만 응답한다.
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>);

    /// 인가가 철회된 central 에게 아직 보내지 못한 청크를 버린다.
    ///
    /// 프레임은 이미 `offer_frame_to` 로 그 central 의 큐에 들어가 있고,
    /// backpressure 로 멈춘 큐는 `on_snapshot` 을 다시 거치지 않고도
    /// 재개 콜백(`peripheralManagerIsReadyToUpdateSubscribers:`)만으로 마저
    /// 나간다. 철회 시점에 큐를 비우지 않으면 방금 인가를 잃은 central 이
    /// 남은 청크를 계속 받는다. `forget_central`/`unpair_peer`/`unpair_all` 은
    /// 그래서 이 호출을 반드시 함께 해야 한다.
    fn revoke_targets(&self, ids: &[CentralId]);

    /// 인가된 구독자만 추린다. 프레임을 만들지 말지, 그리고 누구에게
    /// `offer_frame_to` 를 부를지가 전부 이 목록으로 정해진다(스펙 5.1).
    ///
    /// 청크 크기는 여기서 나오지 않는다 — 이제 각 구독자의 `max_notify_len`
    /// 으로 따로 정하므로 전체에 걸친 최솟값이라는 개념 자체가 없다.
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
    /// 넘어온 프레임을 받은 순서대로. 이제 호출이 central 단위이므로
    /// 대상이 튜플 안에 하나씩 들어간다.
    frames: Mutex<Vec<(CentralId, CharId, Vec<Vec<u8>>)>>,
    subs: Mutex<Vec<Subscriber>>,
    started: Mutex<bool>,
    /// Some 이면 start() 가 이 메시지로 실패한다(오류 전파 테스트용).
    start_error: Mutex<Option<String>>,
    auth_replies: Mutex<Vec<(CentralId, Vec<u8>)>>,
    revoked: Mutex<Vec<CentralId>>,
}

impl FakePeripheral {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_subscribers(&self, subs: Vec<Subscriber>) {
        *self.subs.lock().unwrap() = subs;
    }
    /// central 별로 기록된 프레임을 꺼내고 비운다.
    pub fn taken_frames_by_central(&self) -> Vec<(CentralId, CharId, Vec<Vec<u8>>)> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }
    /// 기록된 프레임을 꺼내고 비운다. 대상이 목록이던 시절의 모양 그대로
    /// 돌려준다 — 프레임 하나가 곧 central 하나이므로 목록은 언제나 한 명이다.
    pub fn taken_frames(&self) -> Vec<(CharId, Vec<Vec<u8>>, Vec<CentralId>)> {
        self.taken_frames_by_central()
            .into_iter()
            .map(|(central, ch, chunks)| (ch, chunks, vec![central]))
            .collect()
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
    /// `revoke_targets` 로 넘어온 id 를 꺼내고 비운다.
    pub fn taken_revocations(&self) -> Vec<CentralId> {
        std::mem::take(&mut *self.revoked.lock().unwrap())
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
    fn offer_frame_to(&self, ch: CharId, central: &CentralId, chunks: Vec<Vec<u8>>) {
        self.frames.lock().unwrap().push((central.clone(), ch, chunks));
    }
    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs.lock().unwrap().clone()
    }
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>) {
        self.auth_replies.lock().unwrap().push((central.clone(), payload));
    }
    fn revoke_targets(&self, ids: &[CentralId]) {
        self.revoked.lock().unwrap().extend(ids.iter().cloned());
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
        p.offer_frame_to(CharId::Snapshot, &CentralId("A".into()), vec![vec![1, 2, 3], vec![4]]);
        let frames = p.taken_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, CharId::Snapshot);
        assert_eq!(frames[0].1, vec![vec![1, 2, 3], vec![4]]);
        assert_eq!(frames[0].2, vec![CentralId("A".into())]);
    }

    #[test]
    fn uuids_match_spec() {
        assert_eq!(SERVICE_UUID, "07A98A35-16C7-4BBA-A296-E28B78B7E683");
        assert_eq!(SNAPSHOT_UUID, "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5");
        assert_eq!(AUTH_UUID, "1403603A-4C78-4899-A2B8-FDA198101900");
        assert_eq!(INFO_UUID, "F494FC3B-ED50-4561-AADE-1A310C5732E6");
    }
}
