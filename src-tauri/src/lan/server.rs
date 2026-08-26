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
//! 동시 연결 수, 이벤트 큐, central 별 송신 큐. 상한을 넘긴 쪽은 기다리게 하지
//! 않고 **놓는다** — 여기서 기다리면 낯선 기기 하나가 서버 전체를 세울 수 있다.

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
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{Sender, UnboundedReceiver, UnboundedSender};
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

/// 연결 일련번호로 세션 id 를 만든다. 모듈 doc 의 "주소를 넣지 않는다" 규칙이
/// 사는 곳이라 연결 코드에서 떼어 두었다.
pub fn central_id(serial: u64) -> CentralId {
    CentralId(format!("lan:{serial}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Connected(CentralId),
    Disconnected(CentralId),
    /// 인증 프레임(텍스트). 해석은 pairing 모듈이 한다.
    Frame { id: CentralId, text: String },
    /// 바인딩 실패. 모듈 doc 참고 — 조용히 warn 만 남기면 안 된다.
    BindFailed(String),
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
    next_serial: AtomicU64,
    sinks: Sinks,
    slots: Arc<Slots>,
    /// 연결 핸들러가 각자 하나씩 들고 있다가, 토글이 꺼지면 스스로 빠져나온다.
    /// `handle` 의 주석 참고.
    shutdown: watch::Receiver<bool>,
}

pub struct ServerHandle {
    shutdown: watch::Sender<bool>,
    /// Task 4(스냅샷 푸시)가 쓴다. 여기만 상한이 없는 이유는 낯선 기기가 아니라
    /// 앱의 틱 루프(초당 하나)만 쓰기 때문이다 — 바깥에서 닿지 않는다.
    pub outbound: UnboundedSender<Outbound>,
}

impl ServerHandle {
    pub fn stop(self) {
        // 수신자가 이미 사라졌어도(태스크가 먼저 끝난 경우) 문제되지 않는다.
        let _ = self.shutdown.send(true);
    }
}

/// 리스너를 띄운다. 운영에서 `port` 는 언제나 `PORT` 다 — 인자로 받는 이유는
/// 테스트가 서로의(그리고 개발 중인 앱의) 4320 을 밟지 않게 하기 위해서다.
/// 바인딩은 이 안에서 기다리지 않는다. 실패는 `ServerEvent::BindFailed` 로 온다.
pub fn spawn(port: u16, events: Sender<ServerEvent>) -> ServerHandle {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        events,
        next_serial: AtomicU64::new(0),
        sinks: Sinks::default(),
        slots: Arc::new(Slots::default()),
        shutdown: shutdown_rx,
    });
    tokio::spawn(run(state, port, out_rx));
    ServerHandle { shutdown: shutdown_tx, outbound: out_tx }
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

async fn run(state: Arc<AppState>, port: u16, mut outbound: UnboundedReceiver<Outbound>) {
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
                .send(ServerEvent::BindFailed(format!(
                    "포트 {port} 을(를) 열지 못했습니다: {e}"
                )))
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
    let serial = state.next_serial.fetch_add(1, Ordering::Relaxed);
    let shutdown = state.shutdown.clone();
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| handle(socket, state, central_id(serial), shutdown, slot))
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

    loop {
        tokio::select! {
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    if !offer_frame(&state.events, &id, text) {
                        tracing::warn!(id = %id.0, "LAN 이벤트 큐가 찼다 — 연결을 놓는다");
                        break;
                    }
                }
                // 클라이언트가 보내는 바이너리는 지금 쓰지 않는다 — 미러는 읽기 전용이다.
                Some(Ok(_)) => {}
                // 오류(상한 초과 포함)든 스트림 끝이든 이 연결은 끝났다.
                Some(Err(_)) | None => break,
            },
            // 쓰기 쪽이 먼저 끝났다 = 상대가 읽지 않아 큐가 밀려 `push_to_sink`
            // 가 큐를 지웠거나, 소켓 쓰기가 실패했다. 읽기만 계속 붙들고 있을
            // 이유가 없다.
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

    /// 같은 연결에 같은 id 가, 다른 연결에 다른 id 가 나와야 한다.
    #[test]
    fn serials_do_not_repeat() {
        let a = central_id(1);
        let b = central_id(2);
        assert_ne!(a, b);
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
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        handle.stop();

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
        let handle = spawn(port, tx);
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
        let _handle = spawn(port, tx);

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("BindFailed 가 오지 않았다")
            .unwrap();
        match ev {
            ServerEvent::BindFailed(msg) => {
                assert!(msg.contains(&port.to_string()), "어느 포트인지 말해야 한다: {msg}");
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
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let sock = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(0)));

        drop(sock);
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(central_id(0)));

        handle.stop();
    }

    /// 두 번째 연결은 다른 id 를 받는다. 같은 기기가 다시 붙어도 새 세션이다 —
    /// 예전 세션의 인가를 물려받으면 안 된다.
    #[tokio::test]
    async fn a_second_connection_gets_a_new_session() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let first = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(0)));
        drop(first);
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(central_id(0)));

        let second = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(1)));

        drop(second);
        handle.stop();
    }

    /// 토글을 끄면 붙어 있던 기기도 정리돼야 한다 — 리스너만 닫고 연결을
    /// 놔두면 그 기기는 계속 인가된 채로 남는다.
    #[tokio::test]
    async fn stopping_disconnects_live_connections() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let _sock = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(0)));

        handle.stop();
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(central_id(0)));
    }

    /// 정상 크기의 텍스트 프레임은 그대로 올라온다 — 상한이 진짜 트래픽까지
    /// 막아버리면 안 되므로 거절 테스트와 짝으로 둔다.
    #[tokio::test]
    async fn a_normal_text_frame_arrives_as_an_event() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(0)));

        sock.write_all(&masked_text_frame(b"HELLO:1")).await.unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Frame { id: central_id(0), text: "HELLO:1".to_string() }
        );

        drop(sock);
        handle.stop();
    }

    /// 상한을 넘겨 선언한 프레임은 조립하지 않고 연결을 끊어야 한다. 인증도
    /// 하기 전에 낯선 기기가 64 MiB 를 우리에게 들고 있게 만들 수 있으면 안 된다.
    #[tokio::test]
    async fn an_oversized_frame_drops_the_connection_instead_of_buffering_it() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        assert_eq!(next_event(&mut rx).await, ServerEvent::Connected(central_id(0)));

        // 상한의 두 배를 선언한다. 본문은 한 바이트도 보내지 않는다.
        sock.write_all(&oversized_frame_header(2 * MAX_FRAME_BYTES as u64)).await.unwrap();

        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Disconnected(central_id(0)),
            "상한을 넘는 프레임은 본문을 기다리지 않고 끊어야 한다"
        );

        handle.stop();
    }

    /// 동시 연결 상한을 넘는 피어는 업그레이드부터 거절한다.
    #[tokio::test]
    async fn the_connection_cap_refuses_extra_peers() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let mut live = Vec::new();
        for i in 0..MAX_CONNECTIONS {
            live.push(handshake(port).await);
            assert_eq!(
                next_event(&mut rx).await,
                ServerEvent::Connected(central_id(i as u64))
            );
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
                handle.stop();
                return;
            }
            drop(s);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("자리가 비었는데도 계속 거절한다");
    }
}
