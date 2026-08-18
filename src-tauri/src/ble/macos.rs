//! CBPeripheralManager 직접 구현 (스펙 3.2). 0단계 스파이크에서 검증한 방식이다.
//!
//! 스레드 규약: CoreBluetooth 호출과 델리게이트 콜백을 모두 **메인 스레드**에서 처리한다.
//! CBPeripheralManager 를 queue=None 으로 만들면 콜백이 메인 큐로 오고, Tauri 가 이미
//! 메인 런루프를 돌리고 있으므로 별도 런루프가 필요 없다.
//! SendQueue 도 메인 스레드가 소유해 updateValue 의 bool 반환을 스레드 왕복 없이 처리한다.
//! tokio 쪽에서는 `offer_frame` 으로 프레임만 던지고(fire-and-forget) 즉시 돌아온다.
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

/// 슬롯에 델리게이트가 있으면 메인 스레드에서 `f` 를 실행한다. 원시 포인터를
/// `&Delegate` 로 되돌리는 유일한 지점 — 이 함수 밖에서는 절대 역참조하지 않는다.
fn with_delegate(app: &AppHandle, f: impl FnOnce(&Delegate) + Send + 'static) {
    let _ = app.run_on_main_thread(move || {
        let Some(raw) = *delegate_slot().lock().unwrap() else {
            return;
        };
        // 메인 스레드에서만 실행되는 클로저 안이므로 역참조가 안전하다.
        f(unsafe { &*(raw as *const Delegate) });
    });
}

/// 메인 스레드에서만 접근하는 상태.
struct MainState {
    manager: Option<Retained<CBPeripheralManager>>,
    chars: HashMap<&'static str, Retained<CBMutableCharacteristic>>,
    subs: HashMap<String, (Retained<CBCentral>, usize)>,
    queues: HashMap<&'static str, SendQueue>,
    events: UnboundedSender<PeripheralEvent>,
}

impl MainState {
    /// 큐에 쌓인 청크를 가능한 만큼 내보낸다.
    fn pump(&mut self, ch_uuid: &'static str) {
        let (Some(mgr), Some(ch)) = (self.manager.clone(), self.chars.get(ch_uuid).cloned())
        else {
            return;
        };
        let centrals: Vec<Retained<CBCentral>> =
            self.subs.values().map(|(c, _)| c.clone()).collect();
        if centrals.is_empty() {
            return;
        }
        let refs: Vec<&CBCentral> = centrals.iter().map(|c| &**c).collect();
        let targets = NSArray::from_slice(&refs);
        let Some(q) = self.queues.get_mut(ch_uuid) else {
            return;
        };
        q.pump(|chunk| {
            let data = NSData::with_bytes(chunk);
            // onSubscribedCentrals 를 명시해 인가 대상만 지정할 수 있게 해둔다(3단계 페어링 대비).
            unsafe {
                mgr.updateValue_forCharacteristic_onSubscribedCentrals(
                    &data,
                    &ch,
                    Some(&targets),
                )
            }
        });
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
            if powered {
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
            let _ = st.events.send(PeripheralEvent::Unsubscribed(CentralId(id)));
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
            self.ivars().borrow_mut().pump(CharId::Snapshot.uuid());
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
        let svc: Retained<CBMutableService> = unsafe {
            CBMutableService::initWithType_primary(CBMutableService::alloc(), &uuid(SERVICE_UUID), true)
        };
        let chars = NSArray::from_slice(&[&*snapshot_ch]);
        unsafe { svc.setCharacteristics(Some(&Retained::cast_unchecked(chars))) };
        self.ivars()
            .borrow_mut()
            .chars
            .insert(CharId::Snapshot.uuid(), snapshot_ch);
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
        // 않고 계속 쌓이고, 동일한 SERVICE_UUID 가 GATT DB 에 중복 등록된다. stop() 은
        // 광고만 멈췄을 뿐이므로, 재개할 때도 광고만 다시 시작하면 된다.
        if delegate_slot().lock().unwrap().is_some() {
            with_delegate(&self.app, |d| {
                let Some(mgr) = d.ivars().borrow().manager.clone() else {
                    return;
                };
                // PoweredOn 이 아니면 didUpdateState 콜백이 다시 켜졌을 때 publish()가 처리한다.
                if unsafe { mgr.state() } == CBManagerState::PoweredOn {
                    d.advertise(&mgr);
                }
            });
            return Ok(());
        }

        let events = self.events.clone();
        self.app.run_on_main_thread(move || {
            let state = MainState {
                manager: None,
                chars: HashMap::new(),
                subs: HashMap::new(),
                queues: HashMap::from([(CharId::Snapshot.uuid(), SendQueue::new())]),
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
        })?;
        Ok(())
    }

    fn stop(&self) {
        self.subs_mirror.lock().unwrap().clear();
        with_delegate(&self.app, |d| {
            if let Some(mgr) = d.ivars().borrow().manager.clone() {
                unsafe { mgr.stopAdvertising() };
            }
        });
    }

    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>) {
        let uuid = ch.uuid();
        with_delegate(&self.app, move |d| {
            {
                let mut st = d.ivars().borrow_mut();
                if let Some(q) = st.queues.get_mut(uuid) {
                    q.offer(chunks);
                }
            }
            d.ivars().borrow_mut().pump(uuid);
        });
    }

    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs_mirror.lock().unwrap().clone()
    }
}
