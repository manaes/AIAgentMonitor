# CYD 클라이언트 구현 계획 — LAN 전송 + ESP32 펌웨어

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ESP32 Cheap Yellow Display 를 세 번째 클라이언트로 붙인다. 맥에는 세 번째 전송 브리지(LAN/WebSocket)가 생기고, 페어링·기기 목록은 기존 공유 구조를 그대로 쓴다.

**Architecture:** WebSocket 연결 하나가 세션 하나(`CentralId` 하나)다 — BLE 링크·iroh 스트림과 같은 모델이다. 인증 프레임은 텍스트, 스냅샷은 E2EE v2 봉인 프레임을 바이너리로 싣는다. 청킹이 없으므로 MTU 도 255청크 상한도 존재하지 않는다.

**Tech Stack:** Rust (`axum 0.7` + `ws` 피처, `mdns-sd 0.21.0`), PlatformIO + `esp32-smartdisplay`(LVGL) + `arduinoWebSockets` + monocypher

**Spec:** `docs/superpowers/specs/2026-08-25-cyd-client-design.md`

**선행 계획:** `docs/superpowers/plans/2026-08-25-e2ee-protocol-v2.md` — **Task 1~9 가 끝나 있어야 한다.** 이 계획은 `crypto::` 모듈과 `PairingManager` 의 v2 분기를 전제한다.

## Global Constraints

- 페어링 코드 TTL **120초**, 창당 시도 **5회** — **절대 넓히지 않는다**
- 토큰은 **128비트 소문자 hex**, 6자리 코드는 어느 방향으로도 링크를 건너지 않는다
- 인가되지 않은 연결은 스냅샷을 **0바이트** 받는다
- **LAN 공유는 기본 꺼짐.** 토글을 켤 때만 리스너가 뜬다
- `begin_pairing` 은 맥 쪽 명시적 사용자 제스처로만
- 바인딩 주소 **`0.0.0.0:4320`**, 라우트 **`GET /mirror`**
- mDNS 서비스 **`_aim._tcp.local`**, 포트 4320, TXT `v=2`
- `CentralId` 형식 **`lan:<연결 일련번호>`** — IP 를 넣지 않는다
- 사용자에게 보이는 식별자는 기존과 같이 `peer_id = hex(SHA-256(token))[..8]`
- 표시 규칙(사용률 바 색 경계 **70%/90%**, 세션 정렬 = 최근 활동순, 상태 단어)은 맥·iOS 와 **같아야 한다**
- 기기에 별도 에이전트 선택 설정을 두지 않는다 — 필터는 이미 `lib.rs:996` 의 `snap.agents.retain(...)` 에서 적용되어 도착한다
- 폰트: UI 라벨만 한글 서브셋. 사용자 데이터(프로젝트명·모델명)는 ASCII 가정, 비ASCII 는 대체 문자
- **SD 카드를 쓰지 않는다** — 쓰면 디스플레이·터치·SD 가 SPI 버스 2개를 두고 다툰다

### 검증 상태에 대한 정직한 고지

- **맥 쪽(Task 1~7)의 크레이트 API 는 실제로 확인했다** — `axum` 의 `ws` 피처와
  `mdns-sd 0.21.0` 이 해석되는 것을 `cargo add --dry-run` 으로 봤다.
- **펌웨어(Task 8~15)의 코드는 컴파일해 보지 않았다.** 하드웨어도 툴체인도 아직
  없다. 문서화된 API 를 근거로 썼으므로, 첫 빌드에서 시그니처가 어긋날 수 있다.
  각 태스크의 첫 단계가 "빌드가 되는지 먼저 확인한다"인 이유다.

## 파일 구조

| 파일 | 책임 |
|---|---|
| `src-tauri/src/lan/mod.rs` (신규) | `network/mod.rs` 와 같은 표면의 브리지 |
| `src-tauri/src/lan/server.rs` (신규) | axum 라우터·리스너·연결 수명 |
| `src-tauri/src/lan/discovery.rs` (신규) | mDNS 게시 |
| `src-tauri/src/lib.rs` (수정) | 명령·배선·상태 |
| `src/components/DevicePanel.svelte` (수정) | 세 번째 토글, IP 표시 |
| `src/lib/store.svelte.ts` / `tauri.ts` (수정) | `lanStatus` / `lanSetEnabled` |
| `firmware/cyd/platformio.ini` (신규) | 보드·라이브러리·파티션 |
| `firmware/cyd/src/main.cpp` (신규) | 부팅·루프 |
| `firmware/cyd/src/config.h/.cpp` (신규) | NVS 설정(WiFi·맥 IP·토큰) |
| `firmware/cyd/src/transport.h/.cpp` (신규) | WebSocket + 발견 |
| `firmware/cyd/src/cryptov2.h/.cpp` (신규) | monocypher 래퍼 |
| `firmware/cyd/src/authfsm.h/.cpp` (신규) | 인증 상태 기계 — **순수 함수** |
| `firmware/cyd/src/ui_*.h/.cpp` (신규) | 키패드·카드·세션 목록 |
| `firmware/cyd/src/font_ko.c` (생성물) | 한글 서브셋 |
| `docs/ble-protocol/DEVICE-TEST.md` (수정) | CYD 절 |

---

## Task 1: `lan` 브리지 골격

**Files:**
- Create: `src-tauri/src/lan/mod.rs`
- Modify: `src-tauri/src/lib.rs` (`mod lan;`)

**Interfaces:**
- Consumes: `ble::pairing::{PairingManager, AuthReply}`, `ble::peripheral::CentralId`
- Produces:
  - `pub struct LanBridge`
  - `pub fn new() -> LanBridge`
  - `pub fn is_enabled(&self) -> bool`
  - `pub fn set_enabled(&mut self, on: bool)   // network 브리지와 동일 — Result 아님`
  - `pub fn served_centrals(&self) -> Vec<CentralId>`
  - `pub fn last_error(&self) -> Option<String>`

`network/mod.rs` 와 **같은 표면**을 갖는다. 세 브리지가 같은 모양이면 `lib.rs` 배선이
대칭이 되고, 하나만 고치고 다른 쪽을 잊는 드리프트가 줄어든다.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_disabled() {
        assert!(!LanBridge::new().is_enabled(), "LAN 공유는 기본 꺼짐이다");
    }

    #[test]
    fn toggling_on_and_off_is_idempotent() {
        let mut b = LanBridge::new();
        b.set_enabled(true).unwrap();
        b.set_enabled(true).unwrap();
        assert!(b.is_enabled());
        b.set_enabled(false).unwrap();
        b.set_enabled(false).unwrap();
        assert!(!b.is_enabled());
    }

    #[test]
    fn has_no_centrals_when_disabled() {
        assert!(LanBridge::new().served_centrals().is_empty());
    }
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::`
Expected: FAIL — `cannot find type LanBridge`

- [ ] **Step 3: 구현한다**

```rust
//! LAN 전송 브리지 (스펙 2026-08-25-cyd-client-design.md).
//!
//! `network/mod.rs`(iroh)와 같은 표면을 갖는다. WebSocket 연결 하나가 세션
//! 하나이고 `CentralId` 하나에 대응한다 — BLE 링크와 같은 모델이다.

pub mod discovery;
pub mod server;

use crate::ble::peripheral::CentralId;
use std::collections::HashSet;

pub struct LanBridge {
    enabled: bool,
    /// 현재 붙어 있는 연결들. 서버 태스크가 갱신한다.
    centrals: HashSet<String>,
    /// 사용자에게 보여줄 마지막 오류. 이 앱은 로그 파일을 남기지 않으므로
    /// 패널이 실패 원인을 알 수 있는 유일한 경로다.
    last_error: Option<String>,
}

impl LanBridge {
    pub fn new() -> Self {
        Self { enabled: false, centrals: HashSet::new(), last_error: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, on: bool)   // network 브리지와 동일 — Result 아님 {
        if on == self.enabled {
            return Ok(());
        }
        self.enabled = on;
        if !on {
            self.centrals.clear();
        }
        Ok(())
    }

    pub fn served_centrals(&self) -> Vec<CentralId> {
        self.centrals.iter().cloned().map(CentralId).collect()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.clone()
    }
}
```

`lib.rs` 에 `mod lan;` 을 추가한다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::`
Expected: PASS — 3개

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/lan/mod.rs src-tauri/src/lib.rs
git commit -m "feat(lan): 브리지 골격 — network 브리지와 같은 표면"
```

---

## Task 2: WebSocket 서버와 연결 수명

**Files:**
- Create: `src-tauri/src/lan/server.rs`
- Modify: `src-tauri/Cargo.toml` (`axum` 에 `ws` 피처)
- Modify: `src-tauri/src/lan/mod.rs` (`set_enabled` 가 실제로 리스너를 띄운다)

**Interfaces:**
- Consumes: Task 1 의 `LanBridge`
- Produces:
  - `pub const PORT: u16 = 4320`
  - `pub enum ServerEvent { Connected(CentralId), Disconnected(CentralId), Frame { id: CentralId, text: String }, BindFailed(String) }`
  - `pub fn spawn(events: Sender<ServerEvent>) -> ServerHandle`
  - `pub struct ServerHandle { pub fn stop(self) }`

- [ ] **Step 1: 의존성을 켠다**

```bash
cd src-tauri
cargo add axum@0.7 --features json,ws
grep -n '^axum' Cargo.toml   # features = ["json", "ws"] 여야 한다
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/lan/server.rs` 아래:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::server::`
Expected: FAIL — `cannot find value PORT`

- [ ] **Step 4: 구현한다**

```rust
//! LAN WebSocket 서버. `quota_proxy.rs` 의 `TcpListener::bind` + `axum::serve`
//! 패턴을 그대로 따른다.

use crate::ble::peripheral::CentralId;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// quota 프록시(4319) 바로 옆.
pub const PORT: u16 = 4320;

pub fn central_id(serial: u64) -> CentralId {
    CentralId(format!("lan:{serial}"))
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Connected(CentralId),
    Disconnected(CentralId),
    /// 인증 프레임(텍스트). 해석은 pairing 모듈이 한다.
    Frame { id: CentralId, text: String },
    /// 바인딩 실패. 사용자가 방금 켠 기능이므로 패널에 보여야 한다 —
    /// quota 프록시처럼 조용히 warn 만 남기면 안 된다.
    BindFailed(String),
}

/// 이 central 에게 보낼 바이트. 서버 태스크가 소유한 송신 채널로 전달된다.
pub enum Outbound {
    Text(CentralId, Vec<u8>),
    Binary(CentralId, Vec<u8>),
    Close(CentralId),
}

struct AppState {
    events: UnboundedSender<ServerEvent>,
    next_serial: AtomicU64,
    /// central 별 송신 채널.
    sinks: std::sync::Mutex<std::collections::HashMap<String, UnboundedSender<Message>>>,
}

pub struct ServerHandle {
    shutdown: tokio::sync::oneshot::Sender<()>,
    pub outbound: UnboundedSender<Outbound>,
}

impl ServerHandle {
    pub fn stop(self) {
        // 수신자가 이미 사라졌어도 문제되지 않는다.
        let _ = self.shutdown.send(());
    }
}

pub fn spawn(events: UnboundedSender<ServerEvent>) -> ServerHandle {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        events: events.clone(),
        next_serial: AtomicU64::new(0),
        sinks: Default::default(),
    });
    tokio::spawn(run(state, events, shutdown_rx, out_rx));
    ServerHandle { shutdown: shutdown_tx, outbound: out_tx }
}

async fn run(
    state: Arc<AppState>,
    events: UnboundedSender<ServerEvent>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
    mut outbound: UnboundedReceiver<Outbound>,
) {
    let app = Router::new()
        .route("/mirror", get(upgrade))
        .with_state(state.clone());
    let addr = SocketAddr::from(([0, 0, 0, 0], PORT));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let _ = events.send(ServerEvent::BindFailed(format!(
                "포트 {PORT} 을(를) 열지 못했습니다: {e}"
            )));
            return;
        }
    };

    // 송신 펌프 — central 별 채널로 넘긴다.
    let pump_state = state.clone();
    tokio::spawn(async move {
        while let Some(item) = outbound.recv().await {
            let (id, msg) = match item {
                Outbound::Text(id, b) => (id, Message::Text(String::from_utf8_lossy(&b).into())),
                Outbound::Binary(id, b) => (id, Message::Binary(b)),
                Outbound::Close(id) => (id, Message::Close(None)),
            };
            let sink = pump_state.sinks.lock().unwrap().get(&id.0).cloned();
            if let Some(s) = sink {
                let _ = s.send(msg);
            }
        }
    });

    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = shutdown.await;
        })
        .await;
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    let serial = state.next_serial.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| handle(socket, state, central_id(serial)))
}

async fn handle(socket: WebSocket, state: Arc<AppState>, id: CentralId) {
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, mut rx) = socket.split();
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    state.sinks.lock().unwrap().insert(id.0.clone(), sink_tx);
    let _ = state.events.send(ServerEvent::Connected(id.clone()));

    let writer = tokio::spawn(async move {
        while let Some(m) = sink_rx.recv().await {
            if tx.send(m).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = rx.next().await {
        if let Message::Text(text) = msg {
            let _ = state.events.send(ServerEvent::Frame { id: id.clone(), text });
        }
        // 클라이언트가 보내는 바이너리는 지금 쓰지 않는다 — 미러는 읽기 전용이다.
    }

    writer.abort();
    state.sinks.lock().unwrap().remove(&id.0);
    // 링크가 끊기면 인가를 즉시 지워야 한다(BLE 의 Disconnected 와 같다).
    let _ = state.events.send(ServerEvent::Disconnected(id));
}
```

`futures-util` 이 필요하면 `cargo add futures-util` 로 더한다.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::`
Expected: PASS

- [ ] **Step 6: 실제로 뜨는지 손으로 확인한다**

```bash
pnpm tauri dev
# 다른 터미널에서
lsof -nP -iTCP:4320 -sTCP:LISTEN   # 토글을 켜기 전에는 아무것도 없어야 한다
```

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lan/
git commit -m "feat(lan): WebSocket 서버와 연결 수명 — 연결 하나가 세션 하나"
```

---

## Task 3: 인증 배선

**Files:**
- Modify: `src-tauri/src/lan/mod.rs`
- Modify: `src-tauri/src/lib.rs` (서버 이벤트를 `PairingManager` 로 흘린다)

**Interfaces:**
- Consumes: Task 2 의 `ServerEvent`, E2EE 계획 Task 6·7 의 v2 분기
- Produces: `pub fn handle_auth(&mut self, id: &CentralId, data: &[u8], pairing: &mut PairingManager) -> AuthOutcome`
  where `pub struct AuthOutcome { pub payload: Vec<u8>, pub now_authorized: bool, pub granted: bool }`

> **정정 2026-08-26.** 초안은 `granted` 를 뺐다 — "LAN 은 v2 전용이라 필요 없다"는
> 판단이었는데 **틀렸다.** `granted` 는 `lib.rs:885` 에서 토큰을 디스크에 쓰는 신호이고,
> v2 페어링(`Granted2`)도 새 토큰을 발급한다. 빠뜨리면 LAN 으로 페어링한 기기가 그
> 세션에서는 동작하다가 맥을 껐다 켜는 순간 토큰이 사라져 영영 재연결이 안 된다.
> `ble/mod.rs:183-189` 의 주석이 같은 이유를 적어두고 있다.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
    use crate::ble::pairing::{AuthReply, PairingManager};
    use std::time::{Duration, UNIX_EPOCH};

    fn t(secs: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// 인증 프레임은 `handle_auth` 가 그대로 PairingManager 에 넘기고,
    /// 응답은 `AuthReply::to_json_bytes()` 그대로다. 새 포맷을 만들지 않는다.
    #[test]
    fn passes_auth_frames_through_unchanged() {
        let mut b = LanBridge::new();
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));
        let id = CentralId("lan:0".into());

        let out = b.handle_auth(&id, b"HELLO2:short", &mut p);
        assert_eq!(
            out.payload,
            AuthReply::Rejected.to_json_bytes(),
            "형식 오류는 그대로 Rejected 가 나가야 한다"
        );
        assert!(!out.now_authorized);
    }

    /// 인가되기 전에는 스냅샷이 0바이트다.
    #[test]
    fn sends_nothing_before_authorization() {
        let mut b = LanBridge::new();
        b.set_enabled(true).unwrap();
        let p = PairingManager::new();
        let sent = b.snapshot_targets(&p);
        assert!(sent.is_empty(), "인가되지 않으면 대상이 없다");
    }

    /// 연결이 끊기면 인가가 즉시 사라진다.
    #[test]
    fn dropping_a_connection_ends_its_session() {
        let mut b = LanBridge::new();
        let mut p = PairingManager::new();
        let id = CentralId("lan:0".into());
        b.on_connected(&id);
        b.on_disconnected(&id, &mut p);
        assert!(!p.is_authorized(&id));
        assert!(b.served_centrals().is_empty());
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::tests::passes_auth`
Expected: FAIL — `no method named handle_auth`

- [ ] **Step 3: 구현한다**

```rust
use crate::ble::pairing::{parse_auth_request, PairingManager};

pub struct AuthOutcome {
    /// 이 central 에게 되돌려보낼 바이트. `AuthReply::to_json_bytes()` 그대로다.
    pub payload: Vec<u8>,
    /// 이번 요청으로 새로 인가됐는가.
    pub now_authorized: bool,
}

impl LanBridge {
    pub fn on_connected(&mut self, id: &CentralId) {
        self.centrals.insert(id.0.clone());
    }

    /// 링크가 끊기면 인가를 즉시 지운다 — BLE 의 `Disconnected` 와 같다.
    pub fn on_disconnected(&mut self, id: &CentralId, pairing: &mut PairingManager) {
        self.centrals.remove(&id.0);
        pairing.end_session(id);
    }

    pub fn handle_auth(
        &mut self,
        id: &CentralId,
        data: &[u8],
        pairing: &mut PairingManager,
    ) -> AuthOutcome {
        let before = pairing.is_authorized(id);
        let reply = pairing.handle(id, parse_auth_request(data), std::time::SystemTime::now());
        let after = pairing.is_authorized(id);
        AuthOutcome { payload: reply.to_json_bytes(), now_authorized: !before && after }
    }

    /// 스냅샷을 보낼 대상. 인가되지 않은 연결은 들어가지 않는다.
    pub fn snapshot_targets(&self, pairing: &PairingManager) -> Vec<CentralId> {
        self.served_centrals()
            .into_iter()
            .filter(|id| pairing.is_authorized(id))
            .collect()
    }

    pub fn set_last_error(&mut self, msg: Option<String>) {
        self.last_error = msg;
    }
}
```

`lib.rs` 에서 `ServerEvent` 를 받아 각각 `on_connected` / `on_disconnected` /
`handle_auth` 로 흘리고, `BindFailed` 는 `set_last_error` 로 넣는다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::`
Expected: PASS

- [ ] **Step 5: 뮤테이션 확인**

`on_disconnected` 의 `pairing.end_session(id);` 를 지운다 →
`dropping_a_connection_ends_its_session` 이 **반드시 실패해야 한다.** 되돌린다.

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/lan/ src-tauri/src/lib.rs
git commit -m "feat(lan): 인증 배선 — 프레임을 그대로 PairingManager 에 넘긴다"
```

---

## Task 4: 봉인 스냅샷 전송

**Files:**
- Modify: `src-tauri/src/lan/mod.rs`
- Modify: `src-tauri/src/lib.rs` (스냅샷 배포에 LAN 을 더한다)

**Interfaces:**
- Consumes: E2EE 계획 Task 7 의 `PairingManager::channel_mut`, Task 2 의 `Outbound`
- Produces:
  - `pub fn prepare_snapshot(&mut self, snap: &Snapshot, now: SystemTime, pairing: &mut PairingManager) -> Vec<(CentralId, Vec<u8>)>` — 동기, 봉인까지만
  - `pub async fn send_prepared(&mut self, lines: Vec<(CentralId, Vec<u8>)>)` — 비동기, 쓰기만

> **정정 2026-08-26.** 초안은 `on_snapshot` 하나였다. E2EE v2 작업 중
> `network/mod.rs` 가 이 둘로 쪼개졌다 — 페어링 잠금이 `write_all().await` 를 넘어
> 유지되면 폰 하나가 백그라운드로 내려갔을 때 `begin_pairing` 과 양 전송의
> `handle_auth` 가 수십 초 멈추기 때문이다. LAN 도 같은 모양을 따른다:
> 호출부가 `prepare_snapshot` 뒤 `drop(pairing)` 하고 `send_prepared` 를 부른다.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
    /// LAN 은 청킹하지 않는다 — WebSocket 이 프레이밍을 한다.
    /// 봉인 프레임을 바이너리 메시지 하나로 그대로 싣는다.
    #[test]
    fn sends_one_binary_frame_per_authorized_central() {
        // (v2 페어링을 마친 central 하나를 준비한다 — pairing.rs 의 v2 테스트가
        //  쓰는 준비 코드를 그대로 따른다)
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        b.on_snapshot(&sample_snapshot(), &mut p, &tx);
        let sent = rx.try_recv().expect("한 건이 나가야 한다");
        match sent {
            Outbound::Binary(_, bytes) => {
                assert!(!bytes.starts_with(b"{"), "평문 JSON 이 나갔다");
                assert!(bytes.len() > 8 + 16, "카운터 8 + 태그 16 보다 길어야 한다");
            }
            _ => panic!("바이너리 프레임이어야 한다"),
        }
        assert!(rx.try_recv().is_err(), "청킹하지 않으므로 한 건뿐이다");
    }

    /// 인가되지 않은 연결에는 아무것도 나가지 않는다.
    #[test]
    fn unauthorized_connection_receives_zero_bytes() {
        let mut b = LanBridge::new();
        b.set_enabled(true).unwrap();
        b.on_connected(&CentralId("lan:0".into()));
        let mut p = PairingManager::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        b.on_snapshot(&sample_snapshot(), &mut p, &tx);
        assert!(rx.try_recv().is_err(), "0바이트여야 한다");
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::tests::sends_one_binary`
Expected: FAIL — `no method named on_snapshot`

- [ ] **Step 3: 구현한다**

```rust
use crate::ble::wire::MirrorSnapshot;
use crate::lan::server::Outbound;
use crate::types::Snapshot;
use tokio::sync::mpsc::UnboundedSender;

impl LanBridge {
    pub fn on_snapshot(
        &mut self,
        snap: &Snapshot,
        pairing: &mut PairingManager,
        out: &UnboundedSender<Outbound>,
    ) {
        if !self.enabled {
            return;
        }
        let targets = self.snapshot_targets(pairing);
        if targets.is_empty() {
            return;
        }
        let json = match serde_json::to_vec(&MirrorSnapshot::from(snap)) {
            Ok(j) => j,
            Err(e) => {
                self.last_error = Some(format!("스냅샷 직렬화 실패: {e}"));
                return;
            }
        };
        for id in targets {
            // v2 세션이면 봉인한다. 채널이 없으면 인가된 v1 세션인데,
            // LAN 은 v2 전용이므로 그런 경우가 없어야 한다 — 있으면 건너뛴다.
            let Some(ch) = pairing.channel_mut(&id) else {
                continue;
            };
            let _ = out.send(Outbound::Binary(id, ch.seal(&json)));
        }
    }
}
```

`lib.rs` 의 스냅샷 배포 지점에서 BLE·네트워크와 나란히 `lan.on_snapshot(...)` 을 부른다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — 전체

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/
git commit -m "feat(lan): 봉인된 스냅샷을 바이너리 프레임 하나로 보낸다"
```

---

## Task 5: mDNS 게시와 IP 표시

**Files:**
- Create: `src-tauri/src/lan/discovery.rs`
- Modify: `src-tauri/Cargo.toml` (`mdns-sd`)
- Modify: `src-tauri/src/lan/mod.rs`

**Interfaces:**
- Consumes: Task 2 의 `PORT`
- Produces:
  - `pub const SERVICE_TYPE: &str = "_aim._tcp.local."`
  - `pub fn publish() -> anyhow::Result<Publication>`
  - `pub struct Publication { pub fn stop(self) }`
  - `pub fn local_ipv4() -> Option<String>`

- [ ] **Step 1: 의존성을 더한다**

```bash
cd src-tauri && cargo add mdns-sd@0.21.0
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_matches_spec() {
        assert_eq!(SERVICE_TYPE, "_aim._tcp.local.");
    }

    /// 수동 입력 대비로 맥 UI 가 IP 를 보여줘야 한다. 루프백을 고르면
    /// 사용자가 그 값을 CYD 에 넣어도 붙지 못한다.
    #[test]
    fn local_ipv4_is_not_loopback() {
        if let Some(ip) = local_ipv4() {
            assert!(!ip.starts_with("127."), "루프백을 고르면 안 된다: {ip}");
        }
        // 랜선이 빠진 CI 에서는 None 일 수 있다 — None 자체는 실패가 아니다.
    }
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::discovery::`
Expected: FAIL — `cannot find value SERVICE_TYPE`

- [ ] **Step 4: 구현한다**

```rust
//! mDNS 게시. CYD 는 `_aim._tcp.local.` 을 조회해 맥을 찾는다.
//! 조회가 막힌 망을 위해 맥 UI 는 IP 를 함께 보여준다.

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_aim._tcp.local.";

pub struct Publication {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Publication {
    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

pub fn publish() -> anyhow::Result<Publication> {
    let daemon = ServiceDaemon::new()?;
    let host = hostname();
    let instance = format!("AIM-{host}");
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &format!("{host}.local."),
        (),
        super::server::PORT,
        &[("v", "2")][..],
    )?
    .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    Ok(Publication { daemon, fullname })
}

fn hostname() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mac".to_string())
}

/// 사용자에게 보여줄 LAN IPv4. 루프백과 링크로컬은 제외한다.
pub fn local_ipv4() -> Option<String> {
    let out = std::process::Command::new("ipconfig")
        .args(["getifaddr", "en0"])
        .output()
        .ok()?;
    let ip = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if ip.is_empty() || ip.starts_with("127.") || ip.starts_with("169.254.") {
        // en0 가 아닌 경우(유선 등)를 위한 대체.
        let out = std::process::Command::new("ipconfig")
            .args(["getifaddr", "en1"])
            .output()
            .ok()?;
        let ip = String::from_utf8(out.stdout).ok()?.trim().to_string();
        return (!ip.is_empty() && !ip.starts_with("127.")).then_some(ip);
    }
    Some(ip)
}
```

`LanBridge::set_enabled(true)` 에서 `publish()` 를 부르고, `false` 에서 `stop()` 한다.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml lan::`
Expected: PASS

- [ ] **Step 6: 실제로 게시되는지 확인한다**

```bash
pnpm tauri dev     # Devices 탭에서 LAN 공유를 켠다
dns-sd -B _aim._tcp local.      # AIM-<호스트> 가 보여야 한다
```

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lan/
git commit -m "feat(lan): mDNS 게시와 LAN IP 표시"
```

---

## Task 6: Devices 탭 세 번째 토글

**Files:**
- Modify: `src-tauri/src/lib.rs` (`lan_status` / `lan_set_enabled` 명령)
- Modify: `src/lib/tauri.ts`, `src/lib/store.svelte.ts`
- Modify: `src/components/DevicePanel.svelte`

**Interfaces:**
- Consumes: Task 1·5 의 `LanBridge`
- Produces:
  - Rust 명령 `lan_status() -> LanStatus`, `lan_set_enabled(on: bool)`
  - `LanStatus { supported: bool, enabled: bool, address: Option<String>, last_error: Option<String> }`
  - TS: `store.lan`, `store.setLanEnabled(on)`

- [ ] **Step 1: 구현한다**

`DevicePanel.svelte` 의 네트워크 토글 아래에 같은 모양으로 하나 더 넣는다.
BLE·네트워크와 **독립 토글**이다.

```svelte
  {#if lanSupported}
    <div class="row" style="margin-top: 8px;">
      <div class="text">
        <strong>LAN 공유</strong>
        <span class="subtle">같은 WiFi 의 전용 기기에 전송합니다</span>
      </div>
      <button class="toggle" class:on={lanEnabled} onclick={() => store.setLanEnabled(!lanEnabled)}>
        {lanEnabled ? "켜짐" : "꺼짐"}
      </button>
    </div>
  {/if}
```

페어링 창이 열려 있고 LAN 이 켜져 있으면, 6자리 코드 아래에 주소를 함께 보여준다.
**QR 은 만들지 않는다 — CYD 에는 카메라가 없다.**

```svelte
        {#if lanEnabled && store.lan?.address}
          <p class="code-label">
            LAN 으로 붙일 기기에는 이 주소를 넣으세요 — {store.lan.address}:4320
          </p>
        {/if}
```

`anyEnabled` 를 `bleEnabled || networkEnabled || lanEnabled` 로 고친다 —
LAN 만 켠 상태에서도 페어링 영역이 나와야 한다.

`shownError` 에 `store.lanActionError` 와 `store.lan?.last_error` 를 더한다.

- [ ] **Step 2: 손으로 확인한다**

```bash
pnpm tauri dev
```

- [ ] LAN 토글이 보이고, 기본이 **꺼짐**이다
- [ ] 켜면 주소가 나오고 `lsof -nP -iTCP:4320 -sTCP:LISTEN` 에 잡힌다
- [ ] 끄면 리스너가 사라진다
- [ ] BLE·네트워크와 **동시에** 켤 수 있다
- [ ] 페어링 창을 열면 6자리 코드와 주소가 함께 보인다
- [ ] 포트가 이미 쓰이는 상태에서 켜면 **패널에 빨간 오류가 뜬다**

마지막 항목은 이렇게 만든다:

```bash
nc -l 4320 &      # 포트를 미리 점유
# 그 상태에서 토글을 켠다
```

- [ ] **Step 3: 커밋**

```bash
git add src-tauri/src/lib.rs src/
git commit -m "feat(ui): Devices 탭에 LAN 공유 토글과 주소 표시"
```

---

## Task 7: 하드웨어 없는 종단 검증

**Files:**
- Create: `scripts/lan-e2e.md` (수동 절차)

**Interfaces:**
- Consumes: Task 1~6 전부
- Produces: 없음

**이 태스크가 LAN 을 BLE 보다 먼저 하는 이유 그 자체다.** 기기 없이 전 구간을 확인한다.

- [ ] **Step 1: 절차를 쓴다**

`scripts/lan-e2e.md`:

````markdown
# LAN 전송 종단 검증 (하드웨어 불필요)

`websocat` 이 필요하다: `brew install websocat`

## 1. 맥 준비

```bash
pnpm tauri dev
```
Devices 탭 → **LAN 공유** 켜기 → **페어링 시작** → 6자리 코드를 적어둔다.

## 2. 연결과 v2 핸드셰이크

```bash
websocat ws://127.0.0.1:4320/mirror
```

붙으면 아래를 순서대로 입력한다. 임시 키·HMAC 계산은
`scripts/lan_e2e_client.py`(이 태스크에서 함께 만든다)가 대신한다.

```bash
python3 scripts/lan_e2e_client.py --code 123456
```

## 3. 확인 항목

- [ ] `HELLO2:` 에 `{"ok":false,"v":2,"await":"code","epk":"…","nonce":"…"}` 가 온다
- [ ] 그 응답에 **6자리 코드가 들어 있지 않다**
- [ ] `CODE2:` 에 `{"ok":true,"v":2,"sealed":"…"}` 가 온다
- [ ] 그 응답에 **32자리 토큰이 평문으로 들어 있지 않다**
- [ ] `sealed` 를 풀면 `{"token":"<32 hex>"}` 가 나온다
- [ ] 이어서 오는 스냅샷 프레임이 **`{` 로 시작하지 않는다**(평문 JSON 이 아니다)
- [ ] 세션 키로 풀면 맥 Detail 창과 같은 값의 JSON 이 나온다
- [ ] 연결을 끊고 `AUTH2:` → `PROOF2:` 로 재인증이 된다
- [ ] 코드를 5번 틀리면 창이 닫히고 이후 시도가 전부 거부된다
- [ ] 맥에서 그 기기를 해제하면 즉시 끊긴다
- [ ] 인증하지 않은 채 연결만 유지하면 **한 바이트도 오지 않는다**
````

- [ ] **Step 2: 파이썬 클라이언트를 만든다**

`scripts/lan_e2e_client.py` — `cryptography` 패키지로 X25519·HKDF·ChaCha20-Poly1305 를
쓴다. 골든 벡터(`docs/ble-protocol/golden/e2ee-v2-sample.json`)를 먼저 대조해
구현이 맞는지 확인한 뒤 실제 연결에 들어간다.

```bash
python3 -m pip install cryptography websockets
python3 scripts/lan_e2e_client.py --selftest   # 골든 벡터 대조만
```

- [ ] **Step 3: 절차를 실제로 한 번 돌린다**

위 확인 항목을 전부 통과시킨다. 하나라도 실패하면 멈추고 원인을 찾는다.

- [ ] **Step 4: 커밋**

```bash
git add scripts/lan-e2e.md scripts/lan_e2e_client.py
git commit -m "test(lan): 하드웨어 없는 종단 검증 절차와 참조 클라이언트"
```

---

# 2부 — 펌웨어 (실기기 필요)

> 아래 코드는 **컴파일해 보지 않았다.** 문서화된 API 를 근거로 썼으므로 첫 빌드에서
> 시그니처가 어긋날 수 있다. 각 태스크는 빌드 확인부터 시작한다.

## Task 8: PlatformIO 골격 — **화면 없이 시리얼만**

> **재구성 2026-08-26.** 초안은 이 태스크가 화면 점등부터였다. 그러면 첫 실패에서
> "암호가 틀렸나 / WebSocket 배선이 틀렸나 / ST7789 설정이 틀렸나"를 **동시에**
> 의심하게 된다. v3 판은 디스플레이 컨트롤러가 ILI9341 이 아니라 ST7789 라
> 설정을 틀리면 화면이 아예 안 켜지고, 그 상태에서 프로토콜까지 의심하면 훨씬 힘들다.
>
> 그래서 순서를 바꾼다. **이 태스크는 디스플레이를 전혀 건드리지 않는다** — WiFi ·
> mDNS · WebSocket · monocypher · 페어링 · 재연결을 전부 **시리얼 출력만으로** 확정한다.
> 화면은 그 다음 태스크에서 붙인다. 실패하면 의심할 곳이 하나뿐이다.
>
> 하드웨어는 CYD 본체를 그대로 쓴다 — 별도 dev board 가 필요 없다. 실물 확인 결과
> 실크스크린이 `esp32-2432s028` 이고 칩은 ESP32 classic(WROOM-32) 이다(2026-08-26).

**Files:**
- Create: `firmware/cyd/platformio.ini`
- Create: `firmware/cyd/src/main.cpp`

- [ ] **Step 1: 보드 변종 — 확인 완료**

USB-C 와 마이크로 USB 가 **둘 다** 있는 판이므로 `esp32-2432S028Rv3` 다.
디스플레이 컨트롤러는 **ST7789** — 이 태스크에서는 쓰지 않지만 다음 태스크에서
이 값이 틀리면 화면이 안 켜진다.

> 참고: v3 는 커넥터가 둘이라 둘 다 꽂으면 `/dev/cu.usbserial-*` 가 두 개 잡힌다.
> 하나만 쓰면 된다. Arduino IDE 의 Serial Monitor 가 포트를 독점하므로,
> `pio` 로 업로드할 때는 그 창을 닫아야 한다.

- [ ] **Step 2: `platformio.ini` 를 쓴다 — 디스플레이 라이브러리 없이**

```ini
[env:cyd]
platform = espressif32
board = esp32-2432S028Rv3        ; 실물 확인됨(2026-08-26) — ST7789 판이다
framework = arduino
monitor_speed = 115200
board_build.partitions = huge_app.csv
lib_deps =
    links2004/WebSockets
    bblanchon/ArduinoJson
    tzapu/WiFiManager
; esp32_smartdisplay(LVGL)는 **일부러 넣지 않는다.** 이 태스크는 화면을 쓰지 않고,
; 넣으면 빌드 시간과 플래시만 늘면서 디버깅 면적이 커진다. 다음 태스크에서 더한다.
```

보드 정의를 쓰려면 저장소를 `platform_packages` 또는 `boards_dir` 로 잡는다 —
`rzeldent/platformio-espressif32-sunton` README 를 따른다.

- [ ] **Step 3: 살아 있다는 것만 확인하는 최소 스케치**

화면도 WiFi 도 아직 없다. 툴체인·보드 id·업로드 경로가 맞는지만 본다.

```cpp
#include <Arduino.h>

void setup() {
    Serial.begin(115200);
    delay(300);                       // USB CDC 가 붙을 시간
    Serial.println();
    Serial.printf("chip=%s rev=%d cores=%d\n",
                  ESP.getChipModel(), ESP.getChipRevision(), ESP.getChipCores());
    Serial.printf("flash=%u bytes  free heap=%u\n",
                  ESP.getFlashChipSize(), ESP.getFreeHeap());
    Serial.println("AI Agent Monitor — CYD 프로토콜 펌웨어 (화면 없음)");
}

void loop() {
    Serial.printf("alive  heap=%u\n", ESP.getFreeHeap());
    delay(5000);
}
```

> **제약 — `loop()` 한 바퀴는 30초를 넘겨 블로킹하지 않는다. Task 8~15 전부에
> 걸린다.**
>
> 맥은 30초마다 WebSocket Ping 을 보내고, 이 기기에게서 **무언가** 받은 지
> 90초가 지나면 사라진 것으로 보고 연결을 놓는다(`src-tauri/src/lan/server.rs`
> 의 `PING_INTERVAL` · `IDLE_TIMEOUT`). 그 Ping 에 Pong 을 돌려주는 것은
> **OS 의 TCP 스택이 아니라 `links2004/WebSockets` 라이브러리이고, 그것은
> `webSocket.loop()` 가 불릴 때만 응답한다.** 즉 메인 루프가 90초 넘게
> 블로킹하면 WiFi 도 소켓도 멀쩡한 보드가 맥에서 끊긴다 — **그리고 이 기기에는
> 이유를 설명할 화면도 키보드도 없다.** 이 전송을 만든 이유가 된 바로 그
> 비대칭이 여기서 사람을 문다.
>
> 그래서 예산은 90초가 아니라 **30초**다. Ping 한 번 놓치는 정도까지만 허용하는
> 값이고, 90초에 바짝 붙여 두면 전체 화면 갱신이 한 번 느려지는 날 곧바로
> 넘긴다. 지키는 방법:
>
> - 주기를 `delay()` 로 만들지 않는다 — `millis()` 비교로 넘긴다.
> - 블로킹이 불가피한 구간(WiFi 재접속, 전체 화면 갱신, NVS 쓰기)은 조각을 내고
>   사이사이에 `webSocket.loop()` 를 부른다.
> - Task 12 에서 WebSocket 이 붙은 뒤에는 `loop()` 한 바퀴의 최장 시간을 시리얼로
>   찍어 둔다. 그러면 이 제약이 깨지는 날이 눈에 보인다.
>
> **위 스케치의 `delay(5000)` 은 이 태스크 한정으로만 무해하다** — 아직 WebSocket
> 이 없기 때문이다. Task 12 에서 소켓이 붙는 순간 이 모양이 그대로 결함이 되므로,
> 그때 이 루프를 `millis()` 기반으로 바꾸는 것을 잊지 말라.

- [ ] **Step 4: 빌드하고 올린다**

```bash
cd firmware/cyd
pio run -t upload
pio device monitor
```

Expected — 시리얼에 다음이 보인다:

```
chip=ESP32 rev=… cores=2
flash=4194304 bytes  free heap=…
AI Agent Monitor — CYD 프로토콜 펌웨어 (화면 없음)
alive  heap=…
```

`chip=ESP32` 와 `flash=4194304`(4MB)가 스펙 2장의 값과 맞는지 확인한다.
**어긋나면 보드 id 나 파티션 설정이 틀린 것이고, 여기서 잡는 편이 나중보다 훨씬 싸다.**

업로드가 `Resource busy` 로 실패하면 Arduino IDE 의 Serial Monitor 가 포트를
잡고 있는 것이다 — 그 창만 닫으면 된다(IDE 전체를 끌 필요 없다).

- [ ] **Step 5: 커밋**

```bash
git add firmware/cyd/
git commit -m "feat(cyd): PlatformIO 골격 — 화면 없이 시리얼만"
```

> **다음 태스크로 넘기는 것**: 화면 점등(`esp32_smartdisplay` + LVGL + ST7789)은
> 프로토콜이 시리얼로 확정된 뒤에 붙인다. 그때 실패하면 의심할 곳이 디스플레이
> 하나뿐이다.

---

## Task 9: 설정 저장과 WiFi 포털

**Files:**
- Create: `firmware/cyd/src/config.h`, `config.cpp`

**Interfaces:**
- Produces:
  - `struct Config { String macHost; String token; }`
  - `bool configLoad(Config&)`, `void configSave(const Config&)`, `void configClearToken()`
  - `bool wifiConnectOrPortal(Config&)`

- [ ] **Step 1: 구현한다**

```cpp
// NVS 저장. 토큰은 **평문이다** — 플래시 암호화를 켜지 않는 한 기기를 주운
// 사람이 읽을 수 있다. iOS Keychain 과 다르며, 대응은 맥에서의 기기별
// 해제뿐이다(스펙 7.2).
#include <Preferences.h>
#include <WiFiManager.h>

static Preferences prefs;

bool configLoad(Config &c) {
    prefs.begin("aim", true);
    c.macHost = prefs.getString("machost", "");
    c.token = prefs.getString("token", "");
    prefs.end();
    return c.token.length() == 32;
}

void configSave(const Config &c) {
    prefs.begin("aim", false);
    prefs.putString("machost", c.macHost);
    prefs.putString("token", c.token);
    prefs.end();
}

void configClearToken() {
    prefs.begin("aim", false);
    prefs.remove("token");
    prefs.end();
}

/// WiFi 자격증명과 맥 주소를 같은 포털에서 받는다. 저항막 화면에 비밀번호를
/// 치는 것보다 폰으로 붙는 편이 낫다.
bool wifiConnectOrPortal(Config &c) {
    WiFiManager wm;
    WiFiManagerParameter host("machost", "맥 주소 (비우면 자동 탐색)",
                              c.macHost.c_str(), 40);
    wm.addParameter(&host);
    bool ok = wm.autoConnect("AIM-Setup");
    if (ok) {
        c.macHost = host.getValue();
        configSave(c);
    }
    return ok;
}
```

`lib_deps` 에 `tzapu/WiFiManager` 를 더한다.

> **`wifiConnectOrPortal()` 은 블로킹이다** — 포털이 열려 있는 동안 `loop()` 는
> 돌지 않는다. 여기서는 괜찮다: 이 함수는 부팅 때, **WebSocket 이 붙기 전에**
> 한 번 돈다. 그것이 Task 8 의 30초 예산을 어겨도 되는 유일한 근거다.
>
> Task 12 이후에 **런타임 중** 포털을 다시 여는 길을 만든다면(맥 주소 변경,
> WiFi 재설정), 포털을 열기 전에 WebSocket 을 명시적으로 끊어라. 끊지 않고 열면
> 맥은 90초 뒤에 그 연결을 죽은 것으로 처리하고, 포털에서 돌아온 기기는 자기가
> 왜 끊겼는지 모른 채 재연결을 시작한다.

- [ ] **Step 2: 확인한다**

- [ ] 첫 부팅에 `AIM-Setup` AP 가 뜬다
- [ ] 폰으로 붙어 SSID·비번·맥 주소를 넣으면 저장되고 재부팅 후에도 남는다

- [ ] **Step 3: 커밋**

```bash
git add firmware/cyd/src/config.*  firmware/cyd/platformio.ini
git commit -m "feat(cyd): NVS 설정과 WiFi 캡티브 포털"
```

---

## Task 10: monocypher 암호 계층과 골든 벡터 대조

**Files:**
- Create: `firmware/cyd/src/cryptov2.h`, `cryptov2.cpp`
- Create: `firmware/cyd/test/test_cryptov2.cpp`

**Interfaces:**
- Consumes: `docs/ble-protocol/golden/e2ee-v2-sample.json`
- Produces:
  - `void v2Transcript(const uint8_t cpk[32], const uint8_t spk[32], uint8_t out[64])`
  - `void v2DeriveSessionKeys(const uint8_t ss[32], const uint8_t *token, size_t tlen, const uint8_t *nonce, size_t nlen, uint8_t s2c[32], uint8_t c2s[32])`
  - `void v2CodeBinding(const char *code, const uint8_t tr[64], uint8_t out[32])`
  - `void v2SessionProof(const uint8_t *token, size_t tlen, const uint8_t *nonce, size_t nlen, const uint8_t tr[64], uint8_t out[32])`
  - `class SealedChannel { bool open(const uint8_t *frame, size_t len, uint8_t *out, size_t *outLen); size_t seal(...); }`

- [ ] **Step 1: 골든 벡터를 상수로 박은 테스트를 쓴다**

`docs/ble-protocol/golden/e2ee-v2-sample.json` 의 값을 그대로 옮긴다.
**세 언어가 같은 값을 내야 한다 — 이 테스트가 실패하면 프로토콜이 갈라진 것이다.**

```cpp
#include <unity.h>
#include "cryptov2.h"

// e2ee-v2-sample.json 에서 옮긴 값. 벡터를 갱신하면 여기도 갱신한다.
static const uint8_t SS[32]    = { /* 0x11 × 32 */ };
static const uint8_t CPK[32]   = { /* 0x22 × 32 */ };
static const uint8_t SPK[32]   = { /* 0x33 × 32 */ };
static const uint8_t NONCE[16] = { /* 0x44 × 16 */ };
static const uint8_t TOKEN[16] = { /* 0x55 × 16 */ };
static const char *CODE = "123456";
static const char *EXPECT_CODE_BINDING = "<골든 벡터의 code_binding>";
static const char *EXPECT_S2C          = "<골든 벡터의 k_s2c>";

void test_code_binding_matches_golden() {
    uint8_t tr[64], out[32];
    v2Transcript(CPK, SPK, tr);
    v2CodeBinding(CODE, tr, out);
    TEST_ASSERT_EQUAL_STRING(EXPECT_CODE_BINDING, toHex(out, 32).c_str());
}

void test_session_keys_match_golden() {
    uint8_t s2c[32], c2s[32];
    v2DeriveSessionKeys(SS, TOKEN, 16, NONCE, 16, s2c, c2s);
    TEST_ASSERT_EQUAL_STRING(EXPECT_S2C, toHex(s2c, 32).c_str());
}
```

- [ ] **Step 2: monocypher 를 넣고 구현한다**

`lib_deps` 에 monocypher 를 더하거나 `lib/monocypher/` 에 소스를 직접 둔다.

HKDF-SHA256 은 monocypher 에 없다 — HMAC-SHA256 은 있으므로 RFC 5869 를
그대로 구현한다(extract → expand, 32바이트 출력이면 T(1) 한 번이면 된다).

```cpp
// HKDF-SHA256, 출력 32바이트. RFC 5869.
// 출력이 해시 길이와 같으므로 expand 는 블록 한 번이면 끝난다.
static void hkdf32(const uint8_t *ikm, size_t ikmLen,
                   const uint8_t *salt, size_t saltLen,
                   const uint8_t *info, size_t infoLen,
                   uint8_t out[32]) {
    uint8_t prk[32];
    crypto_hmac_sha256(prk, salt, saltLen, ikm, ikmLen);   // extract
    // expand: T(1) = HMAC(PRK, info || 0x01)
    uint8_t buf[64];
    memcpy(buf, info, infoLen);
    buf[infoLen] = 0x01;
    crypto_hmac_sha256(out, prk, 32, buf, infoLen + 1);
    crypto_wipe(prk, 32);
    crypto_wipe(buf, sizeof buf);
}
```

`SealedChannel` 은 Rust·Swift 와 같은 규칙이다 —
논스 = `[0,0,0,0] || counter(BE)`, AAD = `"aim-v2"`,
프레임 = `counter(8) || ciphertext || tag(16)`,
**인증에 성공한 뒤에만 `lastRecv` 를 전진시킨다.**

- [ ] **Step 3: 테스트를 돌린다**

```bash
cd firmware/cyd && pio test -e cyd
```
Expected: 골든 벡터 대조 전부 통과

- [ ] **Step 4: 커밋**

```bash
git add firmware/cyd/src/cryptov2.* firmware/cyd/test/
git commit -m "feat(cyd): monocypher E2EE v2 — 골든 벡터로 Rust 와 대조"
```

---

## Task 11: 인증 상태 기계 (순수 함수)

**Files:**
- Create: `firmware/cyd/src/authfsm.h`, `authfsm.cpp`
- Create: `firmware/cyd/test/test_authfsm.cpp`

**Interfaces:**
- Produces:
  - `enum class AuthStep { SendHello2, SendAuth2, SendCode2, SendProof2, Subscribed, NeedsPairing, Failed }`
  - `AuthStep authInitialStep(bool hasToken, bool hasCode)`
  - `AuthStep authOnReply(const ReplyView &reply, bool hasToken, bool hasCode)`

**연결 코드 안에 상태 기계를 녹여넣지 않는다.** 순수 함수로 뽑아야 테스트로 고정된다 —
iOS 에서 이 결정이 연결 코드에 묻혀 있어 두 번 버그가 났다.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```cpp
/// 방금 입력한 코드가 저장된 토큰보다 우선한다. 사용자가 키패드로 코드를
/// 넣었다는 것은 "새로 페어링하겠다"는 명시적 의사다. 맥이 토큰을 이미
/// 폐기했다면 토큰 재인증은 반드시 거부되고, 그 사이 코드는 쓰이지도 못한다.
void test_fresh_code_wins_over_stored_token() {
    TEST_ASSERT_EQUAL(AuthStep::SendHello2, authInitialStep(true, true));
}

void test_token_without_code_reconnects() {
    TEST_ASSERT_EQUAL(AuthStep::SendAuth2, authInitialStep(true, false));
}

void test_no_token_no_code_needs_pairing() {
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authInitialStep(false, false));
}

/// 토큰이 거부되면 그것을 버리고 코드를 요구한다. **재시도하지 않는다** —
/// 코드 없이는 통과할 수 없는 재시도가 화면을 깜빡이게 한다.
void test_rejected_token_asks_for_a_code_without_retrying() {
    ReplyView r = parseReply("{\"ok\":false}");
    TEST_ASSERT_EQUAL(AuthStep::NeedsPairing, authOnReply(r, true, false));
}

/// 다운그레이드하지 않는다. v1 응답이 와도 v1 로 물러서지 않는다.
void test_never_downgrades_to_v1() {
    ReplyView r = parseReply("{\"ok\":false,\"await\":\"code\"}");  // v 필드 없음 = v1
    TEST_ASSERT_EQUAL(AuthStep::Failed, authOnReply(r, false, true));
}
```

- [ ] **Step 2: 실패를 확인한 뒤 구현한다**

```cpp
AuthStep authInitialStep(bool hasToken, bool hasCode) {
    // 코드가 있으면 토큰이 있어도 코드를 쓴다.
    if (hasCode) return AuthStep::SendHello2;
    if (hasToken) return AuthStep::SendAuth2;
    return AuthStep::NeedsPairing;
}
```

- [ ] **Step 3: 테스트를 돌리고 커밋한다**

```bash
cd firmware/cyd && pio test -e cyd
git add firmware/cyd/src/authfsm.* firmware/cyd/test/test_authfsm.cpp
git commit -m "feat(cyd): 인증 상태 기계를 순수 함수로 뽑는다"
```

---

## Task 12: WebSocket 연결과 mDNS 발견

**Files:**
- Create: `firmware/cyd/src/transport.h`, `transport.cpp`

**Interfaces:**
- Consumes: Task 9 의 `Config`, Task 10 의 `SealedChannel`, Task 11 의 `AuthStep`
- Produces: `class Transport { void begin(Config&); void loop(); bool authorized(); }`

- [ ] **Step 1: 구현한다**

발견 순서는 **mDNS → 저장된 IP → 수동 입력 화면**이다.

```cpp
#include <ESPmDNS.h>
#include <WebSocketsClient.h>

String discoverMac(const Config &c) {
    if (MDNS.begin("aim-cyd")) {
        int n = MDNS.queryService("aim", "tcp");
        if (n > 0) return MDNS.IP(0).toString();
    }
    return c.macHost;   // 포털에서 받은 값. 비어 있으면 수동 입력 화면으로.
}
```

**재연결은 지수 백오프**로 한다. 단 `NeedsPairing`/`Failed` 는 사용자가 코드를
다시 넣어야 풀리므로 **재시도하지 않고 키패드 화면에 머무른다.**

- [ ] **Step 2: 확인한다**

- [ ] mDNS 로 맥을 찾는다
- [ ] mDNS 를 끈 망에서도 저장된 IP 로 붙는다
- [ ] 맥을 껐다 켜면 백오프 후 다시 붙는다
- [ ] 맥에서 기기를 해제하면 키패드 화면으로 가고 **깜빡이지 않는다**

- [ ] **Step 3: 커밋**

```bash
git add firmware/cyd/src/transport.*
git commit -m "feat(cyd): WebSocket 연결과 mDNS 발견"
```

---

## Task 13: 한글 서브셋 폰트

**Files:**
- Create: `firmware/cyd/src/font_ko.c` (생성물)
- Create: `firmware/cyd/tools/build-font.sh`

- [ ] **Step 1: 쓰는 글자를 모은다**

UI 라벨에 실제로 쓰는 음절만 넣는다. 초안:

```
연결됨 연결중 끊김 대기 남음 시간 분 초 뒤 초기화 사용률 주간
페어링 코드 입력 남은 시도 회 설정 기기 오류 없음 활동 중 유휴 휴면
```

- [ ] **Step 2: 폰트를 굽는다**

```bash
npx lv_font_conv --font NotoSansKR-Regular.ttf \
  --size 16 --bpp 4 --format lvgl \
  --symbols "연결됨중끊김대기남음시간분초뒤초기화사용률주간페어링코드입력은시도회설정기器오류없활동유휴면" \
  --range 0x20-0x7F \
  -o src/font_ko.c
```

`--range 0x20-0x7F` 로 ASCII 를 함께 넣어 폰트를 하나로 유지한다.

- [ ] **Step 3: 크기를 확인한다**

```bash
ls -l src/font_ko.c
pio run    # 플래시 사용률을 본다
```
Expected: 폰트가 **50KB 이하**여야 한다. 넘으면 `--bpp 2` 로 낮춘다.

- [ ] **Step 4: 비ASCII 대체 처리를 넣는다**

프로젝트 이름에 폰트에 없는 글자가 오면 **대체 문자로 그린다.** 깨진 화면이나
크래시가 아니라 "읽을 수 없다"가 보이는 상태로 떨어져야 한다.

- [ ] **Step 5: 커밋**

```bash
git add firmware/cyd/src/font_ko.c firmware/cyd/tools/build-font.sh
git commit -m "feat(cyd): UI 라벨용 한글 서브셋 폰트"
```

---

## Task 14: 페어링 키패드 화면

**Files:**
- Create: `firmware/cyd/src/ui_pairing.h`, `ui_pairing.cpp`

- [ ] **Step 1: 구현한다**

LVGL `btnmatrix` 로 3×4 키패드를 만든다. 남은 시간과 남은 시도 횟수를 함께 보여준다.

```cpp
static const char *KEYS[] = {"1","2","3","\n","4","5","6","\n",
                             "7","8","9","\n","←","0","확인",""};
```

- [ ] **Step 2: 확인한다**

- [ ] 저항막 화면에서 오입력 없이 눌린다
- [ ] 6자리를 채우면 확인이 활성화된다
- [ ] 틀리면 남은 시도 횟수가 줄어든다
- [ ] 5회를 다 쓰면 "맥에서 페어링을 다시 시작하세요" 가 보인다

- [ ] **Step 3: 커밋**

```bash
git add firmware/cyd/src/ui_pairing.*
git commit -m "feat(cyd): 6자리 코드 키패드 화면"
```

---

## Task 15: 카드 화면과 세션 목록

**Files:**
- Create: `firmware/cyd/src/ui_cards.h/.cpp`, `ui_sessions.h/.cpp`

- [ ] **Step 1: 카드 화면을 만든다**

에이전트마다 카드 하나. 이름 / 사용률 바 + 퍼센트 / tok/s / 리셋 카운트다운.
주간 값이 있으면 주간 바도.

**색 경계는 맥·iOS 와 같아야 한다 — 70% 와 90%.** 다르면 미러가 아니다.

- [ ] **Step 2: 세션 목록을 만든다**

LVGL `list` 가 스크롤을 해결한다. 프로젝트 이름 / 모델 / 상태 점 / 상대 시각.
**최근 활동순 정렬** — iOS 미러와 같은 규칙이다.

- [ ] **Step 3: 화면 전환을 붙인다**

터치로 두 화면을 오간다.

- [ ] **Step 4: 맥 화면과 나란히 놓고 비교한다**

- [ ] tok/s 가 같은 값·같은 표기다
- [ ] 사용률 퍼센트와 바 색이 같다 (70%/90% 경계 포함)
- [ ] 주간 바는 주간 값이 있을 때만 나온다
- [ ] 리셋 카운트다운이 1초마다 줄어든다
- [ ] 세션이 최근 활동순이고 상대 시각이 갱신된다
- [ ] idle/dormant 점 색과 상태 단어가 같다
- [ ] 맥에서 에이전트를 끄면 CYD 에서도 사라진다

마지막 줄이 이 태스크의 완료 판정이다.

- [ ] **Step 5: 커밋**

```bash
git add firmware/cyd/src/ui_*
git commit -m "feat(cyd): 카드 화면과 세션 목록"
```

---

## Task 16: 실기기 검증 절차 문서화

**Files:**
- Modify: `docs/ble-protocol/DEVICE-TEST.md`

- [ ] **Step 1: CYD 절을 추가한다**

```markdown
## 8. CYD 확인

### 8-0. 보드 변종

USB 커넥터를 본다 — 마이크로 USB 만: `esp32-2432S028R` /
USB-C: `v2` / 둘 다: `v3`(디스플레이가 **ST7789**). `platformio.ini` 의
`board` 를 맞춘다. 화면이 안 켜지면 이것부터 의심한다.

### 8-1. 확인 항목

- [ ] WiFi 포털에서 자격증명과 맥 주소를 넣으면 저장된다
- [ ] mDNS 로 맥을 찾는다. 끈 망에서는 저장된 IP 로 붙는다
- [ ] 키패드로 6자리를 넣으면 페어링된다
- [ ] 재부팅 후 자동 재인증된다
- [ ] 맥에서 해제하면 즉시 키패드 화면으로 가고 **깜빡이지 않는다**
- [ ] 맥을 껐다 켜면 백오프 후 다시 붙는다
- [ ] BLE·네트워크 기기와 **동시에** 붙어 있을 수 있다
- [ ] 기기 목록에 CYD 가 하나의 peer 로 보이고 개별 해제가 된다
- [ ] 한글 라벨이 깨지지 않는다
- [ ] 비ASCII 프로젝트 이름이 대체 문자로 떨어진다(크래시 아님)
- [ ] **맥 Detail 창과 나란히 놓고 시각적으로 일치한다** ← 완료 판정

### 8-2. 평문이 나가지 않는지 본다

같은 망의 다른 맥에서:

```bash
sudo tcpdump -i any -A 'tcp port 4320'
```

- [ ] 6자리 코드가 나타나지 않는다
- [ ] 32자리 토큰이 나타나지 않는다
- [ ] 스냅샷 페이로드가 `{` 로 시작하지 않는다
```

- [ ] **Step 2: 커밋**

```bash
git add docs/ble-protocol/DEVICE-TEST.md
git commit -m "docs: CYD 실기기 검증 절차"
```

---

## 자체 리뷰

**1. 스펙 커버리지**

| 스펙 절 | 태스크 |
|---|---|
| §2 하드웨어, 변종 확인 | Task 8 Step 1, Task 16 §8-0 |
| §2.1 SPI 버스 함정 회피 | Task 13 (SD 를 안 쓴다) |
| §3 LAN 우선 근거 | Task 7 |
| §3.1 BLE 의 MTU 결함 | **이 계획의 범위 밖** — E2EE 계획 Task 8 이 리팩터링으로 함께 해결한다 |
| §4.1 전송·포트·기본 꺼짐 | Task 1, 2, 6 |
| §4.2 연결 1개 = 세션 1개 | Task 2, 3 |
| §4.3 프레임(텍스트/바이너리, 청킹 없음) | Task 3, 4 |
| §4.4 발견 | Task 5, 12 |
| §4.5 UI | Task 6 |
| §5.1 툴체인 | Task 8 |
| §5.2 WiFi·맥 주소 | Task 9 |
| §5.3 화면 3장 | Task 14, 15 |
| §5.4 폰트 | Task 13 |
| §5.5 인증 상태 기계 | Task 11 |
| §5.6 토큰 저장 | Task 9 |
| §7 보안 (포트 점유 오류 표시 포함) | Task 6 Step 2 마지막 항목 |
| §8 테스트 | Task 7, 16 |
| §9 2단계 BLE | **이 계획의 범위 밖** — 별도 계획 |

**2. 시그니처 일관성**

- `CentralId` 형식 `lan:<serial>` 이 Task 2·3 에서 같다
- `Outbound::Binary(CentralId, Vec<u8>)` 가 Task 2·4 에서 같다
- `AuthOutcome { payload, now_authorized }` — `network/mod.rs` 의 기존
  `AuthOutcome` 에는 `granted` 가 더 있다. **LAN 은 v2 전용이라 `granted` 가
  필요 없다고 판단해 뺐다.** 구현자가 기존 것을 재사용하려 하면 필드가 달라
  혼동할 수 있으니, `lan::AuthOutcome` 이라는 별도 타입임을 분명히 한다.
- `SealedChannel` 의 seal/open 규칙이 Rust·Swift·C 세 곳에서 같다
  (논스 구성, AAD, 프레임 형식, 인증 후 카운터 전진)

**3. 알려진 미해결**

- Task 4 의 테스트에 v2 페어링 준비 코드가 생략돼 있다. `pairing.rs` 의 v2
  테스트(E2EE 계획 Task 6)가 쓰는 `V2Client` 를 테스트 헬퍼로 꺼내 재사용해야
  한다. 지금 없는 헬퍼 이름을 지어내는 것보다 낫다고 판단했다.
- 펌웨어 코드는 컴파일 검증되지 않았다(2부 머리말).
- `firmware/` 디렉터리가 Tauri 빌드에 섞이지 않는지 확인해야 한다 —
  `.gitignore` 와 `tauri.conf.json` 의 `beforeBuildCommand` 를 본다.
