//! CBPeripheralManager 직접 구현 (스펙 3.2). 0단계 스파이크에서 검증한 방식이다.
//!
//! 스레드 규약: CoreBluetooth 호출과 델리게이트 콜백을 모두 **메인 스레드**에서 처리한다.
//! CBPeripheralManager 를 queue=None 으로 만들면 콜백이 메인 큐로 오고, Tauri 가 이미
//! 메인 런루프를 돌리고 있으므로 별도 런루프가 필요 없다.
//! SendQueue 도 메인 스레드가 소유해 updateValue 의 bool 반환을 스레드 왕복 없이 처리한다.
//! tokio 쪽에서는 `offer_frame_to` 로 프레임만 던지고(fire-and-forget) 즉시 돌아온다.
use super::peripheral::{
    BlePeripheral, CentralId, CharId, PeripheralEvent, Subscriber, SERVICE_UUID,
};
use super::send_queue::SendQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::*;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;
use tokio::sync::mpsc::UnboundedSender;

fn uuid(s: &str) -> Retained<CBUUID> {
    unsafe { CBUUID::UUIDWithString(&NSString::from_str(s)) }
}

/// 델리게이트의 원시 포인터(usize)를 메인 스레드 밖으로도 옮길 수 있게 보관한다.
/// `usize` 는 Send 이므로 이 슬롯 자체는 스레드 간 이동이 안전하다.
/// 역참조는 `run_on_main_thread` 클로저 안에서만 이뤄지므로 안전성이 유지된다.
static DELEGATE: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

fn delegate_slot() -> &'static Mutex<Option<usize>> {
    DELEGATE.get_or_init(|| Mutex::new(None))
}

/// 사용자가 공유를 켜두었는지. `start()`/`stop()` 만 값을 쓰고 델리게이트 콜백은 읽기만 한다.
/// 델리게이트는 `BleBridge.enabled` 를 볼 수 없으므로, 이 플래그가 없으면 제어 센터에서
/// 블루투스를 껐다 켜는 것만으로 didUpdateState(PoweredOn) 이 publish() → advertise() 를
/// 태워 사용자가 끈 공유를 되살린다. 스냅샷은 새어 나가지 않지만(on_snapshot 이 !enabled 에서
/// 즉시 반환) 기기 이름이 다시 전파되므로, 꺼짐이라고 적힌 토글은 꺼짐이어야 한다.
static SHARING_WANTED: AtomicBool = AtomicBool::new(false);

/// 슬롯에 델리게이트가 있으면 `f`, 없으면 `absent` 를 메인 스레드에서 실행한다.
/// 원시 포인터를 `&Delegate` 로 되돌리는 유일한 지점 — 이 함수 밖에서는 절대 역참조하지 않는다.
///
/// "있으면/없으면" 판정을 이 클로저 **안**에서 하는 것이 핵심이다. 슬롯에 값을 쓰는 것도
/// 메인 스레드뿐이므로, 판정과 기록이 같은 메인 스레드 턴에서 직렬화된다. 호출 스레드에서
/// 미리 검사하면 start() 두 번이 겹칠 때 둘 다 "없음"으로 보고 델리게이트를 둘 만든다.
fn with_delegate_or(
    app: &AppHandle,
    f: impl FnOnce(&Delegate) + Send + 'static,
    absent: impl FnOnce() + Send + 'static,
) -> tauri::Result<()> {
    app.run_on_main_thread(move || {
        let Some(raw) = *delegate_slot().lock().unwrap() else {
            absent();
            return;
        };
        // 메인 스레드에서만 실행되는 클로저 안이므로 역참조가 안전하다.
        f(unsafe { &*(raw as *const Delegate) });
    })
}

/// 델리게이트가 없으면 아무것도 하지 않는 `with_delegate_or`.
fn with_delegate(app: &AppHandle, f: impl FnOnce(&Delegate) + Send + 'static) {
    let _ = with_delegate_or(app, f, || {});
}

/// 송신 큐의 키: (characteristic uuid, central id).
///
/// central 을 키에 넣는 것이 이 계층의 핵심이다. E2EE v2 에서는 세션 키가
/// central 마다 달라 같은 스냅샷이라도 바이트가 다르므로, 특성 하나에 큐
/// 하나였던 예전 구조로는 담을 수 없다. 부수 효과로 **키가 곧 전송 대상**이
/// 되어, 인가 목록을 따로 들고 다니며 동기화하던 문제가 구조적으로 사라진다.
type QueueKey = (&'static str, String);

/// 메인 스레드에서만 접근하는 상태.
struct MainState {
    manager: Option<Retained<CBPeripheralManager>>,
    chars: HashMap<&'static str, Retained<CBMutableCharacteristic>>,
    subs: HashMap<String, (Retained<CBCentral>, usize)>,
    /// central 별 송신 큐. 큐가 있다는 것 자체가 "이 central 에게 보내도 된다"는
    /// 뜻이다 — 큐는 `offer_frame_to`(인가된 구독자에게만 불린다)로만 생기고
    /// `revoke_targets`/구독 해제가 지운다. 큐가 없으면 아무것도 나가지 않으므로
    /// 기본값이 fail-closed 다(스펙 5.1).
    queues: HashMap<QueueKey, SendQueue>,
    events: UnboundedSender<PeripheralEvent>,
}

impl MainState {
    /// **큐를 만드는 유일한 경로.** `pump()` 에는 인가 검사가 없고 큐의 존재
    /// 자체가 인가이므로(위 `queues` 주석), 이 계층의 인가 관문 전체가 이
    /// 함수 하나다. 조건은 둘의 논리곱이다:
    ///
    /// 1. **인가됨** — 호출자(`BleBridge::on_snapshot`)가 인가된 구독자에게만
    ///    `offer_frame_to` 를 부른다.
    /// 2. **구독 중** — 여기서 `subs` 로 확인한다. `subs` 는 메인 스레드가
    ///    콜백에서 직접 갱신하므로, tokio 쪽 `subs_mirror` 가 아직 따라잡지
    ///    못한 사이(`did_unsubscribe` 는 지났는데 `apply_event` 는 아직)에
    ///    떠난 central 의 큐가 되살아나는 것을 막는다. 바이트가 새지는
    ///    않지만(`pump` 도 `subs` 를 본다) 큐가 `stop()` 까지 남고, 본딩하지
    ///    않은 peer 의 CBCentral identifier 는 바뀌므로 계속 쌓인다.
    ///
    /// **다른 곳에서 `queues` 에 키를 넣지 마라.** 예를 들어 재구독 지연을
    /// 줄이겠다고 `did_subscribe` 에서 큐를 미리 만들거나, `pump` 의
    /// `let Some(q) = ... else { return }` 을 `entry().or_default()` 로
    /// 단순화하면, 그 순간 미인가 구독자에게도 큐가 생겨 `pump_all` 이
    /// 그들을 순회한다 — 아래 `queued_centrals` 주석이 말하는 "라운드 2가
    /// 고친 누출"이 그대로 되살아난다. 대신 `pump` 에 필터를 다시 넣는 것도
    /// 답이 아니다(그러면 동기화해야 할 상태가 또 생긴다).
    ///
    /// `subs` 를 제네릭으로 받는 이유는 실제 값이 `Retained<CBCentral>` 이라
    /// 테스트에서 만들 수 없기 때문이다 — 판정에 필요한 건 키뿐이다.
    fn enqueue<T>(
        queues: &mut HashMap<QueueKey, SendQueue>,
        subs: &HashMap<String, T>,
        ch_uuid: &'static str,
        central_id: &str,
        chunks: Vec<Vec<u8>>,
    ) {
        if !subs.contains_key(central_id) {
            return;
        }
        queues
            .entry((ch_uuid, central_id.to_string()))
            .or_default()
            .offer(chunks);
    }

    /// 이 characteristic 으로 보낼 것이 남아 있는 central 들 — 즉 `pump_all` 의
    /// 대상 전체다. `CBCentral`/`CBPeripheralManager` 를 전혀 필요로 하지 않는
    /// 순수 함수로 뽑아둔 이유는, 이 파일의 나머지는 실제 CoreBluetooth 객체가
    /// 있어야만 돌아가서 `#[cfg(test)]` 로 묶을 수 없기 때문이다 — 라운드 2가
    /// 고친 누출(인가 필터가 추상화 계층에서만 걸리고 실기기 전송 경로는
    /// 구독자 전원에게 보내던 문제)이 바로 "실제 동작이 테스트 사각지대"
    /// 였다. 지금은 그 필터가 `enqueue` 의 큐 생성 조건으로 옮겨갔고, 여기는
    /// 키 집합 연산일 뿐이므로 양쪽 다 사각지대 없이 검증된다.
    fn queued_centrals(queues: &HashMap<QueueKey, SendQueue>, ch_uuid: &str) -> Vec<String> {
        queues
            .keys()
            .filter(|(u, _)| *u == ch_uuid)
            .map(|(_, id)| id.clone())
            .collect()
    }

    /// `revoke_targets` 의 실제 결정 로직. 인가를 잃은 central 의 큐를 통째로
    /// 버린다 — 큐가 곧 대상이므로 이것이 전송 대상에서 빼는 것이다.
    /// `queued_centrals` 와 같은 이유로 순수 함수로 뽑아둔다.
    fn drop_queues_for(queues: &mut HashMap<QueueKey, SendQueue>, revoked: &[String]) {
        queues.retain(|(_, central), _| !revoked.contains(central));
    }

    /// 한 central 의 큐에 쌓인 청크를 가능한 만큼 내보낸다. 인가 필터가 따로
    /// 없는 이유는 큐의 존재 자체가 인가이기 때문이다(위 `queues` 주석).
    fn pump(&mut self, ch_uuid: &'static str, central_id: &str) {
        let (Some(mgr), Some(ch)) = (self.manager.clone(), self.chars.get(ch_uuid).cloned())
        else {
            return;
        };
        // 구독이 끊긴 central 은 보낼 곳이 없다. 큐는 구독 해제 콜백과
        // `revoke_targets` 가 치우므로 여기서는 그냥 건너뛴다.
        let Some((central, _)) = self.subs.get(central_id).cloned() else {
            return;
        };
        let target: &CBCentral = &central;
        let targets = NSArray::from_slice(&[target]);
        let Some(q) = self.queues.get_mut(&(ch_uuid, central_id.to_string())) else {
            return;
        };
        q.pump(|chunk| {
            let data = NSData::with_bytes(chunk);
            let ok = unsafe {
                mgr.updateValue_forCharacteristic_onSubscribedCentrals(
                    &data,
                    &ch,
                    Some(&targets),
                )
            };
            if !ok {
                unsafe {
                    mgr.updateValue_forCharacteristic_onSubscribedCentrals(
                        &data,
                        &ch,
                        None,
                    )
                }
            } else {
                true
            }
        });
    }

    /// 이 characteristic 을 기다리는 모든 central 의 큐를 두드린다.
    /// backpressure 해제 재개 콜백은 어느 central 때문에 막혔는지 알려주지
    /// 않으므로, 재개 시에는 전부 한 번씩 돌려야 한다.
    fn pump_all(&mut self, ch_uuid: &'static str) {
        for id in Self::queued_centrals(&self.queues, ch_uuid) {
            self.pump(ch_uuid, &id);
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AimBleDelegate"]
    #[ivars = RefCell<MainState>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl CBPeripheralManagerDelegate for Delegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, mgr: &CBPeripheralManager) {
            let state = unsafe { mgr.state() };
            let powered = state == CBManagerState::PoweredOn;
            let st = self.ivars().borrow();
            let _ = st.events.send(if powered {
                PeripheralEvent::PoweredOn
            } else {
                PeripheralEvent::PoweredOff
            });
            // 단순히 꺼져 있는 것과 권한 거부/미지원은 사용자에게 다른 안내가 필요하므로 구분해서 보낸다.
            match state {
                CBManagerState::Unauthorized => {
                    let _ = st.events.send(PeripheralEvent::Error(
                        "블루투스 권한이 거부되었습니다. 시스템 설정 > 개인정보 보호 및 보안 > Bluetooth 에서 허용하세요."
                            .to_string(),
                    ));
                }
                CBManagerState::Unsupported => {
                    let _ = st.events.send(PeripheralEvent::Error(
                        "이 기기는 BLE 주변장치 모드를 지원하지 않습니다.".to_string(),
                    ));
                }
                _ => {}
            }
            drop(st);
            // 전원이 내려가면 구독은 전부 무효다 — stop() 과 같은 이유로 구독과
            // 큐를 함께 버린다. 제어 센터에서 블루투스를 껐다 켜는 경로는 stop()
            // 을 거치지 않으므로(PoweredOff 는 subs_mirror 만 비운다), 여기서
            // 치우지 않으면 포화된 채로 꺼진 큐가 paused=true 로 남는다.
            // SendQueue::pump 는 paused 면 즉시 반환하는데, 재게시 뒤에는 그것을
            // 풀어줄 isReadyToUpdateSubscribers 가 오지 않을 수 있다. 큐가
            // central 별이 된 지금은 그 결과가 "전체 정지"가 아니라 "그 기기만
            // 조용히 죽음"이라 더 알아채기 어렵다(이 앱은 로그를 남기지 않는다).
            if !powered {
                let mut st = self.ivars().borrow_mut();
                st.subs.clear();
                st.queues.clear();
            }
            // 사용자가 공유를 끈 상태라면 전원이 다시 들어와도 서비스를 올리지 않는다.
            // publish() 를 건너뛰면 didAddService 가 오지 않으므로 advertise() 도 함께 막힌다.
            if powered && SHARING_WANTED.load(Ordering::SeqCst) {
                self.publish(mgr);
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(&self, mgr: &CBPeripheralManager, _s: &CBService, err: Option<&NSError>) {
            if let Some(e) = err {
                let _ = self.ivars().borrow().events.send(PeripheralEvent::Error(e.to_string()));
                return;
            }
            self.advertise(mgr);
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_adv(&self, _m: &CBPeripheralManager, err: Option<&NSError>) {
            let st = self.ivars().borrow();
            let _ = st.events.send(match err {
                Some(e) => PeripheralEvent::Error(e.to_string()),
                None => PeripheralEvent::AdvertisingStarted,
            });
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(&self, _m: &CBPeripheralManager, central: &CBCentral, ch: &CBCharacteristic) {
            let mtu = unsafe { central.maximumUpdateValueLength() };
            let id = central_id(central);
            {
                let mut st = self.ivars().borrow_mut();
                st.subs.insert(id.clone(), (central.retain(), mtu));
                let _ = st.events.send(PeripheralEvent::Subscribed(Subscriber {
                    id: CentralId(id),
                    max_notify_len: mtu,
                }));
            }
            let _ = ch;
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(&self, _m: &CBPeripheralManager, central: &CBCentral, _c: &CBCharacteristic) {
            let id = central_id(central);
            let mut st = self.ivars().borrow_mut();
            st.subs.remove(&id);
            // 큐도 함께 버린다. 남겨두면 다시 구독했을 때 끊기기 전의 꼬리
            // 청크가 먼저 나가 한 프레임을 버리게 되고, 다시 오지 않을
            // central 의 큐가 계속 쌓인다.
            MainState::drop_queues_for(&mut st.queues, std::slice::from_ref(&id));
            let _ = st.events.send(PeripheralEvent::Unsubscribed(CentralId(id.clone())));
            // 인가는 연결 단위다 — 링크가 끊기면 그 central 의 인가도 즉시 지워야
            // 한다(BleBridge.forget_central 이 이 이벤트를 받아 처리한다).
            let _ = st.events.send(PeripheralEvent::Disconnected(CentralId(id)));
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_writes(&self, mgr: &CBPeripheralManager, requests: &NSArray<CBATTRequest>) {
            for i in 0..requests.count() {
                let req = requests.objectAtIndex(i);
                let central = unsafe { req.central() };
                let data = unsafe { req.value() }
                    .map(|d| d.to_vec())
                    .unwrap_or_default();
                let _ = self.ivars().borrow().events.send(PeripheralEvent::AuthWrite {
                    central: CentralId(central_id(&central)),
                    data,
                });
            }
            // 응답하지 않으면 iOS 쪽 write 가 타임아웃된다. 해석(코드 검증 등)은
            // 이 파일의 일이 아니므로, 여기서는 항상 성공으로 응답만 한다.
            if requests.count() > 0 {
                unsafe {
                    mgr.respondToRequest_withResult(&requests.objectAtIndex(0), CBATTError::Success)
                };
            }
        }

        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn ready(&self, _m: &CBPeripheralManager) {
            // 스펙 4.5: 포화 해제 신호. 여기서 재개하지 않으면 프레임이 영원히 미완성으로 남는다.
            {
                let mut st = self.ivars().borrow_mut();
                for q in st.queues.values_mut() {
                    q.on_ready();
                }
            }
            self.ivars().borrow_mut().pump_all(CharId::Snapshot.uuid());
        }
    }
);

fn central_id(c: &CBCentral) -> String {
    unsafe { c.identifier().UUIDString().to_string() }
}

impl Delegate {
    fn publish(&self, mgr: &CBPeripheralManager) {
        let snapshot_ch: Retained<CBMutableCharacteristic> = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &uuid(CharId::Snapshot.uuid()),
                CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Readable,
            )
        };
        // Auth 는 central 이 쓰고(Write) Mac 이 답한다(Notify) — HELLO/CODE/AUTH/PROOF 를
        // 받고, 논스·인가 결과를 그 central 에만 되돌려준다(notify_auth).
        let auth_ch: Retained<CBMutableCharacteristic> = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &uuid(CharId::Auth.uuid()),
                CBCharacteristicProperties::Write | CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let svc: Retained<CBMutableService> = unsafe {
            CBMutableService::initWithType_primary(CBMutableService::alloc(), &uuid(SERVICE_UUID), true)
        };
        let chars = NSArray::from_slice(&[&*snapshot_ch, &*auth_ch]);
        unsafe { svc.setCharacteristics(Some(&Retained::cast_unchecked(chars))) };
        {
            let mut st = self.ivars().borrow_mut();
            st.chars.insert(CharId::Snapshot.uuid(), snapshot_ch);
            st.chars.insert(CharId::Auth.uuid(), auth_ch);
        }
        // 전원 재순환(제어 센터 블루투스 off→on)이나 재개 때 동일 SERVICE_UUID 가 GATT DB 에
        // 중복 등록되면 iOS 가 낡은 서비스에 바인딩해 영구히 데이터가 흐르지 않는다.
        // 비어 있어도 무해하므로 무조건 먼저 비운다.
        unsafe { mgr.removeAllServices() };
        unsafe { mgr.addService(&svc) };
    }

    fn advertise(&self, mgr: &CBPeripheralManager) {
        let host = hostname_prefix();
        let name = NSString::from_str(&format!("AIM-{host}"));
        let uuids = NSArray::from_slice(&[&*uuid(SERVICE_UUID)]);
        let ad: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
            Retained::cast_unchecked(NSDictionary::from_slices(
                &[CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey],
                &[
                    &*Retained::cast_unchecked::<NSObject>(name),
                    &*Retained::cast_unchecked::<NSObject>(uuids),
                ],
            ))
        };
        unsafe { mgr.startAdvertising(Some(&ad)) };
    }
}

fn hostname_prefix() -> String {
    let h = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    h.trim().chars().take(8).collect()
}

pub struct MacPeripheral {
    app: AppHandle,
    events: UnboundedSender<PeripheralEvent>,
    /// 메인 스레드가 아닌 곳에서 조회하기 위한 구독자 사본
    subs_mirror: Mutex<Vec<Subscriber>>,
}

impl MacPeripheral {
    pub fn new(app: AppHandle, events: UnboundedSender<PeripheralEvent>) -> Self {
        Self {
            app,
            events,
            subs_mirror: Mutex::new(Vec::new()),
        }
    }

    /// 델리게이트가 보내는 구독 이벤트를 받아 사본을 갱신한다. lib.rs 의 이벤트 루프가 호출한다.
    pub fn apply_event(&self, ev: &PeripheralEvent) {
        let mut m = self.subs_mirror.lock().unwrap();
        match ev {
            PeripheralEvent::Subscribed(s) => {
                m.retain(|x| x.id != s.id);
                m.push(s.clone());
            }
            PeripheralEvent::Unsubscribed(id) => m.retain(|x| &x.id != id),
            PeripheralEvent::PoweredOff => m.clear(),
            _ => {}
        }
    }
}

impl BlePeripheral for MacPeripheral {
    fn start(&self) -> anyhow::Result<()> {
        // 이미 만들어진 델리게이트가 있으면 재사용한다. 매 on/off 사이클마다 새
        // Delegate+CBPeripheralManager+CBMutableService 를 만들면 이전 것들이 해제되지
        // 않고 계속 쌓이고, 동일한 SERVICE_UUID 가 GATT DB 에 중복 등록된다.
        // 재사용/생성 판정은 with_delegate_or 가 메인 스레드 안에서 한다(중복 생성 방지).
        //
        // 공유 의사 플래그는 클로저를 올리기 전에 세운다. start()/stop() 은 BleBridge 가
        // 직렬화해 부르므로 플래그 쓰기 순서 = 클로저 게시 순서 = 메인 스레드 실행 순서다.
        SHARING_WANTED.store(true, Ordering::SeqCst);
        let events = self.events.clone();
        with_delegate_or(
            &self.app,
            // 재사용 경로. stop() 이 서비스를 내렸으므로 서비스부터 다시 올린다.
            // 광고는 didAddService 콜백이 이어서 시작하므로 여기서 부르지 않는다 — 생성 경로와 동일하다.
            |d| {
                let mgr = d.ivars().borrow().manager.clone();
                let Some(mgr) = mgr else {
                    return;
                };
                // PoweredOn 이 아니면 didUpdateState 콜백이 다시 켜졌을 때 publish()가 처리한다.
                if unsafe { mgr.state() } == CBManagerState::PoweredOn {
                    d.publish(&mgr);
                }
            },
            // 생성 경로.
            move || {
                let state = MainState {
                    manager: None,
                    chars: HashMap::new(),
                    subs: HashMap::new(),
                    // 큐는 central 별이므로 미리 만들어둘 수 없다.
                    // `offer_frame_to` 가 그 central 의 첫 프레임에서 만든다.
                    queues: HashMap::new(),
                    events,
                };
                let d = Delegate::alloc().set_ivars(RefCell::new(state));
                let d: Retained<Delegate> = unsafe { msg_send![super(d), init] };
                let proto = ProtocolObject::from_ref(&*d);
                let mgr: Retained<CBPeripheralManager> = unsafe {
                    CBPeripheralManager::initWithDelegate_queue(
                        CBPeripheralManager::alloc(),
                        Some(proto),
                        None,
                    )
                };
                d.ivars().borrow_mut().manager = Some(mgr);
                let raw = Retained::into_raw(d) as usize; // 메인 스레드에서만 역참조한다
                *delegate_slot().lock().unwrap() = Some(raw);
            },
        )?;
        Ok(())
    }

    fn stop(&self) {
        // 여기서부터 블루투스 전원 재순환이 일어나도 델리게이트가 서비스를 되살리지 않는다.
        SHARING_WANTED.store(false, Ordering::SeqCst);
        self.subs_mirror.lock().unwrap().clear();
        with_delegate(&self.app, |d| {
            let mgr = d.ivars().borrow().manager.clone();
            let Some(mgr) = mgr else {
                return;
            };
            unsafe { mgr.stopAdvertising() };
            // CBPeripheralManager 에는 central 연결을 직접 끊는 API 가 없다. 서비스를 내리는 것이
            // 연결된 central 에게 "이 주변장치는 더 이상 없다"고 알리는 유일한 수단이고,
            // 그래야 실제 Unsubscribed 가 발생해 재개 시 재구독이 이뤄진다.
            unsafe { mgr.removeAllServices() };
            // 서비스를 내리면 구독 상태도 함께 무효다. subs 를 남겨두면 재개 후에도
            // didSubscribe 가 다시 오지 않아 subs_mirror 가 영원히 비어 미러가 멈춘다.
            let mut st = d.ivars().borrow_mut();
            st.subs.clear();
            // 큐도 같은 이유로 버린다. stop() 에 잘린 프레임이 current 에 started=true 로
            // 남으면 재개 후 낡은 꼬리 청크가 먼저 나가 한 프레임을 버리게 되고,
            // paused 가 true 로 남았다면 다음 isReadyToUpdateSubscribers 가 올 때까지
            // 큐 전체가 멈춘다. 이제 큐가 central 별이라 값만 갈아끼우는 대신
            // 통째로 비운다 — 재개 후 재구독한 central 에게 `offer_frame_to` 가
            // 새로 만들어준다.
            st.queues.clear();
        });
    }

    fn offer_frame_to(&self, ch: CharId, central: &CentralId, chunks: Vec<Vec<u8>>) {
        let uuid = ch.uuid();
        let id = central.0.clone();
        with_delegate(&self.app, move |d| {
            {
                let mut borrow = d.ivars().borrow_mut();
                // 두 필드를 따로 빌리려면 RefMut 를 한 번 벗겨야 한다.
                let st = &mut *borrow;
                MainState::enqueue(&mut st.queues, &st.subs, uuid, &id, chunks);
            }
            d.ivars().borrow_mut().pump(uuid, &id);
        });
    }

    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs_mirror.lock().unwrap().clone()
    }

    /// Auth 특성으로 그 central 하나에만 응답한다(`pump()` 와 달리 큐를 거치지
    /// 않는다 — 페어링 응답은 스트리밍이 아니라 요청 하나에 응답 하나다).
    /// Auth 특성의 GATT 등록과 `didReceiveWriteRequests:` 배선은 이미 끝났다
    /// (`publish()`, `did_receive_writes`). characteristic 이나 central 을
    /// 못 찾으면 조용히 아무 일도 하지 않는데 — 이는 미완성이 아니라, 구독
    /// 확정(`didSubscribeToCharacteristic`) 전에 응답이 도착하는 순서 문제일
    /// 수 있다(전체 브랜치 리뷰 I-5, 후속 과제로 기록됨). 지금은 그 경로에서도
    /// 최소한 `PeripheralEvent::Error` 를 보내는 것을 고려해야 한다.
    fn notify_auth(&self, central: &CentralId, payload: Vec<u8>) {
        let id = central.0.clone();
        with_delegate(&self.app, move |d| {
            let st = d.ivars().borrow();
            let (Some(mgr), Some(ch), Some((c, _))) = (
                st.manager.clone(),
                st.chars.get(CharId::Auth.uuid()).cloned(),
                st.subs.get(&id).cloned(),
            ) else {
                return;
            };
            let data = NSData::with_bytes(&payload);
            let target: &CBCentral = &c;
            let targets = NSArray::from_slice(&[target]);
            unsafe {
                mgr.updateValue_forCharacteristic_onSubscribedCentrals(&data, &ch, Some(&targets))
            };
        });
    }

    /// 인가를 잃은 central 의 큐를 버린다. 큐에 이미 들어간 청크는
    /// `on_snapshot` 을 다시 거치지 않고도 backpressure 해제 재개만으로
    /// 마저 나가므로, 여기서 버리지 않으면 방금 철회된 central 이 남은
    /// 청크를 계속 받는다.
    fn revoke_targets(&self, ids: &[CentralId]) {
        let ids: Vec<String> = ids.iter().map(|c| c.0.clone()).collect();
        with_delegate(&self.app, move |d| {
            MainState::drop_queues_for(&mut d.ivars().borrow_mut().queues, &ids);
        });
    }
}

/// 큐 키가 곧 전송 대상이라는 성질을 CoreBluetooth 없이 지킨다.
/// (예전 `targets_for_tests` 를 대체한다 — 인가 목록을 따로 들고 다니던
/// `authorized_targets` 가 사라지면서 그 필터링 자체가 없어졌다.)
#[cfg(test)]
mod queue_routing_tests {
    use super::{MainState, QueueKey};
    use crate::ble::send_queue::SendQueue;
    use std::collections::HashMap;

    fn queues(keys: &[(&'static str, &str)]) -> HashMap<QueueKey, SendQueue> {
        keys.iter()
            .map(|(u, id)| ((*u, id.to_string()), SendQueue::new()))
            .collect()
    }

    fn subs(ids: &[&str]) -> HashMap<String, ()> {
        ids.iter().map(|id| (id.to_string(), ())).collect()
    }

    /// 큐 생성 = 인가라는 불변식의 나머지 절반. 구독하지 않은 central 에게
    /// 프레임이 들어와도 큐가 생기면 안 된다 — 생기면 `pump_all` 이 그를
    /// 순회 대상에 넣는다. `enqueue` 의 `subs.contains_key` 가드를 지우면
    /// 이 테스트가 실패한다.
    #[test]
    fn no_queue_is_created_for_a_central_that_is_not_subscribed() {
        let mut q = queues(&[]);
        MainState::enqueue(&mut q, &subs(&[]), "SNAP", "GHOST", vec![vec![1]]);
        assert_eq!(
            MainState::queued_centrals(&q, "SNAP"),
            Vec::<String>::new(),
            "구독하지 않은 central 에게는 큐를 만들지 않는다"
        );
    }

    /// 떠난 central 의 큐가 되살아나지 않는다. `did_unsubscribe` 는 지났는데
    /// tokio 쪽 `subs_mirror` 가 아직 그를 들고 있는 창(窓)에서 실제로 일어난다.
    #[test]
    fn a_departed_centrals_queue_is_not_recreated() {
        let mut q = queues(&[("SNAP", "A")]);
        MainState::drop_queues_for(&mut q, &["A".to_string()]); // did_unsubscribe
        MainState::enqueue(&mut q, &subs(&[]), "SNAP", "A", vec![vec![1]]);
        assert_eq!(MainState::queued_centrals(&q, "SNAP"), Vec::<String>::new());
    }

    #[test]
    fn a_subscribed_central_gets_a_queue() {
        let mut q = queues(&[]);
        MainState::enqueue(&mut q, &subs(&["A"]), "SNAP", "A", vec![vec![1]]);
        assert_eq!(MainState::queued_centrals(&q, "SNAP"), vec!["A".to_string()]);
    }

    #[test]
    fn sends_to_nobody_when_no_queue_exists_yet() {
        // 구독만으로는 큐가 생기지 않는다 — `offer_frame_to` 가 인가된
        // central 에게 불려야 생긴다. 그 전에는 아무도 대상이 아니다(fail-closed).
        let q = queues(&[]);
        assert_eq!(MainState::queued_centrals(&q, "SNAP"), Vec::<String>::new());
    }

    #[test]
    fn lists_every_central_queued_for_that_characteristic() {
        let q = queues(&[("SNAP", "A"), ("SNAP", "B"), ("AUTH", "C")]);
        let mut got = MainState::queued_centrals(&q, "SNAP");
        got.sort();
        assert_eq!(
            got,
            vec!["A".to_string(), "B".to_string()],
            "다른 characteristic 의 큐가 섞이면 안 된다"
        );
    }

    #[test]
    fn drop_queues_for_removes_only_the_given_centrals() {
        let mut q = queues(&[("SNAP", "A"), ("SNAP", "B"), ("SNAP", "C")]);
        MainState::drop_queues_for(&mut q, &["B".to_string()]);
        let mut got = MainState::queued_centrals(&q, "SNAP");
        got.sort();
        assert_eq!(got, vec!["A".to_string(), "C".to_string()]);
    }

    /// 인가 철회는 그 central 의 **모든** characteristic 큐를 지워야 한다.
    #[test]
    fn drop_queues_for_removes_that_central_from_every_characteristic() {
        let mut q = queues(&[("SNAP", "A"), ("AUTH", "A"), ("SNAP", "B")]);
        MainState::drop_queues_for(&mut q, &["A".to_string()]);
        assert_eq!(MainState::queued_centrals(&q, "SNAP"), vec!["B".to_string()]);
        assert_eq!(MainState::queued_centrals(&q, "AUTH"), Vec::<String>::new());
    }

    #[test]
    fn drop_queues_for_is_a_noop_when_nothing_matches() {
        let mut q = queues(&[("SNAP", "A")]);
        MainState::drop_queues_for(&mut q, &["ZZZ".to_string()]);
        assert_eq!(MainState::queued_centrals(&q, "SNAP"), vec!["A".to_string()]);
    }
}
