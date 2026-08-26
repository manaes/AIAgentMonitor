//! LAN 전송 브리지 (스펙 2026-08-25-cyd-client-design.md).
//!
//! `network/mod.rs`(iroh)와 같은 표면을 갖는다. WebSocket 연결 하나가 세션
//! 하나이고 `CentralId` 하나에 대응한다 — BLE 링크와 같은 모델이다. 실제
//! 리스너는 `server` 가 띄우고, 이 브리지는 "지금 누가 붙어 있고 무엇이
//! 잘못됐는지"만 안다(BLE 의 `peripheral` / 네트워크의 accept 루프와 같은
//! 역할 분리).

pub mod server;

use crate::ble::peripheral::CentralId;
use server::{ServerEvent, ServerHandle};
use std::collections::HashSet;
use tokio::sync::mpsc::Sender;

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
            self.server = Some(server::spawn(self.port, self.events.clone()));
        } else {
            self.centrals.clear();
            if let Some(h) = self.server.take() {
                h.stop();
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

    /// 서버 태스크가 올린 이벤트를 상태에 반영한다. 배선(`lib.rs`)에 흩어 두지
    /// 않고 여기에 모은 이유는, "무엇을 기억하고 무엇을 잊을지"가 이 브리지의
    /// 결정이기 때문이다 — 연결 코드 안에 두면 테스트가 닿지 않는다.
    pub fn apply_event(&mut self, ev: &ServerEvent) {
        match ev {
            ServerEvent::Connected(id) => {
                self.centrals.insert(id.clone());
            }
            ServerEvent::Disconnected(id) => self.forget_central(id),
            ServerEvent::BindFailed(msg) => {
                self.last_error = Some(msg.clone());
                // 바인딩에 실패했으면 리스너는 없다. 핸들을 계속 들고 있으면
                // 상태가 "켜져 있고 서버도 있다"로 보여 실제와 어긋난다.
                self.server = None;
            }
            // 프레임 해석은 pairing 이 한다(이후 태스크).
            ServerEvent::Frame { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{channel, Receiver};

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

    #[test]
    fn a_bind_failure_becomes_the_error_the_panel_shows() {
        let (mut b, _rx) = bridge();
        assert!(b.last_error().is_none());
        b.apply_event(&ServerEvent::BindFailed("포트 4320 이 이미 쓰이는 중".into()));
        assert_eq!(b.last_error().as_deref(), Some("포트 4320 이 이미 쓰이는 중"));
    }

    /// 오류를 채우기만 하고 지우지 않으면, 사용자가 원인을 고치고 다시 켜도
    /// 패널이 계속 예전 실패를 보여준다. 켜는 쪽이 반드시 지워야 한다.
    #[tokio::test]
    async fn restarting_clears_a_stale_bind_failure() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&ServerEvent::BindFailed("포트 4320 이 이미 쓰이는 중".into()));
        assert!(b.last_error().is_some());

        b.set_enabled(false);
        b.set_enabled(true);
        assert!(b.last_error().is_none(), "다시 켰으면 지난 실패는 사실이 아니다");
    }

    /// 켜져 있는 상태에서 다시 `set_enabled(true)` 를 부르는 건 아무 일도
    /// 하지 않는다 — 재시작이 아니므로 오류도 지우면 안 된다. 지워버리면
    /// 방금 실패한 서버가 정상인 것처럼 보인다.
    #[tokio::test]
    async fn a_redundant_enable_does_not_hide_a_live_failure() {
        let (mut b, _rx) = bridge();
        b.set_enabled(true);
        b.apply_event(&ServerEvent::BindFailed("포트 4320 이 이미 쓰이는 중".into()));
        b.set_enabled(true);
        assert!(b.last_error().is_some());
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
}
