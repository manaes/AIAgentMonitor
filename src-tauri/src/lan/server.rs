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

use crate::ble::peripheral::CentralId;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;

/// quota 프록시(4319) 바로 옆.
pub const PORT: u16 = 4320;

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

/// central 별 송신 큐. 연결이 하나 생기면 하나 만들고, 그 연결이 끝나면 지운다.
/// 이 규칙이 어긋나면 끊긴 기기의 큐가 계속 쌓이거나(누수), 붙어 있는 기기에
/// 스냅샷이 나가지 않는다. 연결 코드 안에 두면 그 규칙만 따로 확인할 방법이
/// 없어서 별도 타입으로 뺐다.
#[derive(Default)]
struct Sinks(Mutex<HashMap<CentralId, UnboundedSender<Message>>>);

impl Sinks {
    fn insert(&self, id: CentralId, tx: UnboundedSender<Message>) {
        self.0.lock().unwrap().insert(id, tx);
    }

    fn remove(&self, id: &CentralId) {
        self.0.lock().unwrap().remove(id);
    }

    /// 잠금을 쥔 채로 보내지 않는다 — 송신은 await 를 탈 수 있고, 그 사이에
    /// 새 연결이 자기 큐를 등록하지 못하면 안 된다.
    fn get(&self, id: &CentralId) -> Option<UnboundedSender<Message>> {
        self.0.lock().unwrap().get(id).cloned()
    }

    /// 큐 개수를 세는 건 "연결 하나가 큐 하나" 규칙을 확인할 때뿐이다.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

struct AppState {
    events: UnboundedSender<ServerEvent>,
    next_serial: AtomicU64,
    sinks: Sinks,
    /// 연결 핸들러가 각자 하나씩 들고 있다가, 토글이 꺼지면 스스로 빠져나온다.
    /// `handle` 의 주석 참고.
    shutdown: watch::Receiver<bool>,
}

pub struct ServerHandle {
    shutdown: watch::Sender<bool>,
    /// Task 4(스냅샷 푸시)가 쓴다. 지금은 서버가 앱→기기 단방향이라 소비자가 없다.
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
pub fn spawn(port: u16, events: UnboundedSender<ServerEvent>) -> ServerHandle {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        events,
        next_serial: AtomicU64::new(0),
        sinks: Sinks::default(),
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
            let _ = events.send(ServerEvent::BindFailed(format!(
                "포트 {port} 을(를) 열지 못했습니다: {e}"
            )));
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
            if let Some(s) = pump_state.sinks.get(&id) {
                let _ = s.send(msg);
            }
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

async fn upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    // 일련번호는 업그레이드 시점에 뽑는다 — 핸들러가 실제로 도는 시점에 뽑으면
    // 두 기기가 거의 동시에 붙었을 때 순서가 뒤집힐 수 있다(id 자체는 여전히
    // 유일하지만, 목록 순서가 접속 순서와 어긋난다).
    let serial = state.next_serial.fetch_add(1, Ordering::Relaxed);
    let shutdown = state.shutdown.clone();
    ws.on_upgrade(move |socket| handle(socket, state, central_id(serial), shutdown))
}

async fn handle(
    socket: WebSocket,
    state: Arc<AppState>,
    id: CentralId,
    mut shutdown: watch::Receiver<bool>,
) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    state.sinks.insert(id.clone(), sink_tx);
    let _ = state.events.send(ServerEvent::Connected(id.clone()));

    let writer = tokio::spawn(async move {
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
                    let _ = state.events.send(ServerEvent::Frame { id: id.clone(), text });
                }
                // 클라이언트가 보내는 바이너리는 지금 쓰지 않는다 — 미러는 읽기 전용이다.
                Some(Ok(_)) => {}
                // 오류든 스트림 끝이든 이 연결은 끝났다.
                Some(Err(_)) | None => break,
            },
            // 토글을 끄면 리스너만 닫는 것으로는 부족하다. 미러 연결은 스스로
            // 끝나지 않으므로, 여기서 깨우지 않으면 axum 의 graceful shutdown 이
            // 영원히 기다리고 기기는 계속 붙어 있는 채로 남는다.
            _ = shutdown.changed() => break,
        }
    }

    writer.abort();
    state.sinks.remove(&id);
    // 링크가 끊기면 인가를 즉시 지워야 한다(BLE 의 `Disconnected` 와 같은 규칙).
    let _ = state.events.send(ServerEvent::Disconnected(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
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
        assert!(sinks.get(&central_id(9)).is_none());
        assert_eq!(sinks.len(), 0);
    }

    /// 지금 비어 있는 포트를 하나 골라 온다. 테스트가 4320 을 잡으면 서로,
    /// 그리고 개발 중인 앱과 충돌한다.
    async fn free_port() -> u16 {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        l.local_addr().unwrap().port()
    }

    async fn wait_until_listening(port: u16) {
        for _ in 0..200 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("서버가 {port} 에서 뜨지 않았다");
    }

    /// 토글을 끄면 플래그만 내려가는 게 아니라 소켓이 실제로 닫혀야 한다.
    /// "다시 바인딩할 수 있다"가 그것을 증명하는 유일하게 확실한 방법이다.
    #[tokio::test]
    async fn stopping_actually_releases_the_socket() {
        let port = free_port().await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
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
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
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

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
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

    /// WebSocket 핸드셰이크를 손으로 친다. 클라이언트 크레이트를 dev-dep 으로
    /// 더하지 않으려는 것이고, 여기서 확인하려는 것은 프레이밍이 아니라
    /// **연결 수명**(붙으면 Connected, 끊기면 Disconnected)이라 이 정도면 된다.
    async fn handshake(port: u16) -> tokio::net::TcpStream {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
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

        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(head.starts_with("HTTP/1.1 101"), "업그레이드가 거절됐다: {head}");
        s
    }

    /// 연결 하나가 세션 하나다 — 붙는 순간 `Connected`, 끊기는 순간
    /// `Disconnected` 가 같은 id 로 나와야 한다. 뒤쪽이 빠지면 인가가 링크보다
    /// 오래 살아남아 기기 목록이 계속 거짓말한다.
    #[tokio::test]
    async fn one_connection_is_one_session() {
        let port = free_port().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let sock = handshake(port).await;
        let connected = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Connected 가 오지 않았다")
            .unwrap();
        assert_eq!(connected, ServerEvent::Connected(central_id(0)));

        drop(sock);
        let disconnected = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Disconnected 가 오지 않았다")
            .unwrap();
        assert_eq!(disconnected, ServerEvent::Disconnected(central_id(0)));

        handle.stop();
    }

    /// 두 번째 연결은 다른 id 를 받는다. 같은 기기가 다시 붙어도 새 세션이다 —
    /// 예전 세션의 인가를 물려받으면 안 된다.
    #[tokio::test]
    async fn a_second_connection_gets_a_new_session() {
        let port = free_port().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let first = handshake(port).await;
        assert_eq!(rx.recv().await.unwrap(), ServerEvent::Connected(central_id(0)));
        drop(first);
        assert_eq!(rx.recv().await.unwrap(), ServerEvent::Disconnected(central_id(0)));

        let second = handshake(port).await;
        assert_eq!(rx.recv().await.unwrap(), ServerEvent::Connected(central_id(1)));

        drop(second);
        handle.stop();
    }

    /// 토글을 끄면 붙어 있던 기기도 정리돼야 한다 — 리스너만 닫고 연결을
    /// 놔두면 그 기기는 계속 인가된 채로 남는다.
    #[tokio::test]
    async fn stopping_disconnects_live_connections() {
        let port = free_port().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(port, tx);
        wait_until_listening(port).await;

        let _sock = handshake(port).await;
        assert_eq!(rx.recv().await.unwrap(), ServerEvent::Connected(central_id(0)));

        handle.stop();

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("stop() 이 살아 있는 연결을 끊지 않았다")
            .unwrap();
        assert_eq!(ev, ServerEvent::Disconnected(central_id(0)));
    }
}
