//! LAN 전송 브리지 (스펙 2026-08-25-cyd-client-design.md).
//!
//! `network/mod.rs`(iroh)와 같은 표면을 갖는다. WebSocket 연결 하나가 세션
//! 하나이고 `CentralId` 하나에 대응한다 — BLE 링크와 같은 모델이다. 실제
//! 리스너는 `server` 가 띄우고, 이 브리지는 "지금 누가 붙어 있고 무엇이
//! 잘못됐는지"만 안다(BLE 의 `peripheral` / 네트워크의 accept 루프와 같은
//! 역할 분리).

pub mod server;

use crate::ble::pairing::{self, AuthReply, PairingManager};
use crate::ble::peripheral::CentralId;
use server::{Outbound, ServerEvent, ServerHandle};
use std::collections::HashSet;
use std::time::SystemTime;
use tokio::sync::mpsc::Sender;

/// `handle_auth` 한 번의 결과. `network::AuthOutcome` 과 **필드까지 같다** —
/// 세 전송이 같은 표면을 유지해야 빠진 분기가 눈에 띈다.
///
/// `granted` 를 빼고 싶어지는 순간이 온다("LAN 은 v2 전용인데?"). 틀렸다.
/// v2 페어링도 `Granted2` 로 **새 토큰을 발급**하고, 이 플래그가 호출부가
/// 그 토큰을 디스크에 쓰는 유일한 신호다. 빠뜨리면 LAN 으로 페어링한 기기가
/// 그 세션에서는 멀쩡히 동작하다가 맥을 껐다 켜는 순간 토큰이 사라져 영영
/// 재연결하지 못한다(`ble/mod.rs` 의 같은 주석 참고).
pub struct AuthOutcome {
    /// 이 central 에게 그대로 되돌려보낼 바이트. `AuthReply::to_json_bytes()`
    /// 그대로다 — LAN 만 새 포맷을 만들 이유가 없다.
    pub payload: Vec<u8>,
    /// 이 응답이 central 을 인가된 상태로 만들었는가.
    pub now_authorized: bool,
    /// 새 토큰이 발급됐는가. 호출부가 페어링 목록을 디스크에 쓴다.
    pub granted: bool,
}

/// 응답 하나를 `(now_authorized, granted)` 로 가른다.
///
/// 연결 코드가 아니라 여기에 있는 이유는 이 판정이 이 전송에서 가장 틀리기
/// 쉬운 지점이기 때문이다 — 한 갈래를 빠뜨려도 컴파일은 되고 대부분의
/// 흐름은 멀쩡해 보인다. `matches!` 가 아니라 **모든 변형을 적는 match** 를
/// 쓴 것도 같은 이유다: 프로토콜에 응답이 하나 늘면 여기서 컴파일이 깨져
/// 판정을 다시 보게 만든다.
fn classify(reply: &AuthReply) -> (bool, bool) {
    match reply {
        // 인가 + 새 토큰.
        AuthReply::Granted { .. } | AuthReply::Granted2 { .. } => (true, true),
        // 인가만 — 재연결은 이미 아는 토큰을 확인했을 뿐이라 저장할 것이 없다.
        AuthReply::Authorized | AuthReply::Authorized2 => (true, false),
        // 아직 핸드셰이크 중이거나 거절이다.
        AuthReply::AwaitingCode
        | AuthReply::AwaitingCode2 { .. }
        | AuthReply::Nonce { .. }
        | AuthReply::Nonce2 { .. }
        | AuthReply::Denied { .. }
        | AuthReply::Rejected => (false, false),
    }
}

/// LAN 전송이 지금 서비스 중인 central 들과, 사용자에게 보여줄 마지막 오류를
/// 들고 있는다. `network::NetworkBridge`와 표면을 맞춘 이유는 `lib.rs` 배선을
/// 세 전송(BLE/network/lan) 모두 같은 모양으로 유지해, 하나만 고치고 다른
/// 쪽을 잊는 드리프트를 줄이기 위해서다.
pub struct LanBridge {
    enabled: bool,
    /// 현재 붙어 있는 연결들. 서버 이벤트(`apply_event`)가 갱신한다. `network`
    /// 브리지가 `HashMap<CentralId, SendStream>` 로 도메인 타입을 키로 쓰듯,
    /// 여기도 `String` 으로 낮추지 않는다 — 실제 송신 자원이 아직 없어 값
    /// 타입은 `()` 지만, 키 타입까지 낮출 이유는 없다.
    centrals: HashSet<CentralId>,
    /// 사용자에게 보여줄 마지막 오류. 이 앱은 로그 파일을 남기지 않으므로
    /// Devices 패널이 실패 원인(포트 점유·권한 거부 등)을 알 수 있는 유일한
    /// 경로다.
    last_error: Option<String>,
    /// 살아 있는 리스너. `Some` 인 동안만 포트가 열려 있다. 토글을 끄면
    /// 플래그만 내리는 게 아니라 이 핸들을 내려서 소켓까지 실제로 닫는다.
    server: Option<ServerHandle>,
    /// 방금 내린 리스너의 태스크. 다음 리스너가 이것이 끝난 뒤에 bind 하도록
    /// 넘겨준다 — 끄자마자 켰을 때 우리가 만든 `EADDRINUSE` 를 사용자에게
    /// 보여주지 않기 위해서다(`ServerHandle::stop` 의 doc).
    stopping: Option<tokio::task::JoinHandle<()>>,
    /// 지금까지 띄운 리스너의 수 = 현재 세대 번호. `BindFailed` 가 어느
    /// 리스너의 것인지 가리는 데 쓴다.
    generation: u64,
    /// 서버 태스크가 이벤트를 올릴 통로. 리스너는 켰다 껐다 하지만 이 통로는
    /// 브리지와 수명을 같이한다 — 배선(`lib.rs`)이 수신 루프를 한 번만 걸면
    /// 되도록.
    events: Sender<ServerEvent>,
    /// 리스너가 붙을 포트. 운영에서는 언제나 `server::PORT` 다.
    port: u16,
}

impl LanBridge {
    pub fn new(events: Sender<ServerEvent>) -> Self {
        Self::with_port(events, server::PORT)
    }

    fn with_port(events: Sender<ServerEvent>, port: u16) -> Self {
        Self {
            enabled: false,
            centrals: HashSet::new(),
            last_error: None,
            server: None,
            stopping: None,
            generation: 0,
            events,
            port,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// BLE·network 브리지와 같은 이유로 상태를 정리한다 — 꺼졌다 켜졌을 때
    /// 예전 연결이 여전히 붙어 있는 것으로 남지 않게. LAN 공유는 기본 꺼짐이고
    /// 리스너는 이 토글이 켜져 있는 동안만 존재한다(스펙 4장).
    ///
    /// `tokio` 런타임 안에서 불러야 한다 — 여기서 리스너 태스크를 띄운다.
    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if on {
            // 다시 켜는 순간 지난 실패는 더 이상 사실이 아니다. 여기서 지우지
            // 않으면 사용자가 포트를 비우고 다시 켜도 패널은 계속 예전 오류를
            // 보여준다 — 고쳤는데 고쳐지지 않은 것처럼 보이는 게 오류를 아예
            // 안 보여주는 것만큼 나쁘다. 이번 시도도 실패하면 `BindFailed` 가
            // 곧바로 다시 채운다.
            self.last_error = None;
            self.generation += 1;
            self.server = Some(server::spawn(
                self.port,
                self.events.clone(),
                self.generation,
                // 방금 내린 리스너가 있으면 그것이 끝난 뒤에 bind 한다.
                self.stopping.take(),
            ));
        } else {
            self.centrals.clear();
            // 꺼진 전송의 오류를 계속 보여줄 이유가 없다 — BLE·network 도 끌 때
            // 지운다(`lib.rs`). `last_error` 를 브리지가 소유하기로 한 이상 지우는
            // 책임도 여기 있다. 배선이 기억해야 하는 정리는 언젠가 잊힌다.
            self.last_error = None;
            if let Some(h) = self.server.take() {
                self.stopping = Some(h.stop());
            }
        }
    }

    /// 이 전송이 지금 서비스 중인 central 목록. BLE·network 의
    /// `served_centrals`와 같은 목적이다.
    pub fn served_centrals(&self) -> Vec<CentralId> {
        self.centrals.iter().cloned().collect()
    }

    /// 연결이 끊긴 central 의 자원을 정리한다. `network::forget_central` 과
    /// 같다 — 세션 인가는 앱이 공유 `PairingManager` 에서 지운다.
    pub fn forget_central(&mut self, central: &CentralId) {
        self.centrals.remove(central);
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.clone()
    }

    /// 사용자에게 보여줄 오류를 채우거나 지운다. `BindFailed` 말고도 호출부가
    /// 이 전송의 실패(토큰 저장 실패 등)를 실어야 할 곳이 있다 — 이 앱은 로그
    /// 파일을 남기지 않으므로 여기가 사용자가 알 수 있는 유일한 경로다.
    pub fn set_last_error(&mut self, msg: Option<String>) {
        self.last_error = msg;
    }

    /// 이 이벤트로 끝나는 세션들. **`apply_event` 보다 먼저** 불러야 한다 —
    /// `apply_event` 가 먼저 돌면 이 central 은 이미 목록에서 빠져 있어 언제나
    /// 빈 결과가 나온다(BLE 의 `sessions_to_end_before` 와 같은 순서 규칙).
    ///
    /// 인가는 공유 `PairingManager` 가 갖고 있고 이 브리지는 그것을 소유하지
    /// 않는다. 그래서 "누구의 인가가 끝나야 하는가"라는 **판단만** 여기서 하고,
    /// 실제 삭제는 호출부가 한다 — 판단을 배선 코드에 두면 테스트가 닿지 않는다.
    ///
    /// 우리가 서비스하지 않는 id 는 돌려주지 않는다. LAN 의 id 는 `lan:N` 이라
    /// 다른 전송과 겹치지 않지만, 공유 매니저를 건드리는 목록에 모르는 id 를
    /// 넣지 않는다는 규칙 자체를 지킨다.
    pub fn sessions_to_end(&self, ev: &ServerEvent) -> Vec<CentralId> {
        match ev {
            ServerEvent::Disconnected(id) if self.centrals.contains(id) => vec![id.clone()],
            // 바인딩 실패는 연결이 없었다는 뜻이고, 프레임은 세션을 끝내지 않는다.
            _ => Vec::new(),
        }
    }

    /// 이 전송이 **지금** 이 central 을 서비스하고 있는가. 인증과 스냅샷이
    /// 공유하는 문이다.
    ///
    /// `enabled` 를 따로 보는 이유: `set_enabled(false)` 는 `centrals` 를 비우지만,
    /// 서버 태스크가 종료되는 동안 직전에 업그레이드된 연결의 `Connected` 가
    /// 뒤늦게 올라와 목록을 다시 채울 수 있다. 그 이벤트를 버리지 않는 것은
    /// 일부러다 — 무엇을 기억할지는 이벤트 순서대로 두고, "꺼져 있으면 아무도
    /// 서비스하지 않는다"는 판단은 쓰는 쪽에서 한 번만 한다.
    fn serves(&self, central: &CentralId) -> bool {
        self.enabled && self.centrals.contains(central)
    }

    /// 인증 프레임 하나를 `PairingManager` 에 그대로 넘기고 응답을 돌려준다.
    /// I/O 는 없다 — `ble::BleBridge::handle_auth`·`network` 와 마찬가지로
    /// 동기 상태 기계 호출일 뿐이다.
    ///
    /// **우리가 서비스 중인 연결이 아니면 `None` 이고, `pairing` 을 아예 건드리지
    /// 않는다.** 이 문이 없으면 이미 `Disconnected` 로 정리된(혹은 토글이 꺼져
    /// 정리된) id 의 프레임이 뒤늦게 처리되면서 **살아 있는 링크가 없는 central 을
    /// 인가**할 수 있다. 그 인가를 지워 줄 `Disconnected` 는 다시 오지 않으므로
    /// 링크보다 오래 사는 인가가 남는다 — LAN 리스너가 켜져 있는 한 스냅샷을 받을
    /// 자격이 있는 유령이다.
    ///
    /// v1 이라고 거절하지 않는다. 맥은 두 세대를 모두 받아들이고, 그 판단은
    /// `PairingManager` 의 것이지 전송의 것이 아니다.
    pub fn handle_auth(
        &mut self,
        central: &CentralId,
        data: &[u8],
        now: SystemTime,
        pairing: &mut PairingManager,
    ) -> Option<AuthOutcome> {
        if !self.serves(central) {
            return None;
        }
        let reply = pairing.handle(central, pairing::parse_auth_request(data), now);
        let (now_authorized, granted) = classify(&reply);
        Some(AuthOutcome { payload: reply.to_json_bytes(), now_authorized, granted })
    }

    /// 인증 응답을 이 central 에게 텍스트 프레임으로 돌려보낸다. 인증 프레임은
    /// 양방향 모두 텍스트다 — BLE·network 가 쓰는 바이트 그대로이고, 스냅샷만
    /// 바이너리다(Task 4).
    ///
    /// 리스너가 없거나(토글이 꺼졌다) 그 사이 상대가 사라졌으면 조용히
    /// 버려진다. 상대가 끊은 것은 오류가 아니므로 `last_error` 를 건드리지
    /// 않는다 — 그 필드는 사용자가 고칠 수 있는 실패만 담는다.
    pub fn send_auth_reply(&self, central: &CentralId, payload: Vec<u8>) {
        if let Some(h) = &self.server {
            let _ = h.outbound.send(Outbound::Text(central.clone(), payload));
        }
    }

    /// 스냅샷을 보낼 대상. **인가되지 않은 연결은 들어가지 않는다** — 붙어
    /// 있다는 것과 볼 자격이 있다는 것은 다르다. 실제 봉인과 전송은 Task 4 지만,
    /// "누구에게 보낼 수 있는가"는 인증의 결론이므로 여기서 정한다.
    pub fn snapshot_targets(&self, pairing: &PairingManager) -> Vec<CentralId> {
        self.centrals
            .iter()
            .filter(|id| self.serves(id) && pairing.is_authorized(id))
            .cloned()
            .collect()
    }

    /// 이 이벤트가 **지금 살아 있는 리스너**의 것인가. 리스너가 없으면(꺼졌거나
    /// 이미 실패로 내려갔다) 어느 세대의 통지든 반영할 것이 없다.
    fn is_current_generation(&self, generation: u64) -> bool {
        self.server.as_ref().map(|h| h.generation) == Some(generation)
    }

    /// 서버 태스크가 올린 이벤트를 상태에 반영한다. 배선(`lib.rs`)에 흩어 두지
    /// 않고 여기에 모은 이유는, "무엇을 기억하고 무엇을 잊을지"가 이 브리지의
    /// 결정이기 때문이다 — 연결 코드 안에 두면 테스트가 닿지 않는다.
    pub fn apply_event(&mut self, ev: &ServerEvent) {
        match ev {
            ServerEvent::Connected(id) => {
                self.centrals.insert(id.clone());
            }
            ServerEvent::Disconnected(id) => self.forget_central(id),
            ServerEvent::BindFailed { generation, message } => {
                // 낡은 세대의 실패는 지금의 사실이 아니다. 이 문이 없으면
                // 이벤트가 한 사이클만 밀려도 **정상적으로 뜬 리스너를 죽인다**:
                // 핸들을 떨어뜨리면 watch 송신자가 사라지고, `wait_shutdown` 은
                // 그것을 종료로 해석해 멀쩡한 소켓을 닫는다. 남는 상태는
                // "켜져 있는데 리스너는 없고 화면에는 옛날 오류" 다.
                if !self.is_current_generation(*generation) {
                    return;
                }
                self.last_error = Some(message.clone());
                // 바인딩에 실패했으면 리스너는 없다. 핸들을 계속 들고 있으면
                // 상태가 "켜져 있고 서버도 있다"로 보여 실제와 어긋난다.
                self.server = None;
            }
            // 프레임은 세션 목록을 바꾸지 않는다. 해석은 `handle_auth` 가
            // 하고, 그 결과(인가)는 공유 `PairingManager` 에 남는다.
            ServerEvent::Frame { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::pairing::test_client::{self, hex_encode, V2Client};
    use crate::crypto;
    use std::time::{Duration, UNIX_EPOCH};
    use tokio::sync::mpsc::{channel, Receiver};

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// 테스트는 임시 포트(0)로 띄운다 — 서로, 그리고 개발 중인 앱의 4320 을
    /// 밟지 않기 위해서다. 실제 포트가 열리고 닫히는지는 `server` 쪽 테스트가 본다.
    fn bridge() -> (LanBridge, Receiver<ServerEvent>) {
        let (tx, rx) = channel(server::EVENT_QUEUE);
        (LanBridge::with_port(tx, 0), rx)
    }

    #[test]
    fn starts_disabled() {
        let (b, _rx) = bridge();
        assert!(!b.is_enabled(), "LAN 공유는 기본 꺼짐이다");
        assert!(b.server.is_none(), "켜기 전에는 리스너가 없어야 한다");
    }

    #[tokio::test]
    async fn toggling_on_and_off_is_idempotent() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.set_enabled(true);
        assert!(b.is_enabled());
        b.set_enabled(false);
        b.set_enabled(false);
        assert!(!b.is_enabled());
    }

    #[test]
    fn has_no_centrals_when_disabled() {
        let (b, _rx) = bridge();
        assert!(b.served_centrals().is_empty());
    }

    /// 끄면 리스너 핸들이 사라져야 한다. 플래그만 내리고 핸들을 들고 있으면
    /// 소켓은 계속 열려 있다.
    #[tokio::test]
    async fn disabling_drops_the_listener() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        assert!(b.server.is_some(), "켜면 리스너가 있어야 한다");
        b.set_enabled(false);
        assert!(b.server.is_none(), "끄면 리스너 핸들도 없어야 한다");
    }

    #[test]
    fn connecting_and_disconnecting_tracks_that_central_only() {
        let (mut b, _rx) = bridge();
        let a = server::central_id(0);
        let c = server::central_id(1);

        b.apply_event(&ServerEvent::Connected(a.clone()));
        b.apply_event(&ServerEvent::Connected(c.clone()));
        assert_eq!(b.served_centrals().len(), 2);

        // 하나가 끊겼다고 나머지까지 내려가면 안 된다 — `set_enabled(false)` 만이
        // 전부를 비운다.
        b.apply_event(&ServerEvent::Disconnected(a.clone()));
        assert_eq!(b.served_centrals(), vec![c]);
    }

    #[test]
    fn forgetting_an_unknown_central_is_harmless() {
        let (mut b, _rx) = bridge();
        b.apply_event(&ServerEvent::Disconnected(server::central_id(42)));
        assert!(b.served_centrals().is_empty());
    }

    /// 프레임은 아직 이 브리지의 관심사가 아니다 — 세션 목록을 건드리면 안 된다.
    #[test]
    fn a_frame_does_not_invent_a_session() {
        let (mut b, _rx) = bridge();
        b.apply_event(&ServerEvent::Frame {
            id: server::central_id(0),
            text: "AUTH:...".to_string(),
        });
        assert!(b.served_centrals().is_empty());
    }

    /// 지금 살아 있는 리스너의 바인딩 실패. 세대를 브리지에서 읽어 오므로
    /// "몇 번째 리스너인가"를 테스트가 세고 있을 필요가 없다.
    fn bind_failed_now(b: &LanBridge) -> ServerEvent {
        ServerEvent::BindFailed {
            generation: b.generation,
            message: "포트 4320 이 이미 쓰이는 중".into(),
        }
    }

    #[tokio::test]
    async fn a_bind_failure_becomes_the_error_the_panel_shows() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        assert!(b.last_error().is_none());
        b.apply_event(&bind_failed_now(&b));
        assert_eq!(b.last_error().as_deref(), Some("포트 4320 이 이미 쓰이는 중"));
    }

    /// 오류를 채우기만 하고 지우지 않으면, 사용자가 원인을 고치고 다시 켜도
    /// 패널이 계속 예전 실패를 보여준다. 켜는 쪽이 반드시 지워야 한다.
    #[tokio::test]
    async fn restarting_clears_a_stale_bind_failure() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&bind_failed_now(&b));
        assert!(b.last_error().is_some());

        b.set_enabled(false);
        b.set_enabled(true);
        assert!(b.last_error().is_none(), "다시 켰으면 지난 실패는 사실이 아니다");
    }

    /// 끄면 오류도 사실이 아니게 된다 — 꺼진 전송의 실패를 계속 보여줄 이유가
    /// 없다. BLE·network 는 끌 때 지운다(`lib.rs`). `last_error` 를 브리지가
    /// 소유하기로 한 이상 지우는 책임도 브리지에 있다.
    #[tokio::test]
    async fn disabling_clears_the_error_too() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&bind_failed_now(&b));
        assert!(b.last_error().is_some());

        b.set_enabled(false);
        assert!(b.last_error().is_none(), "꺼진 전송의 실패를 계속 보여주면 안 된다");
    }

    /// 켜져 있는 상태에서 다시 `set_enabled(true)` 를 부르는 건 아무 일도
    /// 하지 않는다 — 재시작이 아니므로 오류도 지우면 안 된다. 지워버리면
    /// 방금 실패한 서버가 정상인 것처럼 보인다.
    #[tokio::test]
    async fn a_redundant_enable_does_not_hide_a_live_failure() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&bind_failed_now(&b));
        b.set_enabled(true);
        assert!(b.last_error().is_some());
    }

    /// **낡은 세대의 실패는 지금 리스너를 죽이면 안 된다.** 이벤트가 한 사이클만
    /// 밀려도(Task 3 의 소비자는 이벤트마다 락을 잡는다) 껐다 켠 뒤에 도착할 수
    /// 있고, 그때 핸들을 떨어뜨리면 watch 송신자가 사라져 **정상 동작 중인
    /// 소켓이 닫힌다** — 남는 상태는 "켜져 있는데 리스너 없음 + 옛 오류".
    #[tokio::test]
    async fn a_stale_bind_failure_does_not_kill_the_live_listener() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        let stale = bind_failed_now(&b); // 세대 1 의 실패
        b.set_enabled(false);
        b.set_enabled(true); // 세대 2 가 정상적으로 떴다

        b.apply_event(&stale);

        assert!(b.server.is_some(), "낡은 통지가 살아 있는 리스너를 내렸다");
        assert!(b.last_error().is_none(), "낡은 실패를 지금 사실처럼 보여주면 안 된다");
    }

    /// 꺼져 있을 때 도착한 실패도 반영하지 않는다 — 리스너가 없으니 어느
    /// 세대의 통지든 지금의 사실이 아니다.
    #[tokio::test]
    async fn a_bind_failure_after_the_toggle_went_off_is_ignored() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        let ev = bind_failed_now(&b);
        b.set_enabled(false);

        b.apply_event(&ev);
        assert!(b.last_error().is_none());
    }

    /// 세대 판정 자체. `apply_event` 를 거치지 않고 판단만 본다.
    #[tokio::test]
    async fn only_the_live_listeners_generation_counts() {
        let (mut b, _rx) = bridge();
        assert!(!b.is_current_generation(0), "리스너가 없으면 어느 세대도 아니다");

        b.set_enabled(true);
        let g = b.generation;
        assert!(b.is_current_generation(g));
        assert!(!b.is_current_generation(g - 1));
        assert!(!b.is_current_generation(g + 1));
    }

    /// 진짜 4320 에서 토글이 소켓을 열고 닫는지 본다. 평소 실행에서 빠져 있는
    /// 이유는 개발 중인 앱이나 다른 테스트와 같은 포트를 다투기 때문이다 —
    /// 손으로 확인할 때만 돌린다:
    /// `cargo test --manifest-path src-tauri/Cargo.toml lan:: -- --ignored --test-threads=1`
    #[tokio::test]
    #[ignore = "실제 4320 을 잡는다 — 손으로만"]
    async fn the_real_port_opens_and_closes_with_the_toggle() {
        let (tx, _rx) = channel(server::EVENT_QUEUE);
        let mut b = LanBridge::new(tx);

        assert!(
            tokio::net::TcpListener::bind(("0.0.0.0", server::PORT)).await.is_ok(),
            "시작하기 전에 4320 이 비어 있어야 한다"
        );

        b.set_enabled(true);
        let mut opened = false;
        for _ in 0..200 {
            if tokio::net::TcpStream::connect(("127.0.0.1", server::PORT)).await.is_ok() {
                opened = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(opened, "토글을 켰는데 4320 이 열리지 않았다");

        b.set_enabled(false);
        for _ in 0..200 {
            if tokio::net::TcpListener::bind(("0.0.0.0", server::PORT)).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("토글을 껐는데 4320 이 그대로 열려 있다");
    }

    /// 끄면 붙어 있던 central 이 전부 사라진다 — 다시 켰을 때 예전 연결이
    /// 여전히 붙어 있는 것으로 남지 않게.
    #[tokio::test]
    async fn disabling_clears_every_central() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&ServerEvent::Connected(server::central_id(0)));
        b.apply_event(&ServerEvent::Connected(server::central_id(1)));
        b.set_enabled(false);
        assert!(b.served_centrals().is_empty());
    }

    // --- 인증 (Task 3) ---

    /// 켜져 있고 central 하나가 붙어 있는 브리지. 인증 경로의 출발점이다.
    /// `set_enabled(true)` 가 리스너 태스크를 띄우므로 런타임 안에서만 부른다.
    fn live_bridge(id: &CentralId) -> (LanBridge, Receiver<ServerEvent>) {
        let (mut b, rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&ServerEvent::Connected(id.clone()));
        (b, rx)
    }

    fn field(bytes: &[u8], key: &str) -> String {
        let v: serde_json::Value = serde_json::from_slice(bytes).expect("응답은 JSON 이다");
        v[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} 필드가 없다: {v}"))
            .to_string()
    }

    /// 브리지의 `handle_auth` 만으로 v2 페어링을 끝까지 밟는다 — 응답 바이트를
    /// 클라이언트가 실제로 열 수 있는지까지 확인하므로, 배선이 페이로드를
    /// 건드렸다면 여기서 깨진다.
    fn v2_pair(
        b: &mut LanBridge,
        p: &mut PairingManager,
        central: &CentralId,
        now: SystemTime,
    ) -> AuthOutcome {
        let code = p.begin_pairing(now);
        let mut c = V2Client::new();

        let out = b
            .handle_auth(central, format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(), now, p)
            .expect("서비스 중인 연결이다");
        let (epk, nonce) = (field(&out.payload, "epk"), field(&out.payload, "nonce"));
        let (ss, tr) = c.agree(&epk);

        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let out = b
            .handle_auth(central, format!("CODE2:{cbind}").as_bytes(), now, p)
            .expect("서비스 중인 연결이다");
        let sealed = field(&out.payload, "sealed");
        let _ = test_client::open_pairing_and_session(&ss, &nonce, &sealed);
        out
    }

    /// 인증 프레임은 그대로 `PairingManager` 에 넘어가고, 응답은
    /// `AuthReply::to_json_bytes()` 그대로다. LAN 만 새 포맷을 만들지 않는다.
    #[tokio::test]
    async fn passes_auth_frames_through_unchanged() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));

        let out = b.handle_auth(&id, b"HELLO2:short", t(1001), &mut p).expect("서비스 중이다");
        assert_eq!(
            out.payload,
            AuthReply::Rejected.to_json_bytes(),
            "형식 오류는 그대로 Rejected 가 나가야 한다"
        );
        assert!(!out.now_authorized);
        assert!(!out.granted);
    }

    /// **이 태스크에서 가장 비싼 실수.** v2 페어링도 새 토큰을 발급하므로
    /// `granted` 가 서야 한다. 서지 않으면 LAN 으로 페어링한 기기가 그 세션에서는
    /// 멀쩡히 동작하다가, 맥을 껐다 켜는 순간 토큰이 디스크에 없어 영영 재연결하지
    /// 못한다 — 테스트로 잡지 않으면 사용자 책상에서만 드러난다.
    #[tokio::test]
    async fn v2_pairing_is_reported_as_authorized_and_worth_persisting() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();

        let out = v2_pair(&mut b, &mut p, &id, t(1000));

        assert!(out.now_authorized, "v2 페어링 성공은 인가로 이어져야 한다");
        assert!(out.granted, "v2 도 새 토큰을 발급한다 — 저장하지 않으면 재부팅 후 사라진다");
        assert_eq!(p.issued_peers().len(), 1);
    }

    /// 재연결(`AUTH2`/`PROOF2`)은 인가지만 발급이 아니다. 여기서 `granted` 가
    /// 서면 붙을 때마다 같은 목록을 디스크에 다시 쓴다.
    #[tokio::test]
    async fn v2_reconnect_is_authorized_but_not_persisted() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        let token = "aa".repeat(16);
        p.load_peers(vec![(token.clone(), 900)]);
        let mut c = V2Client::new();
        let now = t(1000);

        let out = b
            .handle_auth(&id, format!("AUTH2:{}", hex_encode(&c.public)).as_bytes(), now, &mut p)
            .expect("서비스 중이다");
        let (epk, nonce) = (field(&out.payload, "epk"), field(&out.payload, "nonce"));
        let (_ss, tr) = c.agree(&epk);
        let proof = hex_encode(&crypto::session_proof(
            &test_client::hex_decode(&token),
            &test_client::hex_decode(&nonce),
            &tr,
        ));

        let out = b
            .handle_auth(&id, format!("PROOF2:{proof}").as_bytes(), now, &mut p)
            .expect("서비스 중이다");
        assert!(out.now_authorized, "재연결 성공은 인가로 이어져야 한다");
        assert!(!out.granted, "재연결은 새 토큰을 발급하지 않는다 — 저장할 것이 없다");
        assert!(p.is_authorized(&id));
    }

    /// v1 도 그대로 통과시킨다. 맥은 두 세대를 모두 받아들이고, 세대 판단은
    /// `PairingManager` 의 것이지 전송의 것이 아니다.
    #[tokio::test]
    async fn v1_is_not_rejected_by_this_transport() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        let code = p.begin_pairing(t(1000));

        let out = b.handle_auth(&id, b"HELLO", t(1001), &mut p).expect("서비스 중이다");
        assert_eq!(out.payload, AuthReply::AwaitingCode.to_json_bytes());

        let out = b
            .handle_auth(&id, format!("CODE:{code}").as_bytes(), t(1002), &mut p)
            .expect("서비스 중이다");
        assert!(out.now_authorized && out.granted, "v1 CODE 성공도 인가이자 발급이다");
    }

    /// 응답 하나하나가 어떤 신호인지 한자리에서 못박는다. `classify` 가
    /// 모든 변형을 적는 `match` 라서, 프로토콜에 응답이 늘면 컴파일이 먼저
    /// 깨지고 이 표가 그다음을 잡는다.
    #[test]
    fn every_reply_is_classified() {
        let table: Vec<(AuthReply, bool, bool)> = vec![
            (AuthReply::Granted { token: "t".into() }, true, true),
            (AuthReply::Granted2 { sealed: "s".into() }, true, true),
            (AuthReply::Authorized, true, false),
            (AuthReply::Authorized2, true, false),
            (AuthReply::AwaitingCode, false, false),
            (AuthReply::AwaitingCode2 { epk: "e".into(), nonce: "n".into() }, false, false),
            (AuthReply::Nonce { nonce: "n".into() }, false, false),
            (AuthReply::Nonce2 { epk: "e".into(), nonce: "n".into() }, false, false),
            (AuthReply::Denied { left: 2 }, false, false),
            (AuthReply::Rejected, false, false),
        ];
        for (reply, authorized, granted) in table {
            assert_eq!(
                classify(&reply),
                (authorized, granted),
                "{reply:?} 의 판정이 다르다"
            );
        }
    }

    /// 우리가 서비스하지 않는 id 의 프레임은 `pairing` 을 아예 건드리지 않는다.
    /// 건드리면 살아 있는 링크가 없는 central 이 인가되고, 그 인가를 지워 줄
    /// `Disconnected` 는 다시 오지 않는다.
    #[tokio::test]
    async fn a_frame_from_a_central_we_do_not_serve_is_ignored() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        let id = server::central_id(0); // Connected 를 받은 적이 없다
        let mut p = PairingManager::new();
        // 창이 열려 있고 코드도 맞다 — 통과시켰다면 토큰이 발급됐을 상황이다.
        let code = p.begin_pairing(t(1000));

        assert!(b.handle_auth(&id, format!("CODE:{code}").as_bytes(), t(1001), &mut p).is_none());
        assert!(!p.is_authorized(&id));
        assert!(p.issued_peers().is_empty(), "토큰이 발급되면 안 된다");
    }

    /// 끊긴 뒤 뒤늦게 도착한 프레임도 마찬가지다. 이벤트는 순서대로 오지만
    /// `Disconnected` 뒤의 프레임이 처리되면 링크보다 오래 사는 인가가 남는다.
    #[tokio::test]
    async fn a_frame_after_the_disconnect_is_ignored() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));

        b.apply_event(&ServerEvent::Disconnected(id.clone()));
        assert!(b.handle_auth(&id, b"HELLO", t(1001), &mut p).is_none());
        assert!(!p.is_authorized(&id));
    }

    /// 토글을 끈 뒤 뒤늦게 올라온 `Connected` 는 목록을 채울 수 있다(서버
    /// 태스크가 종료되는 동안 벌어질 수 있다). 그래도 인증은 열리면 안 된다 —
    /// 공유가 꺼져 있는데 누군가를 인가하는 것이 이 전송의 최악이다.
    #[tokio::test]
    async fn a_frame_while_sharing_is_off_is_ignored() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));

        b.set_enabled(false);
        b.apply_event(&ServerEvent::Connected(id.clone())); // 지각 이벤트
        assert!(b.handle_auth(&id, b"HELLO", t(1001), &mut p).is_none());
        assert!(!p.is_authorized(&id));
    }

    /// 연결이 끊기면 그 세션의 인가가 즉시 끝나야 한다. 브리지는 인가를
    /// 소유하지 않으므로 "누구를 끝낼지"만 말하고, 지우는 것은 호출부다.
    #[tokio::test]
    async fn dropping_a_connection_ends_its_session() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));
        assert!(p.is_authorized(&id));

        let ending = b.sessions_to_end(&ServerEvent::Disconnected(id.clone()));
        assert_eq!(ending, vec![id.clone()]);

        // 호출부(lib.rs)가 하는 일 그대로.
        p.end_sessions(&ending);
        b.apply_event(&ServerEvent::Disconnected(id.clone()));

        assert!(!p.is_authorized(&id), "링크가 끊겼는데 인가가 남아 있다");
        assert!(b.served_centrals().is_empty());
        assert!(b.snapshot_targets(&p).is_empty());
    }

    /// `apply_event` 를 먼저 부르면 목록에서 이미 빠져 언제나 빈 결과가 된다.
    /// 순서 규칙을 못박아 둔다 — 이 순서가 뒤집히면 인가가 조용히 살아남는다.
    #[tokio::test]
    async fn asking_after_applying_is_too_late() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        b.apply_event(&ServerEvent::Disconnected(id.clone()));
        assert!(
            b.sessions_to_end(&ServerEvent::Disconnected(id)).is_empty(),
            "apply_event 뒤에 물으면 늦다 — 호출부가 먼저 물어야 한다"
        );
    }

    /// 끊김만이 세션을 끝낸다. 프레임이나 바인딩 실패로 남의 인가를 내리면 안 된다.
    #[tokio::test]
    async fn only_a_disconnect_ends_a_session() {
        let id = server::central_id(0);
        let (b, _rx) = live_bridge(&id);
        assert!(b.sessions_to_end(&ServerEvent::Connected(id.clone())).is_empty());
        assert!(b
            .sessions_to_end(&ServerEvent::Frame { id: id.clone(), text: "HELLO".into() })
            .is_empty());
        assert!(b
            .sessions_to_end(&ServerEvent::BindFailed { generation: 1, message: "x".into() })
            .is_empty());
    }

    /// 우리가 서비스한 적 없는 id 는 공유 매니저를 건드리는 목록에 넣지 않는다.
    #[test]
    fn ending_a_session_we_never_served_touches_nothing() {
        let (b, _rx) = bridge();
        assert!(b
            .sessions_to_end(&ServerEvent::Disconnected(server::central_id(42)))
            .is_empty());
    }

    /// 인가되기 전에는 스냅샷 대상이 아니다 — 붙어 있다는 것과 볼 자격이
    /// 있다는 것은 다르다.
    #[tokio::test]
    async fn sends_nothing_before_authorization() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));
        assert!(b.snapshot_targets(&p).is_empty(), "인가되지 않으면 대상이 없다");

        // 핸드셰이크 중간(AwaitingCode2)도 아직 아니다.
        let c = V2Client::new();
        b.handle_auth(&id, format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(), t(1001), &mut p);
        assert!(b.snapshot_targets(&p).is_empty(), "핸드셰이크 중에는 대상이 아니다");
    }

    #[tokio::test]
    async fn an_authorized_central_becomes_a_target() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));
        assert_eq!(b.snapshot_targets(&p), vec![id]);
    }

    /// 인가된 기기가 붙어 있어도 공유를 끄면 대상이 없다.
    #[tokio::test]
    async fn turning_sharing_off_leaves_no_targets() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));

        b.set_enabled(false);
        b.apply_event(&ServerEvent::Connected(id.clone())); // 지각 이벤트
        assert!(b.snapshot_targets(&p).is_empty(), "꺼져 있으면 아무도 대상이 아니다");
    }

    /// 인가된 기기 옆에 인가되지 않은 기기가 붙어 있어도 그쪽은 대상이 아니다.
    #[tokio::test]
    async fn an_unauthorized_neighbour_is_not_carried_along() {
        let paired = server::central_id(0);
        let stranger = server::central_id(1);
        let (mut b, _rx) = live_bridge(&paired);
        b.apply_event(&ServerEvent::Connected(stranger.clone()));
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &paired, t(1000));

        assert_eq!(b.snapshot_targets(&p), vec![paired]);
    }

    /// 인증 프레임이 진짜 소켓으로 들어와 응답이 **텍스트 프레임으로** 되돌아가는
    /// 것까지 본다. 여기서만 `send_auth_reply` 가 실제 WebSocket 에 닿는다 —
    /// 나머지 테스트는 전부 I/O 없는 상태 기계 호출이다.
    #[tokio::test]
    async fn an_auth_frame_gets_its_reply_over_the_socket() {
        use server::test_socket::*;
        use tokio::io::AsyncWriteExt;

        let port = free_port().await;
        let (tx, mut rx) = channel(server::EVENT_QUEUE);
        let mut b = LanBridge::with_port(tx, port);
        let mut p = PairingManager::new();
        b.set_enabled(true);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let ev = rx.recv().await.expect("Connected 가 와야 한다");
        b.apply_event(&ev);
        // id 는 프로세스 전역 발급기가 준다 — 절대값을 가정하지 않는다.
        assert!(matches!(ev, ServerEvent::Connected(_)), "Connected 를 기대했다: {ev:?}");

        // 창이 열려 있지 않으므로 응답은 Rejected 다 — 여기서 보려는 것은
        // 페어링 결과가 아니라 "프레임이 오가는 통로"다.
        sock.write_all(&masked_text_frame(b"HELLO")).await.unwrap();
        let ServerEvent::Frame { id, text } = rx.recv().await.expect("Frame 이 와야 한다") else {
            panic!("Frame 을 기대했다");
        };

        let out = b
            .handle_auth(&id, text.as_bytes(), t(1000), &mut p)
            .expect("서비스 중인 연결이다");
        b.send_auth_reply(&id, out.payload.clone());

        let got = read_text_frame(&mut sock).await;
        assert_eq!(got, out.payload, "응답 바이트가 그대로 나가야 한다");
        assert_eq!(got, AuthReply::Rejected.to_json_bytes());

        b.set_enabled(false);
    }

    /// 맥을 껐다 켜면 일련번호는 다시 0부터 시작한다(`static` 은 프로세스와
    /// 수명을 같이한다). 그래도 예전 `lan:0` 의 인가를 물려받지 않는 이유는
    /// **인가가 디스크에 남지 않기 때문**이다 — 저장되는 것은 토큰뿐이고
    /// (`PairingManager::load_peers`), 세션 인가는 프로세스와 함께 사라진다.
    /// 이 성질이 깨지는 순간 재시작이 I2 와 똑같은 사고가 된다.
    #[tokio::test]
    async fn a_restored_peer_list_authorizes_nobody() {
        let id = server::central_id(0);
        let (b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.load_peers(vec![("aa".repeat(16), 900)]);

        assert_eq!(p.issued_peers().len(), 1, "토큰은 복원된다");
        assert!(!p.is_authorized(&id), "복원되는 것은 토큰이지 인가가 아니다");
        assert!(b.snapshot_targets(&p).is_empty(), "다시 증명하기 전에는 아무것도 못 받는다");
    }

    /// 리스너가 없을 때 응답을 보내려 해도 조용히 버려진다. 상대가 사라진 것은
    /// 사용자가 고칠 수 있는 실패가 아니므로 `last_error` 를 건드리면 안 된다.
    #[test]
    fn replying_with_no_listener_is_harmless() {
        let (b, _rx) = bridge();
        b.send_auth_reply(&server::central_id(0), b"{}".to_vec());
        assert!(b.last_error().is_none());
    }
}
