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
//! **시간 상한이 걸리는 곳은 셋이다.**
//!
//! 1. 조용해진 피어 — `IDLE_TIMEOUT`(90초). 마지막으로 무언가 받은 지 그만큼
//!    지나면 사라진 것으로 본다.
//! 2. 송신 경로가 밀린 피어 — 큐가 차서 지워지면 하트비트를 보내지 못하고,
//!    그 순간 연결을 놓는다.
//! 3. **인가되지 않은 피어 — `AUTH_DEADLINE`(150초).** 붙은 지 그만큼 지났는데도
//!    인가되지 않았으면 놓는다.
//!
//! 셋 중 앞의 둘은 피어가 하기 나름이다. 1번은 계속 말하면 오지 않고, 2번은
//! 상대가 읽지 않아 커널 송신 버퍼까지 차야 오는데 그 시간은 우리가 얼마나
//! 보내느냐에 달려 있다 — 하트비트만 흐르던 때(초당 1바이트 미만)는 수 주,
//! 스냅샷이 흐르는 지금은 초 단위다. 즉 **말은 하면서 읽지 않는 피어에게
//! 앞의 둘은 상한이 아니라 산수였다.** 산수는 트래픽이 바뀌면 함께 바뀐다.
//!
//! 3번만 상대가 무엇을 보내든 읽든 늘어나지 않는다. 그것이 이 시한의 존재
//! 이유이고, **인가되지 않은** 연결의 자리가 실제로 회수된다는 유일한 근거다 —
//! 그것도 **`handle` 에 들어온 연결에 한해서** 그렇다. 자리는 `upgrade()` 가
//! `on_upgrade` 보다 먼저 잡고(업그레이드가 실패해도 자리가 새지 않게 한 Task 2
//! 의 결정) 시한은 `handle()` 안에서 무장되므로, 101 응답을 주고받는 사이의 짧은
//! 구간은 자리를 쥐면서 어떤 시한도 받지 않는다. 그 구간은 수십 바이트의 왕복이라
//! 실질적으로 닫혀 있다.
//!
//! **인가된 연결에는 셋 중 어느 것도 절대 상한이 아니다.** 시한은 걷혔고
//! (`Deadline::Lift`) 나머지 둘은 위에서 본 대로 피어가 조종한다. 그쪽 자리를
//! 돌려주는 것은 시간이 아니라 **사용자의 결정**이다: 토글을 끄거나 그 기기를
//! 해제하면(`lan::LanBridge::drop_sessions`) `Outbound::Close` 가 나가고, 그것을
//! 받은 핸들러가 루프를 빠져나가며 `Slot` 을 떨어뜨린다. 인가는 사용자가 준
//! 것이므로 그것을 거두는 것도 사용자여야 한다는 뜻이기도 하다.
//!
//! "왜 하필 150초인가"는 `AUTH_DEADLINE` 의 doc 에 있다 — 그 숫자는 페어링
//! 코드의 수명에 묶여 있어서, 한쪽만 바꾸면 조용히 깨진다.

use crate::ble::peripheral::CentralId;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

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

/// 인증 시한 — **인가되지 않은 연결이 자리를 붙들 수 있는 절대 상한**이다.
/// 연결이 성립한 순간부터 재고, 상대가 무엇을 보내든 읽든 늘어나지 않는다.
///
/// **왜 늘어나면 안 되나.** 늘어나는 순간 그것은 상한이 아니라 산수가 된다.
/// `IDLE_TIMEOUT` 은 말을 계속 하면 오지 않고, 송신 큐 상한은 우리가 얼마나
/// 보내느냐에 따라 수 주에서 수 초 사이를 오간다. 둘 다 피어가 조종할 수
/// 있으므로, 자리 여덟 개가 회수된다는 보장을 그 위에 세울 수 없다.
///
/// **왜 `CODE_TTL` 보다 긴가 — 여기가 이 숫자의 전부다.** 페어링 전 구간이
/// 인가되지 않은 연결 위에서 일어난다: CYD 가 붙어 `HELLO2` 를 보내고, 맥
/// 화면의 여섯 자리를 사람이 읽어 기기 키패드로 넣고, 그제서야 `CODE2` 가
/// 나간다(스펙 6장의 흐름도). 그 **사람 시간**의 예산을 정해 둔 것이 페어링
/// 코드의 수명 `pairing::CODE_TTL`(120초)이다.
///
/// 시한이 그보다 짧으면 사람이 키패드 앞에 서 있는 동안 연결이 끊긴다. 다시
/// 붙은 연결에는 `HELLO2` 트랜스크립트가 없으므로(연결 하나 = 세션 하나),
/// 방금 제대로 입력한 코드가 `Rejected` 로 돌아온다 — 사용자에게는 "맞는
/// 코드인데 틀렸다고 한다"로 보이고, 이 앱은 로그를 남기지 않으므로 원인을
/// 알 방법이 없다. 그래서 120초에 왕복·재접속 여유 30초를 얹었다.
///
/// 30초를 더 얹어도 상한의 성질은 그대로다: 자리 하나가 최악 150초마다
/// 회수되는 것과 영영 회수되지 않는 것의 차이가 이 상수의 전부이고,
/// 150 과 60 의 차이는 그다음 문제다.
///
/// 줄이고 싶어지면 `CODE_TTL` 을 먼저 보라 — 둘의 관계는
/// `the_auth_deadline_outlives_the_code_a_human_is_typing` 이 못박아 둔다.
const AUTH_DEADLINE: Duration = Duration::from_secs(150);

/// 하트비트와 인증 시한의 타이밍. 상수를 그대로 쓰지 않고 값으로 들고 다니는
/// 이유는 90초·150초를 실제로 기다리지 않고도 "조용해진 피어를 정말 놓는가",
/// "인가되지 않은 피어를 정말 놓는가"를 테스트가 확인할 수 있게 하기
/// 위해서다 — `port` 를 인자로 받는 것과 같은 이유다.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub ping: Duration,
    pub idle: Duration,
    pub auth: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { ping: PING_INTERVAL, idle: IDLE_TIMEOUT, auth: AUTH_DEADLINE }
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

/// 인증 시한이 다다랐을 때의 판단. `Heartbeat` 와 같은 이유로 연결 코드에서
/// 떼어 두었다 — 이것은 "무엇이 자리를 계속 쓸 자격이 있는가"라는 정책이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    /// 아직 인가되지 않았다 — 놓는다.
    Drop,
    /// 이미 인가됐다. 인가된 연결에는 시한이 없으므로 **다시 재지 않는다.**
    ///
    /// 한 번 걷힌 시한은 돌아오지 않는다. 그래서 **인가된 연결의 자리를 돌려주는
    /// 것은 시한이 아니다** — 상대가 끊거나, 조용해져 `IDLE_TIMEOUT` 에 걸리거나,
    /// 송신 큐가 밀리거나, 토글이 꺼지거나, 사용자가 그 기기를 해제하는 것이다.
    ///
    /// 마지막 것이 `lan::LanBridge::drop_sessions` 다. 해제된 central 을 목록에서
    /// 지우고 `Outbound::Close` 를 보내, 연결 핸들러가 루프를 빠져나가며 `Slot` 을
    /// 떨어뜨리게 한다. 그 배선이 없던 동안(`lib.rs::persist_and_drop` 이 BLE·
    /// network 두 브리지만 불렀다)에는 해제된 기기의 연결이 시한도 없이 자리에
    /// 남았다 — 바이트는 한 톨도 나가지 않았지만 자리는 쥐고 있었다.
    ///
    /// 바이트를 막는 것은 `LanBridge::snapshot_targets` 의 `is_authorized`
    /// 필터다. 봉인 지점(`lan::seal_for`)에도 같은 검사가 있지만 그것은 두 번째
    /// 자물쇠이고, 오늘 실제로 문을 잠그는 것은 바깥 필터다.
    Lift,
}

/// 시한이 보는 것은 인가 여부 **하나뿐**이다. 여기에 "그래도 최근에 말은
/// 했으니까" 같은 조건을 더하는 순간, 시한은 다시 피어가 조종할 수 있는 값이
/// 된다(`AUTH_DEADLINE` 의 doc).
fn deadline(authorized: bool) -> Deadline {
    if authorized {
        Deadline::Lift
    } else {
        Deadline::Drop
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
    /// 리스너가 실제로 떴다 — `bind` 가 성공한 **뒤에** 나간다.
    ///
    /// 이 통지가 따로 있는 이유는 mDNS 광고 때문이다. 토글(`enabled`)은 bind
    /// 성공을 뜻하지 않는다 — `BindFailed` 는 `enabled` 를 켠 채로 리스너만
    /// 없앤다. 광고를 토글에 매달면 포트가 열리지 않은 맥이 계속 광고되고 CYD 는
    /// 죽은 포트로 걸어간다(`LanBridge::advertise` 의 doc).
    ///
    /// **어느 리스너인지 함께 싣는다** — `BindFailed` 와 같은 이유다.
    Listening { generation: u64 },
    /// mDNS 게시가 **시작한 뒤에** 실패했다(멀티캐스트 차단 등).
    ///
    /// **이 변이만 리스너가 아니라 게시에서 온다.** 별도 통로를 파지 않은 이유는,
    /// LAN 전송이 배선(`lib.rs`)으로 무언가를 올리는 길이 이 채널 하나이고 그
    /// 배선이 이미 프레임 아닌 이벤트마다 패널을 다시 그리기 때문이다. 통로를
    /// 하나 더 만들면 그 알림 경로를 처음부터 다시 엮어야 한다.
    ///
    /// 세대를 싣는 이유도 같다 — 게시는 리스너 세대에 묶여 있으므로, 낡은 데몬이
    /// 늦게 올린 실패가 지금의 광고를 탓하면 안 된다.
    AdvertiseFailed { generation: u64, message: String },
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

/// 서버 태스크가 소유한 채널로 전달되는 항목. 대부분은 "이 central 에게 보낼
/// 것"이지만 `Authorized` 만 다르다 — 나가는 바이트가 없는 내부 통지다.
///
/// 같은 채널에 태우는 이유는 그 채널의 소비자(펌프)가 바깥에서 `sinks` 를
/// 만지는 **유일한** 지점이기 때문이다. 통지용 채널을 따로 두면 `sinks` 를
/// 만지는 곳이 둘이 되고, 그 둘 사이의 순서를 아무도 보장하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Text(CentralId, Vec<u8>),
    Binary(CentralId, Vec<u8>),
    Close(CentralId),
    /// 이 연결이 인가됐다 — 인증 시한을 건다(`Deadline::Lift`). 앱의 이벤트
    /// 루프가 `handle_auth` 결과를 보고 보낸다. **상대에게는 아무것도 나가지
    /// 않는다**: 우리가 누구를 인가했는지는 낯선 기기가 알 일이 아니다.
    Authorized(CentralId),
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

/// 살아 있는 연결 하나가 서버 쪽에 남기는 것.
///
/// 인가 여부가 **송신 큐 옆에** 있는 이유는, 그것을 알아야 하는 쪽(연결
/// 핸들러의 인증 시한)과 그것을 아는 쪽(앱의 이벤트 루프 → 펌프)이 서로 다른
/// 태스크이기 때문이다. 핸들러는 시한이 다다랐을 때 **한 번만** 읽으므로
/// 깨울 필요가 없고, 그래서 채널이 아니라 원자값이면 충분하다.
#[derive(Clone)]
struct Conn {
    tx: Sender<Message>,
    authorized: Arc<AtomicBool>,
}

/// central 별 연결 상태. 연결이 하나 생기면 하나 만들고, 그 연결이 끝나면 지운다.
/// 이 규칙이 어긋나면 끊긴 기기의 큐가 계속 쌓이거나(누수), 붙어 있는 기기에
/// 스냅샷이 나가지 않는다. 연결 코드 안에 두면 그 규칙만 따로 확인할 방법이
/// 없어서 별도 타입으로 뺐다.
#[derive(Default)]
struct Sinks(Mutex<HashMap<CentralId, Conn>>);

impl Sinks {
    fn insert(&self, id: CentralId, conn: Conn) {
        self.0.lock().unwrap().insert(id, conn);
    }

    fn remove(&self, id: &CentralId) {
        self.0.lock().unwrap().remove(id);
    }

    /// 잠금을 쥔 채로 보내지 않는다 — 송신은 await 를 탈 수 있고, 그 사이에
    /// 새 연결이 자기 큐를 등록하지 못하면 안 된다.
    fn get(&self, id: &CentralId) -> Option<Conn> {
        self.0.lock().unwrap().get(id).cloned()
    }

    /// 이 연결을 인가된 것으로 표시한다. 인증 시한이 보는 유일한 값이다.
    ///
    /// **모르는 id 는 아무 일도 하지 않는다.** 이미 끝난 연결의 늦은 통지가
    /// 다음 연결에 옮겨붙으면, 낯선 기기가 남의 인가로 시한 없는 자리를 얻는다.
    /// 세션 id 는 프로세스 전역에서 반복되지 않으므로(`next_central_id`) 실제로
    /// 옮겨붙지는 않지만, 그 성질에 기대는 코드를 하나 더 만들지 않는다.
    fn mark_authorized(&self, id: &CentralId) {
        if let Some(c) = self.0.lock().unwrap().get(id) {
            c.authorized.store(true, Ordering::Relaxed);
        }
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
    /// 인증 응답·봉인 스냅샷·인가 통지가 지나는 통로. 여기만 상한이 없는
    /// 이유는 낯선 기기가 아니라 앱 자신(틱 루프는 초당 하나, 인증 응답은
    /// 프레임당 하나)만 쓰기 때문이다 — 바깥에서 닿지 않는다. 상대가 읽지
    /// 않아 밀리는 것은 그 뒤의 central 별 큐(`SINK_QUEUE`)가 받아 낸다.
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
    // **bind 뒤다.** 이 통지 하나가 mDNS 광고를 켠다 — 먼저 보내면 광고가 아직
    // 열리지 않은(그리고 영영 열리지 않을 수도 있는) 포트를 가리킨다.
    let _ = events.send(ServerEvent::Listening { generation }).await;

    // 송신 펌프 — central 별 큐로 넘긴다. 별도 태스크인 이유는 `axum::serve` 가
    // 이 태스크를 끝까지 점유하기 때문이다.
    let pump_state = state.clone();
    let pump = tokio::spawn(async move {
        while let Some(item) = outbound.recv().await {
            match route(item) {
                Routed::Frame(id, msg) => {
                    push_to_sink(&pump_state.sinks, &id, msg);
                }
                Routed::Authorized(id) => pump_state.sinks.mark_authorized(&id),
            }
        }
    });

    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(wait_shutdown(shutdown))
        .await;
    pump.abort();
    tracing::info!(port, "LAN 미러 서버 종료");
}

/// 펌프가 항목 하나를 보고 내린 결론.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Routed {
    /// 이 central 에게 이 프레임을 보낸다.
    Frame(CentralId, Message),
    /// 이 central 의 인증 시한만 건다. 나가는 바이트는 없다.
    Authorized(CentralId),
}

/// 항목 하나를 어디로 보낼지 정한다. 순수 함수라 펌프 태스크와 떼어 확인할
/// 수 있고, **무엇이 상대에게 나가고 무엇이 나가지 않는가**가 한 곳에 모인다 —
/// 인가 통지가 실수로 프레임이 되어 나가면, 우리가 누구를 인가했는지 같은
/// WiFi 의 아무에게나 알려주는 셈이다.
fn route(item: Outbound) -> Routed {
    match item {
        Outbound::Text(id, b) => {
            Routed::Frame(id, Message::Text(String::from_utf8_lossy(&b).into_owned()))
        }
        Outbound::Binary(id, b) => Routed::Frame(id, Message::Binary(b)),
        Outbound::Close(id) => Routed::Frame(id, Message::Close(None)),
        Outbound::Authorized(id) => Routed::Authorized(id),
    }
}

/// 이 central 의 큐에 프레임을 넣는다. 큐가 밀려 있으면 **큐를 지워 연결을
/// 놓는다** — 상대가 읽지 않는데 계속 쌓으면 그 자체가 메모리 증가 레버다.
/// `false` 는 "이 central 에게 더 보낼 수 없다"(모르는 id 이거나 방금 놓았다).
fn push_to_sink(sinks: &Sinks, id: &CentralId, msg: Message) -> bool {
    let Some(c) = sinks.get(id) else {
        return false;
    };
    if c.tx.try_send(msg).is_err() {
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
    // 인가 여부는 앱의 이벤트 루프가 `Outbound::Authorized` 로 알려준다. 등록을
    // `Connected` 를 올리기 **전에** 해 두는 것이 중요하다 — 순서가 반대면
    // 인증이 아주 빨리 끝난 기기의 통지가 아무 데도 닿지 않는다.
    let authorized = Arc::new(AtomicBool::new(false));
    state
        .sinks
        .insert(id.clone(), Conn { tx: sink_tx, authorized: authorized.clone() });

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

    // 인증 시한. **연결이 성립한 이 순간부터** 재는 고정된 시각이고, 루프가
    // 몇 바퀴를 돌든 다시 계산되지 않는다 — `sleep_until` 에 절대 시각을
    // 주는 것이 그 뜻이다. 상대가 무엇을 보내든 읽든 이 시각은 움직이지
    // 않는다(`AUTH_DEADLINE` 의 doc).
    let auth_deadline = tokio::time::Instant::now() + state.timing.auth;
    // 시한이 이미 걷혔는가. 한 번 걷히면 다시 재지 않으므로, 이 플래그가
    // 아래 가지를 영구히 끈다(끄지 않으면 지난 시각의 `sleep_until` 이 매번
    // 즉시 깨어나 루프가 바쁘게 돈다).
    let mut deadline_lifted = false;

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
            // **인가되지 않은 연결의 절대 시한.** 위의 두 시간 상한은 피어가
            // 조종할 수 있다 — 말을 계속 하면 무응답 판정이 오지 않고, 큐가
            // 차는 시점은 우리가 얼마나 보내느냐에 달렸다. 이 가지만 그렇지
            // 않고, 그래서 자리 여덟 개가 실제로 회수된다는 근거가 여기 있다.
            _ = tokio::time::sleep_until(auth_deadline), if !deadline_lifted => {
                match deadline(authorized.load(Ordering::Relaxed)) {
                    Deadline::Drop => {
                        tracing::info!(id = %id.0, "LAN 인증 시한이 지났다 — 연결을 놓는다");
                        break;
                    }
                    Deadline::Lift => deadline_lifted = true,
                }
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

    /// 닫기 핸드셰이크를 **규약대로 되받는** 프레임. 서버가 `Close` 를 보내면
    /// 정상적인 클라이언트는 이것을 돌려주고, 그것을 받아야 서버 쪽 상태가
    /// `CloseAcknowledged` 로 넘어가 다음 읽기가 스트림의 끝이 된다 — 즉 연결
    /// 핸들러가 루프를 빠져나가 `Slot` 을 떨어뜨린다. 되받지 않는 기기(전원이
    /// 뽑혔다)는 그대로 `IDLE_TIMEOUT` 이나 다음 하트비트 송신 실패로 걸린다.
    pub(crate) fn masked_close_frame() -> Vec<u8> {
        // FIN + opcode 8, 마스크 비트 + 길이 0. 본문 없는 Close 도 유효하다.
        vec![0x88, 0x80, 0x37, 0xfa, 0x21, 0x3d]
    }

    /// 서버→클라이언트 프레임 하나를 읽어 `(첫 바이트, 본문)` 으로 돌려준다.
    /// 서버 프레임은 마스킹하지 않는다(RFC 6455). 길이는 7비트와 16비트 두
    /// 형태만 다룬다 — 어느 쪽으로 나가든 `MAX_FRAME_BYTES`(64 KiB) 안이라
    /// 64비트 형태는 나올 수 없다.
    pub(crate) async fn read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        s.read_exact(&mut head).await.expect("프레임 머리를 읽지 못했다");
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
        (head[0], payload)
    }

    /// 텍스트 프레임 하나(인증 응답)를 읽는다.
    pub(crate) async fn read_text_frame(s: &mut TcpStream) -> Vec<u8> {
        let (opcode, payload) = read_frame(s).await;
        assert_eq!(opcode, 0x81, "텍스트 프레임(FIN+opcode 1)이어야 한다");
        payload
    }

    /// 바이너리 프레임 하나(봉인된 스냅샷)를 읽는다. **텍스트로 받으면 안
    /// 된다** — 봉인 프레임은 UTF-8 이 아니라, 텍스트로 옮기는 순간 손실된다.
    pub(crate) async fn read_binary_frame(s: &mut TcpStream) -> Vec<u8> {
        let (opcode, payload) = read_frame(s).await;
        assert_eq!(opcode, 0x82, "바이너리 프레임(FIN+opcode 2)이어야 한다");
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

    /// 아무도 읽지 않는 큐를 가진 연결 하나. 실제 연결에서 writer 태스크가
    /// 멈춰 있는 상태와 같다. `_rx` 를 함께 돌려주는 이유는 그것을 떨어뜨리면
    /// 채널이 닫혀 `try_send` 가 큐 상한과 무관하게 실패하기 때문이다.
    fn conn() -> (Conn, Receiver<Message>) {
        let (tx, rx) = tokio::sync::mpsc::channel(SINK_QUEUE);
        (Conn { tx, authorized: Arc::new(AtomicBool::new(false)) }, rx)
    }

    #[test]
    fn text_outbound_becomes_a_text_frame() {
        let routed = route(Outbound::Text(central_id(3), b"{\"a\":1}".to_vec()));
        assert_eq!(
            routed,
            Routed::Frame(central_id(3), Message::Text("{\"a\":1}".to_string()))
        );
    }

    #[test]
    fn binary_outbound_stays_binary() {
        // 봉인된 스냅샷은 UTF-8 이 아니다. 텍스트로 옮기면 손실된다.
        let sealed = vec![0x00, 0xff, 0xfe];
        assert_eq!(
            route(Outbound::Binary(central_id(0), sealed.clone())),
            Routed::Frame(central_id(0), Message::Binary(sealed))
        );
    }

    #[test]
    fn close_outbound_becomes_a_close_frame() {
        assert_eq!(
            route(Outbound::Close(central_id(0))),
            Routed::Frame(central_id(0), Message::Close(None))
        );
    }

    /// 인가 통지는 프레임이 **아니다.** 여기서 프레임으로 새면 우리가 누구를
    /// 인가했는지가 같은 WiFi 의 아무에게나 나간다.
    #[test]
    fn an_authorization_notice_never_becomes_a_frame() {
        assert_eq!(
            route(Outbound::Authorized(central_id(2))),
            Routed::Authorized(central_id(2))
        );
    }

    /// 큐는 연결이 생길 때 만들어지고 끝날 때 사라진다. 이 규칙이 깨지면
    /// 끊긴 기기의 큐가 남아 계속 쌓인다.
    #[test]
    fn a_sink_lives_exactly_as_long_as_its_connection() {
        let sinks = Sinks::default();
        let (c, _rx) = conn();
        let id = central_id(0);

        assert_eq!(sinks.len(), 0);
        sinks.insert(id.clone(), c);
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
        let (c, _rx) = conn();
        let id = central_id(0);
        sinks.insert(id.clone(), c);

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

    /// 다음 이벤트. **`Listening` 은 건너뛴다.**
    ///
    /// 리스너가 떴다는 통지는 거의 모든 테스트에서 관심 밖의 서두다. 그것 하나
    /// 때문에 스무 개의 테스트가 첫 이벤트를 한 번씩 버리게 만들면 정작 각 테스트가
    /// 무엇을 보고 있는지가 흐려진다. 그 이벤트 자체는
    /// `a_listening_event_arrives_only_after_the_port_is_open` 이 본다.
    async fn next_event(rx: &mut Receiver<ServerEvent>) -> ServerEvent {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("이벤트가 오지 않았다")
                .unwrap();
            if !matches!(ev, ServerEvent::Listening { .. }) {
                return ev;
            }
        }
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

    /// **`Close` 가 자리를 실제로 돌려주는가 (Task 4b).**
    ///
    /// 프레임을 보내는 것과 `Slot` 이 떨어지는 것은 다른 일이다 — 자리는 핸들러가
    /// **끝나야** 돌아온다(`handle` 의 `_slot`). 그래서 `Disconnected` 하나만
    /// 보면 부족하다: 그 이벤트는 핸들러가 반환하기 직전에 나가므로 "루프는
    /// 빠져나왔다"까지만 말한다. 자리가 돌아왔다는 증거는 **거절당하던 새 연결이
    /// 다시 받아들여지는 것**뿐이다.
    ///
    /// 그래서 여덟 자리를 전부 채워 503 을 확인한 뒤 하나만 `Close` 로 닫는다.
    /// 이 경로가 없으면 해제된 기기가 자리를 영영 쥐고, 인가된 연결에는 그것을
    /// 되돌려 줄 시간 상한이 없다(`Deadline::Lift`).
    #[tokio::test]
    async fn closing_a_connection_gives_its_slot_back() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = listener(port, tx);
        wait_until_listening(port).await;

        let mut live = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            live.push(handshake(port).await);
            ids.push(next_connected(&mut rx).await);
        }
        let (_refused, head) = try_handshake(port).await;
        assert!(head.starts_with("HTTP/1.1 503"), "자리가 다 차 있어야 한다: {head}");

        // 앱이 언페어링에서 하는 일 그대로(`LanBridge::drop_sessions`).
        handle.outbound.send(Outbound::Close(ids[0].clone())).unwrap();

        let (opcode, _) = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut live[0]))
            .await
            .expect("Close 를 보냈는데 프레임이 소켓으로 나오지 않았다");
        assert_eq!(opcode, 0x88, "Close 프레임(FIN+opcode 8)이어야 한다");

        // 규약대로 되받는다 — 그래야 서버가 닫기 핸드셰이크를 끝낸다.
        live[0].write_all(&masked_close_frame()).await.unwrap();
        assert_eq!(next_event(&mut rx).await, ServerEvent::Disconnected(ids[0].clone()));

        // `Disconnected` 와 `_slot` 이 떨어지는 순간 사이에는 아주 짧은 틈이 있다.
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
        panic!("Close 로 닫은 연결이 자리를 돌려주지 않았다");
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

    /// mDNS 광고는 이 통지 하나에 매달려 있다(`LanBridge::advertise`). 그래서 두
    /// 가지를 본다: **세대가 실려 나가는가**(낡은 통지로 광고가 켜지면 안 된다)와,
    /// **통지가 bind 보다 뒤인가**. 뒤가 아니면 광고가 아직 열리지 않은 포트를
    /// 가리키게 되고, CYD 쪽에는 그것을 알려 줄 오류가 없다.
    #[tokio::test]
    async fn a_listening_event_arrives_only_after_the_port_is_open() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn(port, tx, 9, None);

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Listening 이 오지 않았다")
            .unwrap();
        match ev {
            ServerEvent::Listening { generation } => {
                assert_eq!(generation, 9, "어느 리스너가 떴는지 말해야 한다")
            }
            other => panic!("Listening 을 기대했다: {other:?}"),
        }

        // 통지를 받은 시점에 포트는 이미 열려 있어야 한다. 여기서 기다리지 않는
        // 것이 요점이다 — `wait_until_listening` 을 쓰면 순서를 보지 못한다.
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok(),
            "Listening 이 bind 보다 먼저 나갔다 — 광고가 죽은 포트를 가리킨다"
        );

        let _ = handle.stop();
    }

    /// 반대쪽. bind 가 실패하면 `Listening` 은 **나가지 않는다** — 나가면 열리지
    /// 않은 포트가 광고된다. 첫 이벤트가 `BindFailed` 라는 것이 그 증거다.
    #[tokio::test]
    async fn a_failed_bind_never_says_it_is_listening() {
        let occupier = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
        let port = occupier.local_addr().unwrap().port();

        let (tx, mut rx) = events();
        let _handle = spawn(port, tx, 3, None);

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("이벤트가 오지 않았다")
            .unwrap();
        assert!(
            matches!(ev, ServerEvent::BindFailed { generation: 3, .. }),
            "실패한 리스너가 떴다고 말했다: {ev:?}"
        );
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
        // 인증 시한은 기본값(150초) 그대로 둔다 — 이 테스트가 보려는 것은
        // 무응답 경로이므로, 시한이 끼어들면 무엇이 연결을 놓았는지 흐려진다.
        let timing = Timing {
            ping: Duration::from_millis(60),
            idle: Duration::from_millis(200),
            ..Timing::default()
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
            Timing {
                ping: Duration::from_millis(40),
                idle: Duration::from_secs(30),
                ..Timing::default()
            },
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
            ..Timing::default()
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

    // --- 인증 시한 (인가되지 않은 연결의 절대 상한) ---

    /// 시한이 보는 것은 인가 여부 하나뿐이다. 조건이 하나 더 붙는 순간 시한은
    /// 다시 피어가 조종할 수 있는 값이 된다.
    #[test]
    fn only_authorization_lifts_the_deadline() {
        assert_eq!(deadline(false), Deadline::Drop);
        assert_eq!(deadline(true), Deadline::Lift);
    }

    /// **이 두 상수는 함께 움직여야 한다.** 페어링 전 구간이 인가되지 않은
    /// 연결 위에서 일어나고, 그 사람 시간의 예산이 `CODE_TTL` 이다. 시한이
    /// 그보다 짧으면 사람이 기기 키패드 앞에 서 있는 동안 연결이 끊기고, 다시
    /// 붙은 연결에는 `HELLO2` 트랜스크립트가 없어 **제대로 입력한 코드가
    /// `Rejected` 로 돌아온다.** 시한만 줄이는 변경을 여기서 막는다.
    #[test]
    fn the_auth_deadline_outlives_the_code_a_human_is_typing() {
        assert!(
            AUTH_DEADLINE > crate::ble::pairing::CODE_TTL,
            "인증 시한({AUTH_DEADLINE:?})이 페어링 코드 수명({:?})보다 짧으면 \
             정상 페어링이 중간에 끊긴다",
            crate::ble::pairing::CODE_TTL
        );
    }

    /// 인가 통지는 **그 연결에만** 붙는다. 옆 연결로 새면 낯선 기기가 남의
    /// 인가로 시한 없는 자리를 얻는다.
    #[test]
    fn marking_one_connection_does_not_authorize_another() {
        let sinks = Sinks::default();
        let (mine, _rx1) = conn();
        let (theirs, _rx2) = conn();
        let (a, b) = (central_id(0), central_id(1));
        sinks.insert(a.clone(), mine);
        sinks.insert(b.clone(), theirs);

        sinks.mark_authorized(&a);

        assert!(sinks.get(&a).unwrap().authorized.load(Ordering::Relaxed));
        assert!(
            !sinks.get(&b).unwrap().authorized.load(Ordering::Relaxed),
            "옆 연결까지 인가된 것으로 표시됐다"
        );
    }

    /// 이미 끝난 연결에 대한 늦은 통지는 아무 일도 하지 않는다 — 항목을
    /// 만들어서도 안 된다.
    #[test]
    fn marking_an_unknown_connection_creates_nothing() {
        let sinks = Sinks::default();
        sinks.mark_authorized(&central_id(9));
        assert_eq!(sinks.len(), 0);
    }

    /// 이 테스트가 이 태스크의 요구사항 그 자체다.
    ///
    /// 피어는 **계속 말한다** — 그러므로 `last_seen` 이 계속 갱신돼 무응답
    /// 판정은 오지 않는다(`idle` 을 30초로 넉넉히 잡아 그 경로를 배제한다).
    /// 소비자 속도에 맞춰 보내므로 이벤트 큐도 넘치지 않고, 우리가 보내는
    /// 것을 다 읽어 주므로 송신 큐도 밀리지 않는다. 즉 **다른 어떤 상한도
    /// 이 연결에 걸리지 않는다.** 그래도 놓여야 한다.
    #[tokio::test]
    async fn a_peer_that_never_authorizes_is_dropped_however_well_it_behaves() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn_with(
            port,
            tx,
            1,
            None,
            Timing {
                ping: Duration::from_millis(40),
                idle: Duration::from_secs(30),
                auth: Duration::from_millis(300),
            },
        );
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        // 시한을 넘길 때까지 주기적으로 말한다. 이벤트는 그때그때 비워
        // 이벤트 큐 상한이 끼어들 여지를 없앤다.
        let dropped = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let _ = sock.write_all(&masked_text_frame(b"HELLO")).await;
                while let Ok(ev) = rx.try_recv() {
                    if ev == ServerEvent::Disconnected(id.clone()) {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await;

        assert!(
            dropped.is_ok(),
            "말은 하지만 인가되지 않은 피어가 자리를 계속 붙들고 있다"
        );
        let _ = handle.stop();
    }

    /// 반대쪽. **인가된 연결에는 시한이 없다** — 있으면 멀쩡히 미러를 보고
    /// 있던 기기가 150초마다 화면 앞에서 끊긴다.
    #[tokio::test]
    async fn an_authorized_peer_outlives_the_deadline() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn_with(
            port,
            tx,
            1,
            None,
            // 시한을 600ms 로 잡은 것은 펌프가 인가 통지를 처리할 여유를
            // 넉넉히 주기 위해서다 — 그 처리가 시한보다 늦으면 이 테스트는
            // 실제 결함이 아니라 부하 때문에 실패한다.
            Timing {
                ping: Duration::from_secs(30),
                idle: Duration::from_secs(30),
                auth: Duration::from_millis(600),
            },
        );
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;

        // 앱의 이벤트 루프가 `handle_auth` 결과를 보고 하는 일 그대로.
        handle.outbound.send(Outbound::Authorized(id.clone())).unwrap();

        // 시한을 한참 넘긴 뒤에도 살아 있어야 한다. 살아 있다는 증거는
        // "끊겼다는 이벤트가 없다"가 아니라 **프레임이 아직 오간다**는 것이다.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        sock.write_all(&masked_text_frame(b"HELLO")).await.unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Frame { id, text: "HELLO".to_string() },
            "인가된 연결이 시한에 끊겼다"
        );

        let _ = handle.stop();
    }

    /// 인가되기 **전에** 도착한 프레임은 시한을 밀지 못한다. 위 테스트가
    /// 걷어 준 것이 인가 통지인지 그냥 트래픽인지 헷갈릴 여지를 없앤다.
    #[tokio::test]
    async fn traffic_alone_does_not_lift_the_deadline() {
        let port = free_port().await;
        let (tx, mut rx) = events();
        let handle = spawn_with(
            port,
            tx,
            1,
            None,
            Timing {
                ping: Duration::from_secs(30),
                idle: Duration::from_secs(30),
                auth: Duration::from_millis(200),
            },
        );
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let id = next_connected(&mut rx).await;
        sock.write_all(&masked_text_frame(b"HELLO")).await.unwrap();
        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Frame { id: id.clone(), text: "HELLO".to_string() }
        );

        assert_eq!(
            next_event(&mut rx).await,
            ServerEvent::Disconnected(id),
            "프레임을 보냈다는 것만으로 시한이 걷히면 안 된다"
        );

        let _ = handle.stop();
    }
}
