//! LAN WebSocket 서버 (포트 4320, `GET /mirror`).
//!
//! `quota_proxy.rs` 의 `TcpListener::bind` + `axum::serve` 패턴을 그대로 따른다.
//! 딱 한 군데가 의도적으로 다르다: quota 프록시는 바인딩 실패를 `tracing::warn`
//! 으로만 남긴다. 그쪽은 개발자가 알아서 켜는 편의 기능이라 그래도 되지만, LAN
//! 공유는 사용자가 방금 토글로 켠 기능이다 — 이 앱은 로그 파일을 남기지 않으므로
//! 조용히 죽으면 "켰는데 아무 일도 안 일어난다"로만 보인다. 그래서 실패는
//! `ServerEvent::BindFailed` 로 올려 Devices 패널까지 닿게 한다.
//!
//! ## 연결 하나 = 세션 하나 = `CentralId` 하나
//! BLE 링크·iroh 연결과 같은 모델이다. id 는 연결 일련번호로만 만든다(`lan:0`,
//! `lan:1`, ...). 주소를 넣지 않는 이유는 두 가지다: 식별에 필요 없고(같은
//! 기기가 DHCP 로 주소를 바꿔도 세션은 그대로 하나여야 한다), 기기 목록에
//! 사용자의 내부망 주소를 노출할 이유도 없다.
//!
//! ## 왜 여기저기 상한이 붙어 있나
//! BLE 는 물리적으로 가까워야 하고 iroh 는 상대가 걸어와야 한다. **LAN 리스너는
//! 둘 다 아니다** — 같은 WiFi 의 아무나, 인증 전에, 아무 때나 두드릴 수 있다.
//! 그래서 인증보다 **앞에** 있는 자원은 전부 상한이 있어야 한다: 프레임 크기,
//! 동시 연결 수, 이벤트 큐, central 별 송신 큐, 그리고 **시간**. 상한을 넘긴
//! 쪽은 기다리게 하지 않고 **놓는다** — 여기서 기다리면 낯선 기기 하나가 서버
//! 전체를 세울 수 있다.
//!
//! 시간이 목록에 늦게 들어온 이유는 그것만 메모리로 드러나지 않기 때문이다.
//! 연결 하나는 여덟 자리 중 하나이고, 자리를 놓지 않는 피어는 아무것도 키우지
//! 않으면서 자리를 영원히 쓴다.
//!
//! **지금 시간 상한이 걸리는 곳은 정확히 둘이다.** 하나는 조용해진 피어
//! (`IDLE_TIMEOUT`, 90초). 다른 하나는 **송신 경로가 밀린** 피어 — 큐가 차서
//! 지워지면 하트비트를 보내지 못하고, 그 순간 연결을 놓는다.
//!
//! 뒤쪽은 "읽지 않으면 얼마 뒤에 끊긴다"가 **아니다.** 큐가 차려면 커널 송신
//! 버퍼가 먼저 차야 하고, 지금 이 전송이 내보내는 것은 30초에 한 번의 Ping
//! 뿐이라(초당 1바이트 미만) 그 버퍼를 채우는 데만 수 주가 걸린다. 즉 오늘
//! **읽지 않으면서 말은 하는 피어에게는 사실상 시한이 없다.** 스냅샷 푸시가
//! 붙는 Task 4 부터 이 경로가 초 단위로 짧아지고, 그때 이 문장은 사실이 된다.
//!
//! 그래서 **인증하지 않은 연결에 걸리는 절대 시한은 아직 없다.** 필요하고,
//! Task 4 의 몫이다. 이 문단이 "이미 있다"고 읽히면 다음 사람이 확인을
//! 건너뛴다 — 이 저장소는 그런 문서로 이미 세 번 값을 치렀다.

use crate::ble::peripheral::CentralId;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{Sender, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::sync::watch;

/// quota 프록시(4319) 바로 옆.
pub const PORT: u16 = 4320;

/// 받아들일 메시지·프레임 상한.
///
/// tungstenite 기본값은 메시지 64 MiB · 프레임 16 MiB 인데, 그 조립은 **인증보다
/// 먼저** 일어난다 — 같은 WiFi 의 아무나 64 MiB 짜리 조각난 메시지를 우리에게
/// 조립시켜 들고 있게 만들 수 있다는 뜻이다.
///
/// 예산은 프로토콜이 실제로 보내는 것에서 나왔다:
/// - **들어오는 것**: 페어링 인증 프레임(HELLO/CODE/AUTH/PROOF)뿐이다. 가장 큰
///   v2 프레임도 base64 공개키 + 토큰이라 1 KiB 를 넘지 않는다. 미러는 앱→기기
///   단방향이라 이보다 큰 것이 들어올 이유가 없다.
/// - **나가는 것**: 봉인된 스냅샷. 같은 스냅샷을 BLE 는 `framing::MAX_CHUNKS`
///   (255 조각)로 이미 45 KiB 언저리에서 자른다.
///
/// 64 KiB 는 그 위에 얹은 여유다. 늘리기 전에 위 두 줄이 아직 사실인지 보라 —
/// "조금만 풀자"는 곧 인증 전 메모리를 그만큼 내주는 것이다.
const MAX_FRAME_BYTES: usize = 64 * 1024;

/// 동시 연결 상한. 연결 하나가 태스크 둘 · 채널 둘 · 맵 항목 둘을 잡는다.
/// 책상 위 기기 하나(+ 재접속이 겹치는 한둘)면 충분한데, 상한이 없으면 같은
/// WiFi 의 아무나 그 묶음을 무한히 만들 수 있다.
pub const MAX_CONNECTIONS: usize = 8;

/// 이벤트 큐 길이. 연결 하나가 인증에 쓰는 프레임은 한 자릿수라 연결을
/// `MAX_CONNECTIONS` 까지 채워도 넉넉하다. 넘치는 쪽은 놓는다.
pub const EVENT_QUEUE: usize = 256;

/// central 별 송신 큐 길이. 스냅샷은 초당 하나꼴이므로 여덟 개가 밀렸다는 건
/// 상대가 8 초째 읽지 않는다는 뜻이다 — 폰이 백그라운드로 갔거나 기기가 죽었거나.
/// 흔한 일이고, 그때는 무한정 쌓느니 연결을 놓고 다시 붙게 하는 편이 낫다.
const SINK_QUEUE: usize = 8;

/// 하트비트 주기와 무응답 판정 시간.
///
/// **왜 필요한가.** BLE 는 OS 스택이 링크 끊김을 알려주고 iroh 는 QUIC 이
/// idle timeout 을 갖는다. TCP 는 둘 다 아니다 — 기기의 전원이 뽑히거나 WiFi 가
/// 사라지면 소켓은 **살아 있는 것처럼 보인다**(macOS 기본 keepalive 는 2시간).
/// 그동안 `Disconnected` 가 나가지 않으므로 "끊기면 인가가 즉시 사라진다"는
/// 불변식이 조용히 깨진다. CYD 는 전원이 뽑히는 게 일상인 기기다.
///
/// **왜 이 숫자인가.** 30초마다 Ping 을 보내고, 마지막으로 무언가 받은 지
/// 90초가 지나면 사라진 것으로 본다 — Ping 세 번을 연속으로 놓친 셈이다.
/// 한 번으로 끊지 않는 이유는 잠깐 바쁜 기기를 죽은 것으로 오인하지 않기
/// 위해서다(ESP32 의 느린 화면 갱신, 폰의 WiFi 절전, 순간적인 패킷 손실).
/// 반대로 늘리지 않는 이유는 이 시간 동안 인가가 실제 링크보다 오래 살기
/// 때문이다. 늦게 알아채는 비용은 기기 목록의 유령 한 줄이지만, 성급하게
/// 끊는 비용은 멀쩡히 쓰는 기기가 화면 앞에서 끊기는 것이다.
///
/// 살아 있는 기기는 Pong 을 자동으로 되돌려주고(WebSocket 규약), 그 Pong 도
/// "무언가 받았다"로 친다 — 즉 화면이 멈춰 있어도 스택만 살아 있으면 유지된다.
const PING_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// 하트비트 타이밍. 상수를 그대로 쓰지 않고 값으로 들고 다니는 이유는
/// 90초를 실제로 기다리지 않고도 "조용해진 피어를 정말 놓는가"를 테스트가
/// 확인할 수 있게 하기 위해서다 — `port` 를 인자로 받는 것과 같은 이유다.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub ping: Duration,
    pub idle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { ping: PING_INTERVAL, idle: IDLE_TIMEOUT }
    }
}

/// 이번 하트비트 틱에서 할 일. 연결 코드에서 떼어 둔 이유는 이 판단이
/// "얼마나 조용하면 죽은 것으로 보는가"라는 정책이기 때문이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heartbeat {
    /// 살아 있는지 물어본다.
    Ping,
    /// 대답이 너무 오래 없었다 — 사라진 것으로 보고 연결을 놓는다.
    Drop,
}

fn heartbeat(silent_for: Duration, timing: &Timing) -> Heartbeat {
    if silent_for >= timing.idle {
        Heartbeat::Drop
    } else {
        Heartbeat::Ping
    }
}

/// 연결 일련번호로 세션 id 를 만든다. 모듈 doc 의 "주소를 넣지 않는다" 규칙이
/// 사는 곳이라 연결 코드에서 떼어 두었다. **순수 함수다** — 같은 번호를 넣으면
/// 같은 id 가 나온다. 번호를 발급하는 것은 `next_central_id` 다.
pub fn central_id(serial: u64) -> CentralId {
    CentralId(format!("lan:{serial}"))
}

/// 세션 id 발급기. **프로세스 전체에서** 단조 증가한다 — 리스너(`AppState`)가
/// 아니라 모듈이 들고 있는 것이 핵심이다.
///
/// 리스너마다 0부터 다시 세면, 토글을 껐다 켠 뒤의 `lan:0` 과 그 전의 `lan:0` 이
/// 같은 이름이 된다. id 는 이제 공유 `PairingManager` 의 **인가 키**이므로 그
/// 겹침은 두 방향 모두 실제 사고다: 이전 세대의 늦은 `Disconnected(lan:0)` 이
/// 지금 붙어 있는 다른 기기의 인가를 지워버리거나(그 기기는 소켓이 살아 있어
/// 재연결하지 않으므로 화면이 빈 채로 멈춘다), 반대로 남아 있던
/// `authorized["lan:0"]` 을 **다른 기기가 물려받는다**. 뒤쪽은 인가가 피어를
/// 건너뛰는 것이다.
///
/// 단일 채널의 FIFO 는 발행 순서만 보장하지, 세대가 다른 태스크들의 스케줄
/// 순서까지 보장하지 않는다. 그래서 순서를 조율하는 대신 **같은 이름이 두 번
/// 나오지 않게** 만든다.
pub fn next_central_id() -> CentralId {
    static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);
    central_id(NEXT_SERIAL.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Connected(CentralId),
    Disconnected(CentralId),
    /// 인증 프레임(텍스트). 해석은 pairing 모듈이 한다.
    Frame { id: CentralId, text: String },
    /// 바인딩 실패. 모듈 doc 참고 — 조용히 warn 만 남기면 안 된다.
    ///
    /// **어느 리스너의 실패인지 함께 싣는다.** 이벤트는 큐에서 밀릴 수 있고,
    /// 그 사이 사용자가 토글을 껐다 켜면 이미 정상적으로 뜬 새 리스너가 낡은
    /// 실패 통지에 의해 내려간다(`LanBridge::apply_event` 참고).
    BindFailed { generation: u64, message: String },
}

/// 이 central 에게 보낼 것. 서버 태스크가 소유한 송신 채널로 전달된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Text(CentralId, Vec<u8>),
    Binary(CentralId, Vec<u8>),
    Close(CentralId),
}

/// 동시 연결 자리표. `MAX_CONNECTIONS` 를 넘으면 자리를 내주지 않는다.
#[derive(Default)]
struct Slots {
    live: AtomicUsize,
}

impl Slots {
    /// 자리를 하나 잡는다. 상한을 넘으면 `None` — 부르는 쪽은 업그레이드를 거절한다.
    fn acquire(self: &Arc<Self>) -> Option<Slot> {
        let mut live = self.live.load(Ordering::Relaxed);
        loop {
            if live >= MAX_CONNECTIONS {
                return None;
            }
            match self.live.compare_exchange_weak(
                live,
                live + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Slot(self.clone())),
                Err(actual) => live = actual,
            }
        }
    }

    fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }
}

/// 잡은 자리는 `Drop` 이 돌려준다. 핸들러 끝에서 직접 빼지 않는 이유는,
/// 업그레이드가 중간에 실패해 핸들러가 **아예 돌지 않는** 경우에도 자리가
/// 새면 안 되기 때문이다 — 그러면 실패한 시도 여덟 번으로 포트가 잠긴다.
struct Slot(Arc<Slots>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// central 별 송신 큐. 연결이 하나 생기면 하나 만들고, 그 연결이 끝나면 지운다.
/// 이 규칙이 어긋나면 끊긴 기기의 큐가 계속 쌓이거나(누수), 붙어 있는 기기에
/// 스냅샷이 나가지 않는다. 연결 코드 안에 두면 그 규칙만 따로 확인할 방법이
/// 없어서 별도 타입으로 뺐다.
#[derive(Default)]
struct Sinks(Mutex<HashMap<CentralId, Sender<Message>>>);

impl Sinks {
    fn insert(&self, id: CentralId, tx: Sender<Message>) {
        self.0.lock().unwrap().insert(id, tx);
    }

    fn remove(&self, id: &CentralId) {
        self.0.lock().unwrap().remove(id);
    }

    /// 잠금을 쥔 채로 보내지 않는다 — 송신은 await 를 탈 수 있고, 그 사이에
    /// 새 연결이 자기 큐를 등록하지 못하면 안 된다.
    fn get(&self, id: &CentralId) -> Option<Sender<Message>> {
        self.0.lock().unwrap().get(id).cloned()
    }

    /// 큐 개수를 세는 건 "연결 하나가 큐 하나" 규칙을 확인할 때뿐이다.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

struct AppState {
    events: Sender<ServerEvent>,
    sinks: Sinks,
    slots: Arc<Slots>,
    timing: Timing,
    /// 연결 핸들러가 각자 하나씩 들고 있다가, 토글이 꺼지면 스스로 빠져나온다.
    /// `handle` 의 주석 참고.
    shutdown: watch::Receiver<bool>,
}

pub struct ServerHandle {
    shutdown: watch::Sender<bool>,
    /// Task 4(스냅샷 푸시)가 쓴다. 여기만 상한이 없는 이유는 낯선 기기가 아니라
    /// 앱의 틱 루프(초당 하나)만 쓰기 때문이다 — 바깥에서 닿지 않는다.
    pub outbound: UnboundedSender<Outbound>,
    /// 이 리스너의 세대. `BindFailed` 가 누구 것인지 가리는 데 쓴다.
    pub generation: u64,
    /// 리스너 태스크. `stop()` 이 돌려주고, **다음 세대가 이것을 기다린 뒤에야
    /// 바인딩한다** — 자세한 이유는 `stop` 과 `spawn` 의 doc.
    task: JoinHandle<()>,
}

impl ServerHandle {
    /// 종료를 신호하고, **끝났는지 기다릴 수 있는 손잡이**를 돌려준다.
    ///
    /// 신호만 쏘고 반환하는 것은 그대로다(`set_enabled` 는 동기 함수라 여기서
    /// 기다릴 수 없다). 문제는 그다음이었다: 끄자마자 켜면 옛 리스너가 아직
    /// LISTEN 중인 소켓 위에 새 리스너가 바인딩을 시도해 `EADDRINUSE` 가 난다.
    /// 그러면 앱이 스스로 만든 실패를 "포트 4320 을 열지 못했습니다"라고 사용자
    /// 탓처럼 패널에 띄운다 — 사용자는 아무 포트도 쓰지 않고 있는데.
    ///
    /// 그래서 기다리는 쪽을 호출자에서 **새 리스너 태스크**로 옮겼다. 이 손잡이를
    /// 다음 `spawn` 에 넘기면, 새 태스크가 옛 태스크의 종료를 확인한 뒤에 bind 한다.
    #[must_use = "다음 spawn 에 넘겨야 즉시 재시작이 EADDRINUSE 를 만들지 않는다"]
    pub fn stop(self) -> JoinHandle<()> {
        // 수신자가 이미 사라졌어도(태스크가 먼저 끝난 경우) 문제되지 않는다.
        let _ = self.shutdown.send(true);
        self.task
    }
}

/// 옛 리스너가 끝나기를 기다리는 최대 시간. 넘기면 그냥 진행한다 — 그 경우
/// bind 가 실패하고 `BindFailed` 가 뜨는데, 이는 기다리지 않던 예전 동작과
/// 같으므로 더 나빠지지 않는다. 무한정 기다리면 반대로 "켰는데 아무 일도
/// 일어나지 않는" 훨씬 나쁜 상태가 된다.
const PREVIOUS_LISTENER_GRACE: Duration = Duration::from_secs(3);

/// 리스너를 띄운다. 운영에서 `port` 는 언제나 `PORT` 다 — 인자로 받는 이유는
/// 테스트가 서로의(그리고 개발 중인 앱의) 4320 을 밟지 않게 하기 위해서다.
/// 바인딩은 이 안에서 기다리지 않는다. 실패는 `ServerEvent::BindFailed` 로 온다.
///
/// `previous` 는 방금 `stop()` 한 리스너의 태스크다. 있으면 그것이 끝난 뒤에
/// bind 한다(`ServerHandle::stop` 의 doc).
pub fn spawn(
    port: u16,
    events: Sender<ServerEvent>,
    generation: u64,
    previous: Option<JoinHandle<()>>,
) -> ServerHandle {
    spawn_with(port, events, generation, previous, Timing::default())
}

fn spawn_with(
    port: u16,
    events: Sender<ServerEvent>,
    generation: u64,
    previous: Option<JoinHandle<()>>,
    timing: Timing,
) -> ServerHandle {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        events,
        sinks: Sinks::default(),
        slots: Arc::new(Slots::default()),
        timing,
        shutdown: shutdown_rx,
    });
    let task = tokio::spawn(run(state, port, out_rx, generation, previous));
    ServerHandle { shutdown: shutdown_tx, outbound: out_tx, generation, task }
}

/// 종료 신호를 기다린다. `send(true)` 뿐 아니라 **보내는 쪽이 사라진 경우**도
/// 종료로 본다 — `ServerHandle` 을 `stop()` 없이 그냥 떨어뜨렸는데 소켓이 계속
/// 열려 있으면 "껐는데 포트가 살아 있다"가 된다.
async fn wait_shutdown(mut rx: watch::Receiver<bool>) {
    while rx.changed().await.is_ok() {
        if *rx.borrow() {
            return;
        }
    }
}

async fn run(
    state: Arc<AppState>,
    port: u16,
    mut outbound: UnboundedReceiver<Outbound>,
    generation: u64,
    previous: Option<JoinHandle<()>>,
) {
    // 옛 리스너가 소켓을 놓기 전에 bind 하면 우리가 만든 EADDRINUSE 를 사용자에게
    // 보여주게 된다(`ServerHandle::stop` 의 doc).
    if let Some(prev) = previous {
        if tokio::time::timeout(PREVIOUS_LISTENER_GRACE, prev).await.is_err() {
            tracing::warn!(port, "이전 LAN 리스너가 제때 끝나지 않았다 — 그대로 진행한다");
        }
    }

    let events = state.events.clone();
    let shutdown = state.shutdown.clone();
    let app = Router::new()
        .route("/mirror", get(upgrade))
        .with_state(state.clone());

    // 0.0.0.0 이다. quota 프록시(127.0.0.1)와 달리 같은 WiFi 의 기기가 붙어야 한다.
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let _ = events
                .send(ServerEvent::BindFailed {
                    generation,
                    message: format!("포트 {port} 을(를) 열지 못했습니다: {e}"),
                })
                .await;
            return;
        }
    };
    tracing::info!(port, "LAN 미러 서버 시작 (GET /mirror)");

    // 송신 펌프 — central 별 큐로 넘긴다. 별도 태스크인 이유는 `axum::serve` 가
    // 이 태스크를 끝까지 점유하기 때문이다.
    let pump_state = state.clone();
    let pump = tokio::spawn(async move {
        while let Some(item) = outbound.recv().await {
            let (id, msg) = to_message(item);
            push_to_sink(&pump_state.sinks, &id, msg);
        }
    });

    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(wait_shutdown(shutdown))
        .await;
    pump.abort();
    tracing::info!(port, "LAN 미러 서버 종료");
}

/// 보낼 항목을 WebSocket 프레임으로 옮긴다. 순수 함수라 펌프 태스크와 떼어
/// 확인할 수 있다.
fn to_message(item: Outbound) -> (CentralId, Message) {
    match item {
        Outbound::Text(id, b) => (id, Message::Text(String::from_utf8_lossy(&b).into_owned())),
        Outbound::Binary(id, b) => (id, Message::Binary(b)),
        Outbound::Close(id) => (id, Message::Close(None)),
    }
}

/// 이 central 의 큐에 프레임을 넣는다. 큐가 밀려 있으면 **큐를 지워 연결을
/// 놓는다** — 상대가 읽지 않는데 계속 쌓으면 그 자체가 메모리 증가 레버다.
/// `false` 는 "이 central 에게 더 보낼 수 없다"(모르는 id 이거나 방금 놓았다).
fn push_to_sink(sinks: &Sinks, id: &CentralId, msg: Message) -> bool {
    let Some(s) = sinks.get(id) else {
        return false;
    };
    if s.try_send(msg).is_err() {
        tracing::warn!(id = %id.0, "LAN 송신 큐가 밀렸다 — 연결을 놓는다");
        sinks.remove(id);
        return false;
    }
    true
}

/// 인증 프레임을 이벤트 큐에 올린다. 가득 차 있으면 **올리지 않고** `false` 를
/// 돌려준다 — 부르는 쪽은 그 연결을 놓는다. 여기서 기다리면 프레임을 빠르게
/// 밀어 넣는 낯선 기기 하나가 서버 전체를 세울 수 있다.
fn offer_frame(events: &Sender<ServerEvent>, id: &CentralId, text: String) -> bool {
    events
        .try_send(ServerEvent::Frame { id: id.clone(), text })
        .is_ok()
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    // 자리를 먼저 잡는다. 못 잡으면 업그레이드 자체를 하지 않는다.
    //
    // 이 거절은 `last_error` 에 남기지 않는다 — 그 필드는 **사용자가 만든,
    // 사용자가 고칠 수 있는** 실패(포트 점유·권한 거부)를 위한 것이다. 낯선
    // 기기가 포트를 두드리는 건 사용자가 고칠 것이 없고, 거기에 오류를 실으면
    // 남이 우리 UI 에 글을 쓸 수 있게 된다.
    let Some(slot) = state.slots.acquire() else {
        tracing::warn!(max = MAX_CONNECTIONS, "LAN 동시 연결 상한 — 업그레이드 거절");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // 일련번호는 자리를 잡은 뒤에 뽑는다 — 거절된 시도가 번호를 먹지 않게.
    let id = next_central_id();
    let shutdown = state.shutdown.clone();
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| handle(socket, state, id, shutdown, slot))
}

async fn handle(
    socket: WebSocket,
    state: Arc<AppState>,
    id: CentralId,
    mut shutdown: watch::Receiver<bool>,
    // 이 연결이 살아 있는 동안 자리를 붙들고 있다가, 함수가 끝나면 돌려준다.
    _slot: Slot,
) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<Message>(SINK_QUEUE);
    state.sinks.insert(id.clone(), sink_tx);

    // 수명 이벤트(Connected/Disconnected)는 흘려보내면 안 된다 — Disconnected 를
    // 놓치면 인가가 실제 링크보다 오래 살아남는다. 그래서 프레임과 달리 여기서는
    // 기다린다. 기다리는 건 이 연결의 태스크뿐이고, 그런 태스크 수는
    // `MAX_CONNECTIONS` 로 이미 막혀 있다.
    if state.events.send(ServerEvent::Connected(id.clone())).await.is_err() {
        // 받는 쪽이 사라졌다 = 앱이 내려가는 중이다.
        state.sinks.remove(&id);
        return;
    }

    let mut writer = tokio::spawn(async move {
        while let Some(m) = sink_rx.recv().await {
            if ws_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    // 마지막으로 이 기기에게서 **무언가** 받은 시각. Pong 도 포함이다 — 화면이
    // 멈춰 있어도 스택이 살아 있으면 살아 있는 것으로 본다.
    let mut last_seen = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + state.timing.ping,
        state.timing.ping,
    );

    loop {
        tokio::select! {
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    last_seen = tokio::time::Instant::now();
                    if !offer_frame(&state.events, &id, text) {
                        tracing::warn!(id = %id.0, "LAN 이벤트 큐가 찼다 — 연결을 놓는다");
                        break;
                    }
                }
                // 클라이언트가 보내는 바이너리는 지금 쓰지 않는다 — 미러는 읽기 전용이다.
                // Pong·Ping 도 여기로 온다. 내용은 쓸 데가 없지만 **왔다는 사실**이
                // 하트비트의 전부다(인바운드 Ping 에 대한 Pong 은 라이브러리가 보낸다).
                Some(Ok(_)) => {
                    last_seen = tokio::time::Instant::now();
                }
                // 오류(상한 초과 포함)든 스트림 끝이든 이 연결은 끝났다.
                Some(Err(_)) | None => break,
            },
            // 조용히 사라진 기기를 알아채는 유일한 수단이다. TCP 는 전원이 뽑힌
            // 상대와 살아 있는 상대를 구분해 주지 않는다(`PING_INTERVAL` 의 doc).
            _ = ping.tick() => {
                match heartbeat(last_seen.elapsed(), &state.timing) {
                    Heartbeat::Drop => {
                        tracing::info!(id = %id.0, "LAN 피어가 조용하다 — 사라진 것으로 본다");
                        break;
                    }
                    // **보내지 못하면 그것으로 끝이다.** 큐가 지워졌다는 것은
                    // 상대가 읽지 않아 writer 가 `ws_tx.send().await` 에서 멈춰
                    // 있다는 뜻이고, 멈춘 writer 는 `sink_rx.recv()` 로 돌아오지
                    // 않으므로 큐를 지워도 스스로 끝나지 않는다 — 아래 writer
                    // 분기는 영영 깨어나지 않는다.
                    //
                    // 그동안 하트비트도 무력하다: 말은 계속 하는 피어라면
                    // `last_seen` 이 계속 갱신돼 `Drop` 판정이 나오지 않는다.
                    // 메모리는 전부 상한이 있어 늘지 않지만, **연결 자리 여덟 중
                    // 하나를 시간 제한 없이 붙들고 있게 된다.** 여덟이 그러면
                    // 정작 CYD 가 못 붙는다.
                    //
                    // 다만 **여기에 닿는 데 걸리는 시간은 지금 매우 길다** —
                    // 큐가 차려면 커널 송신 버퍼가 먼저 차야 하는데 이 전송이
                    // 내보내는 것은 30초에 한 번의 Ping 뿐이다. 스냅샷이 붙는
                    // Task 4 부터 초 단위가 된다(모듈 doc 의 같은 이야기).
                    // 그러니 이 분기는 "인증 없는 연결의 절대 시한"이 아니다.
                    Heartbeat::Ping => {
                        if !push_to_sink(&state.sinks, &id, Message::Ping(Vec::new())) {
                            tracing::info!(id = %id.0, "LAN 피어가 읽지 않는다 — 연결을 놓는다");
                            break;
                        }
                    }
                }
            },
            // 쓰기 쪽이 먼저 끝났다 = 소켓 쓰기가 실패했거나, writer 가 멈춰
            // 있지 않은 사이에 큐가 지워져 채널이 닫힌 것이다. 읽기만 계속
            // 붙들고 있을 이유가 없다.
            //
            // "상대가 읽지 않는 경우"는 여기가 아니다 — 그때 writer 는
            // `ws_tx.send().await` 에서 멈춰 있어 이 분기가 **깨어나지 않는다.**
            // 그것을 잡는 것은 위의 하트비트다.
            _ = &mut writer => break,
            // 토글을 끄면 리스너만 닫는 것으로는 부족하다. 미러 연결은 스스로
            // 끝나지 않으므로, 여기서 깨우지 않으면 axum 의 graceful shutdown 이
            // 영원히 기다리고 기기는 계속 붙어 있는 채로 남는다.
            _ = shutdown.changed() => break,
        }
    }

    writer.abort();
    state.sinks.remove(&id);
    // 링크가 끊기면 인가를 즉시 지워야 한다(BLE 의 `Disconnected` 와 같은 규칙).
    let _ = state.events.send(ServerEvent::Disconnected(id)).await;
}

/// 테스트에서 진짜 소켓을 다루는 최소한의 도구. `pairing::test_client` 를
/// 끌어올린 것과 같은 이유로 형제 모듈(`lan::mod` 의 테스트)에서도 보이게
/// 둔다 — 핸드셰이크와 프레이밍을 각자 베껴 두면 한 곳만 고치고 나머지를
/// 잊는다. WebSocket 클라이언트 크레이트를 dev-dep 으로 더하지 않는 이유는,
/// 여기서 확인하려는 것이 라이브러리의 프레이밍이 아니라 **우리 쪽 수명·상한·
/// 인증 판단**이기 때문이다.
#[cfg(test)]
pub(crate) mod test_socket {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// 지금 비어 있는 포트를 하나 골라 온다. 테스트가 4320 을 잡으면 서로,
    /// 그리고 개발 중인 앱과 충돌한다.
    pub(crate) async fn free_port() -> u16 {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        l.local_addr().unwrap().port()
    }

    pub(crate) async fn wait_until_listening(port: u16) {
        for _ in 0..200 {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("서버가 {port} 에서 뜨지 않았다");
    }

    /// 업그레이드를 시도하고 응답 머리를 그대로 돌려준다 — 거절(503)을 보는
    /// 테스트가 있으므로 여기서 성공을 단정하지 않는다.
    pub(crate) async fn try_handshake(port: u16) -> (TcpStream, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(
            b"GET /mirror HTTP/1.1\r\n\
              Host: localhost\r\n\
              Connection: Upgrade\r\n\
              Upgrade: websocket\r\n\
              Sec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = [0u8; 512];
        let n = s.read(&mut buf).await.unwrap();
        (s, String::from_utf8_lossy(&buf[..n]).to_string())
    }

    pub(crate) async fn handshake(port: u16) -> TcpStream {
        let (s, head) = try_handshake(port).await;
        assert!(head.starts_with("HTTP/1.1 101"), "업그레이드가 거절됐다: {head}");
        s
    }

    /// 수신 버퍼를 아주 작게 잡은 클라이언트. **읽지 않는 피어를 재현하는 데
    /// 필요하다** — 루프백의 기본 버퍼는 수 MB라, 상대가 한 바이트도 읽지 않아도
    /// 커널이 다 받아줘서 서버의 writer 가 멈추지 않는다. 실제 CYD 는 그 반대다
    /// (ESP32 의 lwIP 기본 수신 윈도는 6 KB 남짓이다). 그러니 이 값이 인위적인
    /// 것이 아니라, 루프백 쪽이 실제와 동떨어진 조건이다.
    pub(crate) async fn handshake_with_a_tiny_window(port: u16) -> TcpStream {
        let sock = tokio::net::TcpSocket::new_v4().unwrap();
        sock.set_recv_buffer_size(2 * 1024).unwrap();
        let mut s = sock
            .connect(([127, 0, 0, 1], port).into())
            .await
            .unwrap();
        s.write_all(
            b"GET /mirror HTTP/1.1\r\n\
              Host: localhost\r\n\
              Connection: Upgrade\r\n\
              Upgrade: websocket\r\n\
              Sec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = [0u8; 512];
        let n = s.read(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(head.starts_with("HTTP/1.1 101"), "업그레이드가 거절됐다: {head}");
        s
    }

    /// 클라이언트→서버 프레임은 마스킹해야 한다(RFC 6455).
    pub(crate) fn masked_text_frame(payload: &[u8]) -> Vec<u8> {
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        let mut out = vec![0x81]; // FIN + text
        assert!(payload.len() < 126, "테스트는 짧은 프레임만 보낸다");
        out.push(0x80 | payload.len() as u8);
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    /// 서버→클라이언트 텍스트 프레임 하나를 읽는다. 서버 프레임은 마스킹하지
    /// 않는다(RFC 6455). 길이는 7비트와 16비트 두 형태만 다룬다 — 인증 응답은
    /// 가장 긴 것도 200바이트 남짓이라 그 이상은 나올 수 없다.
    pub(crate) async fn read_text_frame(s: &mut TcpStream) -> Vec<u8> {
        let mut head = [0u8; 2];
        s.read_exact(&mut head).await.expect("프레임 머리를 읽지 못했다");
        assert_eq!(head[0], 0x81, "텍스트 프레임(FIN+opcode 1)이어야 한다");
        assert_eq!(head[1] & 0x80, 0, "서버 프레임은 마스킹하지 않는다");
        let len = match head[1] & 0x7f {
            126 => {
                let mut ext = [0u8; 2];
                s.read_exact(&mut ext).await.unwrap();
                u16::from_be_bytes(ext) as usize
            }
            n => n as usize,
        };
        let mut payload = vec![0u8; len];
        s.read_exact(&mut payload).await.expect("본문을 읽지 못했다");
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::test_socket::*;
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::mpsc::Receiver;

    fn events() -> (Sender<ServerEvent>, Receiver<ServerEvent>) {
        tokio::sync::mpsc::channel(EVENT_QUEUE)
    }

    /// 리스너 하나. 세대 1, 앞선 리스너 없음.
    fn listener(port: u16, tx: Sender<ServerEvent>) -> ServerHandle {
        spawn(port, tx, 1, None)
    }

    /// `Connected` 를 받아 **그 id 를 받아 쓴다.** 절대값(`lan:0`)을 가정하지
    /// 않는 것이 요점이다 — 일련번호는 프로세스 전역이라(I2) 앞선 테스트가 몇
    /// 개를 썼는지에 따라 달라진다. 절대값을 가정하는 테스트는 그 자체로
    /// "세대마다 0부터 다시 센다"는 옛 동작에 의존한다.
    async fn next_connected(rx: &mut Receiver<ServerEvent>) -> CentralId {
        match next_event(rx).await {
            ServerEvent::Connected(id) => id,
            other => panic!("Connected 를 기대했다: {other:?}"),
        }
    }

    #[test]
    fn port_is_next_to_the_quota_proxy() {
        assert_eq!(PORT, 4320, "quota 프록시 4319 바로 옆이다");
    }

    /// 연결 일련번호가 CentralId 를 만든다. IP 를 넣지 않는다 — 식별에
    /// 필요 없고 기기 목록에 노출될 이유도 없다.
    #[test]
    fn central_id_is_a_serial_not_an_address() {
        assert_eq!(central_id(0).0, "lan:0");
        assert_eq!(central_id(7).0, "lan:7");
    }

    /// `central_id` 는 순수 함수다 — 같은 번호면 같은 id.
    ///
    /// (예전 이름은 `serials_do_not_repeat` 이었는데, `central_id(1) != central_id(2)`
    /// 는 발급기에 대해 **아무것도 증명하지 않는** 항등식이었다. 실제로 반복하지
    /// 않는지는 아래 `the_id_allocator_never_repeats` 가 본다.)
    #[test]
    fn central_id_is_pure() {
        assert_eq!(central_id(7), central_id(7));
        assert_ne!(central_id(1), central_id(2));
    }

    /// 발급기는 같은 id 를 두 번 내주지 않는다. 리스너를 몇 번 껐다 켜든
    /// 마찬가지다 — 발급기가 리스너가 아니라 모듈에 붙어 있기 때문이다.
    /// id 가 겹치면 인가가 피어 사이를 건너뛴다(`next_central_id` 의 doc).
    #[test]
    fn the_id_allocator_never_repeats() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(next_central_id()), "발급기가 같은 id 를 두 번 냈다");
        }
    }

    #[test]
    fn text_outbound_becomes_a_text_frame() {
        let (id, msg) = to_message(Outbound::Text(central_id(3), b"{\"a\":1}".to_vec()));
        assert_eq!(id, central_id(3));
        assert_eq!(msg, Message::Text("{\"a\":1}".to_string()));
    }

    #[test]
    fn binary_outbound_stays_binary() {
        // 봉인된 스냅샷은 UTF-8 이 아니다. 텍스트로 옮기면 손실된다.
        let sealed = vec![0x00, 0xff, 0xfe];
        let (_, msg) = to_message(Outbound::Binary(central_id(0), sealed.clone()));
        assert_eq!(msg, Message::Binary(sealed));
    }

    #[test]
    fn close_outbound_becomes_a_close_frame() {
        let (_, msg) = to_message(Outbound::Close(central_id(0)));
        assert_eq!(msg, Message::Close(None));
    }

    /// 큐는 연결이 생길 때 만들어지고 끝날 때 사라진다. 이 규칙이 깨지면
    /// 끊긴 기기의 큐가 남아 계속 쌓인다.
    #[test]
    fn a_sink_lives_exactly_as_long_as_its_connection() {
        let sinks = Sinks::default();
        let (tx, _rx) = tokio::sync::mpsc::channel(SINK_QUEUE);
        let id = central_id(0);

        assert_eq!(sinks.len(), 0);
        sinks.insert(id.clone(), tx);
        assert_eq!(sinks.len(), 1);
        assert!(sinks.get(&id).is_some());

        sinks.remove(&id);
        assert_eq!(sinks.len(), 0);
        assert!(sinks.get(&id).is_none(), "끊긴 연결의 큐는 남아 있으면 안 된다");
    }

    /// 모르는 central 에게 보내려 해도 큐가 생기지 않는다 — 펌프가
    /// `get` 만 쓰기 때문이다(`insert` 는 연결 핸들러만 한다).
    #[test]
    fn sending_to_an_unknown_central_creates_nothing() {
        let sinks = Sinks::default();
        assert!(!push_to_sink(&sinks, &central_id(9), Message::Close(None)));
        assert_eq!(sinks.len(), 0);
    }

    // --- 상한 (인증 전에 낯선 기기가 닿는 자원들) ---

    /// 상대가 읽지 않으면 큐가 찬다. 그때 계속 쌓는 대신 연결을 놓아야 한다 —
    /// 폰이 백그라운드로 가는 건 공격이 아니라 흔한 일이고, 어느 쪽이든
    /// 무한정 쌓는 건 답이 아니다.
    #[test]
    fn a_backed_up_sink_is_dropped_rather_than_grown() {
        let sinks = Sinks::default();
        // 아무도 읽지 않는 큐. 실제 연결에서 writer 태스크가 멈춘 상태와 같다.
        let (tx, _rx) = tokio::sync::mpsc::channel(SINK_QUEUE);
        let id = central_id(0);
        sinks.insert(id.clone(), tx);

        for i in 0..SINK_QUEUE {
            assert!(
                push_to_sink(&sinks, &id, Message::Binary(vec![0])),
                "{i}번째까지는 들어가야 한다"
            );
        }

        assert!(
            !push_to_sink(&sinks, &id, Message::Binary(vec![0])),
            "큐가 찼으면 더 넣지 않는다"
        );
        assert!(sinks.get(&id).is_none(), "밀린 연결은 놓아야 한다");
        assert_eq!(sinks.len(), 0);
    }

    /// 프레임 이벤트는 큐가 차면 기다리지 않고 포기한다. 기다리면 프레임을
    /// 빠르게 밀어 넣는 기기 하나가 서버를 세울 수 있다.
    #[test]
    fn a_full_event_queue_gives_up_instead_of_blocking() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ServerEvent>(2);
        let id = central_id(0);

        assert!(offer_frame(&tx, &id, "AUTH:1".into()));
        assert!(offer_frame(&tx, &id, "AUTH:2".into()));
        assert!(
            !offer_frame(&tx, &id, "AUTH:3".into()),
            "큐가 찼으면 false 를 돌려 연결을 놓게 해야 한다"
        );
    }

    /// 받는 쪽이 사라져도 마찬가지다.
    #[test]
    fn a_closed_event_queue_gives_up_too() {
        let (tx, rx) = tokio::sync::mpsc::channel::<ServerEvent>(2);
        drop(rx);
        assert!(!offer_frame(&tx, &central_id(0), "AUTH:1".into()));
    }

    #[test]
    fn slots_stop_at_the_cap() {
        let slots = Arc::new(Slots::default());
        let held: Vec<Slot> = (0..MAX_CONNECTIONS)
            .map(|i| slots.acquire().unwrap_or_else(|| panic!("{i}번째 자리는 있어야 한다")))
            .collect();

        assert_eq!(slots.live(), MAX_CONNECTIONS);
        assert!(slots.acquire().is_none(), "상한을 넘으면 자리를 주지 않는다");
        drop(held);
    }

    /// 자리는 `Drop` 이 돌려준다. 핸들러가 아예 돌지 않은 경우(업그레이드
    /// 실패)에도 자리가 새면 안 되기 때문에 명시적 해제로 두지 않았다.
    #[test]
    fn a_released_slot_is_reusable() {
        let slots = Arc::new(Slots::default());
        let mut held: Vec<Slot> = (0..MAX_CONNECTIONS).map(|_| slots.acquire().unwrap()).collect();
        assert!(slots.acquire().is_none());

        held.pop();
        assert_eq!(slots.live(), MAX_CONNECTIONS - 1);
        assert!(slots.acquire().is_some(), "자리가 비었으면 다시 받아야 한다");
    }

    // --- 실제 소켓 (도구는 `test_socket` 에 있다) ---

    /// 토글을 끄면 플래그만 내려가는 게 아니라 소켓이 실제로 닫혀야 한다.
    /// "다시 바인딩할 수 있다"가 그것을 증명하는 유일하게 확실한 방법이다.
    #[tokio::test]
    async fn stopping_actually_releases_the_socket() {
        let port = free_port().await;
        let (tx, _rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let _ = handle.stop();

        for _ in 0..200 {
            if tokio::net::TcpListener::bind(("0.0.0.0", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("stop() 뒤에도 {port} 가 열려 있다");
    }

    /// `stop()` 을 부르지 않고 핸들만 떨어뜨려도 마찬가지다.
    #[tokio::test]
    async fn dropping_the_handle_also_releases_the_socket() {
        let port = free_port().await;
        let (tx, _rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        drop(handle);

        for _ in 0..200 {
            if tokio::net::TcpListener::bind(("0.0.0.0", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("핸들을 떨어뜨린 뒤에도 {port} 가 열려 있다");
    }

    /// 이미 잡혀 있는 포트로 뜨면 실패가 이벤트로 올라와야 한다. quota
    /// 프록시처럼 조용히 warn 만 남기면 사용자는 알 길이 없다.
    #[tokio::test]
    async fn a_taken_port_reports_bind_failed() {
        let occupier = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
        let port = occupier.local_addr().unwrap().port();

        let (tx, mut rx) = events();
        let _handle = listener(port, tx);

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("BindFailed 가 오지 않았다")
            .unwrap();
        match ev {
            ServerEvent::BindFailed { generation, message } => {
                assert_eq!(generation, 1, "어느 리스너의 실패인지 말해야 한다");
                assert!(
                    message.contains(&port.to_string()),
                    "어느 포트인지 말해야 한다: {message}"
                );
            }
            other => panic!("BindFailed 를 기대했다: {other:?}"),
        }
    }

    /// 헤더에 `declared` 바이트를 적어 놓고 본문은 보내지 않는다. 상한을 헤더
    /// 단계에서 보는지가 요점이다 — 본문을 다 받은 뒤에 재면 이미 늦다.
    fn oversized_frame_header(declared: u64) -> Vec<u8> {
        let mut out = vec![0x81, 0x80 | 127];
        out.extend_from_slice(&declared.to_be_bytes());
        out.extend_from_slice(&[0x37, 0xfa, 0x21, 0x3d]); // mask key
        out
    }

    async fn next_event(rx: &mut Receiver<ServerEvent>) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("이벤트가 오지 않았다")
            .unwrap()
    }

    /// 연결 하나가 세션 하나다 — 붙는 순간 `Connected`, 끊기는 순간
    /// `Disconnected` 가 같은 id 로 나와야 한다. 뒤쪽이 빠지면 인가가 링크보다
    /// 오래 살아남아 기기 목록이 계속 거짓말한다.
    #[tokio::test]
    async fn one_connection_is_one_session() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        drop(sock);
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(id));

        let _ = handle.stop();
    }

    /// 두 번째 연결은 다른 id 를 받는다. 같은 기기가 다시 붙어도 새 세션이다 —
    /// 예전 세션의 인가를 물려받으면 안 된다.
    #[tokio::test]
    async fn a_second_connection_gets_a_new_session() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let first = handshake(port).await;
        let first_id = next_connected(&mut rx).await;
        drop(first);
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(first_id.clone()));

        let second = handshake(port).await;
        let second_id = next_connected(&mut rx).await;
        assert_ne!(first_id, second_id, "다시 붙으면 새 세션이다");

        drop(second);
        let _ = handle.stop();
    }

    /// 토글을 끄면 붙어 있던 기기도 정리돼야 한다 — 리스너만 닫고 연결을
    /// 놔두면 그 기기는 계속 인가된 채로 남는다.
    #[tokio::test]
    async fn stopping_disconnects_live_connections() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let _sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        let _ = handle.stop();
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(id));
    }

    /// 정상 크기의 텍스트 프레임은 그대로 올라온다 — 상한이 진짜 트래픽까지
    /// 막아버리면 안 되므로 거절 테스트와 짝으로 둔다.
    #[tokio::test]
    async fn a_normal_text_frame_arrives_as_an_event() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        sock.write_all(&masked_text_frame(b"HELLO:1")).await.unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Frame { id, text: "HELLO:1".to_string() }
        );

        drop(sock);
        let _ = handle.stop();
    }

    /// 상한을 넘겨 선언한 프레임은 조립하지 않고 연결을 끊어야 한다. 인증도
    /// 하기 전에 낯선 기기가 64 MiB 를 우리에게 들고 있게 만들 수 있으면 안 된다.
    #[tokio::test]
    async fn an_oversized_frame_drops_the_connection_instead_of_buffering_it() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        // 상한의 두 배를 선언한다. 본문은 한 바이트도 보내지 않는다.
        sock.write_all(&oversized_frame_header(2 * MAX_FRAME_BYTES as u64)).await.unwrap();

        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Disconnected(id),
            "상한을 넘는 프레임은 본문을 기다리지 않고 끊어야 한다"
        );

        let _ = handle.stop();
    }

    /// 동시 연결 상한을 넘는 피어는 업그레이드부터 거절한다.
    #[tokio::test]
    async fn the_connection_cap_refuses_extra_peers() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let mut live = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..MAX_CONNECTIONS {
            live.push(handshake(port).await);
            assert!(ids.insert(next_connected(&mut rx).await), "id 가 겹쳤다");
        }

        let (_refused, head) = try_handshake(port).await;
        assert!(
            head.starts_with("HTTP/1.1 503"),
            "상한을 넘으면 업그레이드하지 않아야 한다: {head}"
        );

        // 자리를 하나 비우면 다시 받아야 한다 — 상한이 영구 잠금이 되면 안 된다.
        live.pop();
        assert!(matches!(next_event(&mut rx).await, ServerEvent::Disconnected(_)));

        for _ in 0..200 {
            let (s, head) = try_handshake(port).await;
            if head.starts_with("HTTP/1.1 101") {
                drop(s);
                let _ = handle.stop();
                return;
            }
            drop(s);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("자리가 비었는데도 계속 거절한다");
    }

    // --- 세대 (I2/I3/I4) ---

    /// 리스너를 껐다 켜도 id 는 이어진다. 세대마다 0부터 다시 세면 이전 세대의
    /// 늦은 `Disconnected(lan:0)` 이 지금 붙어 있는 다른 기기의 인가를 지운다.
    #[tokio::test]
    async fn ids_do_not_restart_with_the_listener() {
        let port = free_port().await;
        let (tx, mut rx) = events();

        let first = spawn(port, tx.clone(), 1, None);
        wait_until_listening(port).await;
        let sock = handshake(port).await;
        let old_id = next_connected(&mut rx).await;
        drop(sock);
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(old_id.clone()));

        let second = spawn(port, tx, 2, Some(first.stop()));
        wait_until_listening(port).await;
        let sock = handshake(port).await;
        let new_id = next_connected(&mut rx).await;

        assert_ne!(old_id, new_id, "리스너가 새로 떠도 예전 세션 이름을 재사용하면 안 된다");

        drop(sock);
        let _ = second.stop();
    }

    /// 끄자마자 켜도 `EADDRINUSE` 가 나면 안 된다. 그건 앱이 스스로 만든
    /// 실패인데 패널에는 "포트를 열지 못했습니다"로 떠서 사용자 탓처럼 보인다.
    /// 새 리스너가 옛 태스크의 종료를 기다린 뒤 bind 하므로 나지 않아야 한다.
    #[tokio::test]
    async fn an_immediate_restart_does_not_collide_with_itself() {
        let port = free_port().await;
        let (tx, mut rx) = events();

        let first = spawn(port, tx.clone(), 1, None);
        wait_until_listening(port).await;

        // 사이에 await 가 없다 — 동기 `set_enabled(false); set_enabled(true);` 와 같다.
        let previous = first.stop();
        let second = spawn(port, tx, 2, Some(previous));

        wait_until_listening(port).await;
        let sock = handshake(port).await;
        let _ = next_connected(&mut rx).await;
        assert!(rx.try_recv().is_err(), "재시작만으로 이벤트가 더 나오면 안 된다");

        drop(sock);
        let _ = second.stop();
    }

    /// 낡은 세대의 bind 실패가 지금 리스너를 죽이면 안 된다. 여기서는 서버 쪽
    /// 절반(실패에 세대가 실려 나가는가)만 본다 — 무시 판단은 `LanBridge` 다.
    #[tokio::test]
    async fn a_bind_failure_says_which_listener_it_was() {
        let occupier = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
        let port = occupier.local_addr().unwrap().port();

        let (tx, mut rx) = events();
        let _handle = spawn(port, tx, 7, None);

        match next_event(&mut rx).await {
            ServerEvent::BindFailed { generation, .. } => assert_eq!(generation, 7),
            other => panic!("BindFailed 를 기대했다: {other:?}"),
        }
    }

    // --- 하트비트 (I5) ---

    /// 판정 자체는 순수 함수다. 90초를 기다리지 않고 정책만 확인한다.
    #[test]
    fn a_peer_is_given_three_missed_pings_before_we_give_up() {
        let t = Timing::default();
        assert_eq!(heartbeat(Duration::from_secs(0), &t), Heartbeat::Ping);
        assert_eq!(heartbeat(t.ping, &t), Heartbeat::Ping, "한 번 놓친 것으로는 끊지 않는다");
        assert_eq!(heartbeat(t.ping * 2, &t), Heartbeat::Ping);
        assert_eq!(heartbeat(t.idle, &t), Heartbeat::Drop);
        assert_eq!(heartbeat(t.idle + t.ping, &t), Heartbeat::Drop);
    }

    /// 상수 사이의 관계를 못박는다. idle 이 ping 보다 짧으면 살아 있는 기기가
    /// 대답할 기회조차 없이 끊긴다.
    #[test]
    fn the_idle_budget_covers_several_pings() {
        assert!(
            IDLE_TIMEOUT >= PING_INTERVAL * 3,
            "잠깐 바쁜 기기를 죽은 것으로 오인하지 않으려면 여유가 있어야 한다"
        );
    }

    /// 전원이 뽑힌 기기는 FIN 도 RST 도 보내지 않는다 — 소켓은 살아 있는
    /// 것처럼 보인다. 그래도 `Disconnected` 가 나와야 인가가 링크와 함께
    /// 사라진다. (실제 90초 대신 짧은 타이밍으로 같은 경로를 탄다.)
    #[tokio::test]
    async fn a_silent_peer_is_eventually_dropped() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let timing = Timing {
            ping: Duration::from_millis(60),
            idle: Duration::from_millis(200),
        };
        let handle = spawn_with(port, tx, 1, None, timing);
        wait_until_listening(port).await;

        // 붙기만 하고 한 마디도 하지 않는다. 소켓은 계속 열려 있다.
        let _sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Disconnected(id),
            "조용히 사라진 기기를 알아채지 못하면 인가가 링크보다 오래 산다"
        );

        let _ = handle.stop();
    }

    /// **말은 하지만 읽지 않는 피어**에게도 시간 상한이 있어야 한다.
    ///
    /// 이 피어에게는 다른 어떤 상한도 걸리지 않는다: `last_seen` 이 계속
    /// 갱신되니 무응답 판정이 안 나오고, 프레임을 소비자 속도에 맞추면 이벤트
    /// 큐도 안 넘치며, writer 는 `ws_tx.send().await` 에 멈춰 있어 큐를 지워도
    /// 스스로 끝나지 않는다. 메모리는 늘지 않지만 **연결 자리 여덟 중 하나를
    /// 영원히** 붙든다.
    ///
    /// 재현: 수신 윈도가 좁은 클라이언트가 한 바이트도 읽지 않는 동안 서버가
    /// 큰 프레임을 밀어 넣는다 → writer 가 `ws_tx.send().await` 에서 멈춘다 →
    /// central 별 큐가 차서 지워진다 → 다음 하트비트가 보내지 못하고, 그때
    /// 연결을 놓아야 한다. `idle` 은 넉넉히 잡아 **무응답 경로가 아니라 이
    /// 경로로** 끝난다는 것을 분명히 한다.
    #[tokio::test]
    async fn a_peer_that_never_reads_does_not_hold_its_slot_forever() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn_with(
            port,
            tx,
            1,
            None,
            Timing { ping: Duration::from_millis(40), idle: Duration::from_secs(30) },
        );
        wait_until_listening(port).await;

        // 붙기만 하고 한 바이트도 읽지 않는다.
        let _sock = handshake_with_a_tiny_window(port).await;
        let id = next_connected(&mut rx).await;

        // 좁은 윈도를 넘기고도 남을 만큼 밀어 넣는다. 큐(8)를 채우고 writer 를
        // 멈추게 하는 것이 목적이지, 전부 나가는 것이 목적이 아니다.
        for _ in 0..64 {
            let _ = handle
                .outbound
                .send(Outbound::Binary(id.clone(), vec![0u8; MAX_FRAME_BYTES]));
        }

        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Disconnected(id),
            "읽지 않는 피어가 자리를 무한정 붙들면 안 된다"
        );

        let _ = handle.stop();
    }

    /// 반대쪽도 확인한다 — 말을 계속 하는 기기를 끊으면 안 된다. 잠깐 바쁜
    /// 기기를 죽은 것으로 오인하는 것이 이 기능의 유일한 오작동 방향이다.
    #[tokio::test]
    async fn a_talking_peer_is_kept() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let timing = Timing {
            ping: Duration::from_millis(40),
            idle: Duration::from_millis(150),
        };
        let handle = spawn_with(port, tx, 1, None, timing);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        // idle 예산의 네 배가 넘는 시간 동안 주기적으로 말한다.
        for _ in 0..12 {
            sock.write_all(&masked_text_frame(b"HELLO")).await.unwrap();
            assert_eq!(
                next_event(&mut rx).await,
                ServerEvent::Frame { id: id.clone(), text: "HELLO".to_string() },
                "말하는 기기가 끊겼다"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        drop(sock);
        let _ = handle.stop();
    }
}
