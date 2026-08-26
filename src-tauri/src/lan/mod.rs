//! LAN 전송 브리지 (스펙 2026-08-25-cyd-client-design.md).
//!
//! `network/mod.rs`(iroh)와 같은 표면을 갖는다. WebSocket 연결 하나가 세션
//! 하나이고 `CentralId` 하나에 대응한다 — BLE 링크와 같은 모델이다. 실제
//! 리스너는 `server` 가 띄우고, 이 브리지는 "지금 누가 붙어 있고 무엇이
//! 잘못됐는지"만 안다(BLE 의 `peripheral` / 네트워크의 accept 루프와 같은
//! 역할 분리).

pub mod server;

use crate::ble::pairing::{self, PairingManager};
use crate::ble::peripheral::CentralId;
use crate::ble::wire::MirrorSnapshot;
use crate::emitter::EmitGate;
use crate::types::Snapshot;
use server::{Outbound, ServerEvent, ServerHandle};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::Sender;

/// 미러 갱신 주기. BLE·network 와 같은 값이다 — 두 전송을 나란히 켜고 껐을 때
/// 체감 차이가 없어야 하고, `server::SINK_QUEUE`(8) 의 "여덟이 밀렸다는 건
/// 상대가 8초째 읽지 않는다는 뜻" 이라는 계산이 바로 이 1Hz 를 전제한다.
/// 이 값을 줄이면 그 큐가 뜻하는 시간도 함께 줄어든다.
const LAN_THROTTLE: Duration = Duration::from_millis(1000);

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

impl AuthOutcome {
    /// 이 결과가 **프론트가 보는 상태**를 바꿨는가.
    ///
    /// 바꾸지 않았으면 상태 이벤트를 내보내지 않는다. 프레임은 인증 이전에
    /// 아무나 보낼 수 있고 소비자 속도에 맞춰 보내면 큐 상한에도 걸리지
    /// 않으므로, 프레임마다 내보내면 **인증하지 않은 피어가 시간 제한 없이
    /// 프론트를 계속 다시 그리게** 만들 수 있다. 형제 전송도 이벤트마다
    /// 내보내지만 거기서 이벤트를 만들려면 근접(BLE)하거나 상대가 걸어와야
    /// 한다(iroh) — LAN 리스너만 같은 WiFi 의 아무에게나 열려 있다.
    ///
    /// 핸드셰이크 중간 응답(`AwaitingCode2`/`Nonce2`)이나 거절은 패널에 보이는
    /// 것을 아무것도 바꾸지 않는다. 인가와 발급만 바꾼다.
    pub fn changed_visible_state(&self) -> bool {
        self.now_authorized || self.granted
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
    /// 미러 갱신 게이트. 앱의 틱은 250ms 인데 미러는 1Hz 로 묶는다
    /// (`LAN_THROTTLE`). 내용이 그대로면 아예 내보내지 않으므로, 조용한
    /// 시간에는 봉인 카운터도 전진하지 않는다.
    gate: EmitGate,
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
            gate: EmitGate::new(LAN_THROTTLE),
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
            // 다시 켰으면 첫 스냅샷은 곧바로 나가야 한다. 게이트는 "내용이
            // 같으면 안 보낸다"이므로, 되돌리지 않으면 꺼져 있는 동안 아무것도
            // 바뀌지 않은 경우 다시 켠 기기가 변화가 생길 때까지 빈 화면을 본다.
            self.gate.reset();
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
    ///
    /// **값이 실제로 바뀌었을 때만 `true`.** 호출부는 이걸 보고 프론트에 알릴지
    /// 정한다 — 같은 오류를 다시 쓰는 것은 사용자에게 새 소식이 아니고, "바뀌지
    /// 않아도 매번 알린다"는 것이 finding A 에서 고친 바로 그 형태다.
    pub fn set_last_error(&mut self, msg: Option<String>) -> bool {
        if self.last_error == msg {
            return false;
        }
        self.last_error = msg;
        true
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
        // 판정은 `AuthReply` 를 소유한 pairing 모듈에 한 벌만 있다 — 세 전송이
        // 각자 베끼면 한쪽만 고치고 다른 쪽을 잊는다(`ReplySignals` 의 doc).
        let s = reply.signals();
        Some(AuthOutcome {
            payload: reply.to_json_bytes(),
            now_authorized: s.authorized,
            granted: s.granted,
        })
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

    /// 이 연결이 인가됐음을 서버에 알린다 — 그래야 서버가 이 연결의 **인증
    /// 시한**을 걷는다(`server::AUTH_DEADLINE`). 호출부는 `handle_auth` 가
    /// `now_authorized` 를 세운 바로 그 자리에서 부른다.
    ///
    /// 인가 자체는 공유 `PairingManager` 가 갖고 있는데도 서버에 따로 알려야
    /// 하는 이유는, 연결 핸들러가 그 매니저를 볼 수 없기 때문이다 — 볼 수 있게
    /// 만들면 연결 태스크 여덟 개가 페어링 잠금을 다투게 된다. 시한이 필요로
    /// 하는 것은 불리언 하나이므로 그것만 건넨다.
    ///
    /// 리스너가 없으면(토글이 꺼졌다) 조용히 버려진다. 그 경우 `handle_auth`
    /// 도 애초에 아무도 인가하지 않는다(`serves`).
    pub fn mark_authorized(&self, central: &CentralId) {
        if let Some(h) = &self.server {
            let _ = h.outbound.send(Outbound::Authorized(central.clone()));
        }
    }

    /// 스냅샷을 보낼 대상. **인가되지 않은 연결은 들어가지 않는다** — 붙어
    /// 있다는 것과 볼 자격이 있다는 것은 다르다. "누구에게 보낼 수 있는가"는
    /// 인증의 결론이므로 봉인(`prepare_snapshot`)과 떼어 여기서 정한다.
    pub fn snapshot_targets(&self, pairing: &PairingManager) -> Vec<CentralId> {
        self.centrals
            .iter()
            .filter(|id| self.serves(id) && pairing.is_authorized(id))
            .cloned()
            .collect()
    }

    /// 스냅샷 틱의 **앞쪽 절반** — 이번 틱에 각 central 에게 나갈 봉인 프레임을
    /// 만든다. 인가된 central 이 하나도 없거나 꺼져 있으면 빈 목록이다(BLE 의
    /// "구독자 없으면 직렬화도 안 함" 과 같은 절약).
    ///
    /// **쓰기와 나눠 둔 이유는 페어링 잠금 시간이다.** 봉인에는 `&mut
    /// PairingManager` 가 필요하지만(카운터가 전진한다) 쓰기에는 필요 없다.
    /// 그래서 호출부는 이 함수까지만 잠금 안에서 부르고, 잠금을 놓은 뒤
    /// `send_prepared` 를 부른다 — `network::prepare_snapshot` 과 같은 모양이다.
    ///
    /// 정직하게 말하면 **LAN 의 쓰기는 오늘 막히지 않는다**(`send_prepared` 의
    /// doc). 그런데도 같은 모양을 유지하는 이유는 두 가지다: 세 전송의 표면이
    /// 같아야 배선에서 한쪽만 고치는 드리프트가 눈에 띄고, 봉인과 쓰기가 함수
    /// 경계로 갈라져 있어야 나중에 이 쓰기가 정말 await 를 타게 되어도 잠금이
    /// 이미 바깥에 있기 때문이다.
    ///
    /// 게이트를 대상 검사 **뒤에** 두는 것은 일부러다. 아무도 없는데 게이트를
    /// 소비하면, 기기가 붙은 직후의 첫 스냅샷이 "방금 보냈다"는 이유로 밀린다.
    pub fn prepare_snapshot(
        &mut self,
        snap: &Snapshot,
        now: SystemTime,
        pairing: &mut PairingManager,
    ) -> Vec<(CentralId, Vec<u8>)> {
        if !self.enabled {
            return Vec::new();
        }
        let targets = self.snapshot_targets(pairing);
        if targets.is_empty() {
            return Vec::new();
        }
        if !self.gate.should_emit(snap, now) {
            return Vec::new();
        }

        let json = match serde_json::to_vec(&MirrorSnapshot::from(snap)) {
            Ok(j) => j,
            Err(e) => {
                // `last_error` 에 싣지 않는다 — 그 필드는 사용자가 고칠 수 있는
                // 실패(포트 점유·권한 거부)만 담는다는 것이 이 모듈의 규칙이고
                // (`send_auth_reply`·`upgrade` 의 doc), 직렬화 실패는 사용자가
                // 할 수 있는 일이 없는 우리 쪽 결함이다. 게이트는 이미 "보냈다"로
                // 커밋했으므로 되돌려 다음 틱에 다시 시도되게 한다.
                tracing::error!("LAN 스냅샷 직렬화 실패: {e}");
                self.gate.reset();
                return Vec::new();
            }
        };

        // 봉인은 central 마다 다른 키와 카운터를 쓰므로 프레임도 central 마다
        // 따로 만든다. 하나가 빠져도 나머지는 그대로 나간다.
        let mut out = Vec::with_capacity(targets.len());
        for central in targets {
            if let Some(frame) = sealed_frame(pairing, &central, &json) {
                out.push((central, frame));
            }
        }
        out
    }

    /// 스냅샷 틱의 **뒤쪽 절반** — 준비된 프레임을 각 central 의 큐로 넘긴다.
    /// 페어링 잠금 없이 돈다(`prepare_snapshot` 의 doc).
    ///
    /// **여기서 실제로 막히는 일은 없다.** `outbound` 는 무한 채널이라
    /// `send` 가 기다리지 않는다. 상대가 읽지 않아 밀리는 것은 그 뒤의 central
    /// 별 큐(`server::SINK_QUEUE`)가 받아 내고, 밀린 연결은 서버가 놓는다 —
    /// 이 함수는 그 판단을 하지 않는다.
    ///
    /// 한 central 의 실패가 다른 central 을 막지 않는다. 리스너가 그 사이
    /// 내려갔으면 전부 조용히 버려지고, 다음 틱이 새 프레임을 만든다 — 버려진
    /// 프레임의 카운터는 되돌리지 않는다(`sealed_frame` 의 doc).
    pub async fn send_prepared(&mut self, frames: Vec<(CentralId, Vec<u8>)>) {
        let Some(h) = &self.server else {
            return;
        };
        for (central, frame) in frames {
            // 봉인 프레임은 UTF-8 이 아니다 — 반드시 바이너리로 나가야 한다.
            let _ = h.outbound.send(Outbound::Binary(central, frame));
        }
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

/// 이 central 에게 실제로 나갈 봉인 프레임(`counter || ciphertext || tag`).
/// 인가되지 않았거나 v2 세션이 아니면 `None` — **한 바이트도 나가면 안 된다.**
///
/// **인가 검사가 여기 또 있는 이유.** `snapshot_targets` 가 이미 걸렀다. 그래도
/// 다시 보는 것은, 이 함수가 나갈 바이트를 만드는 유일한 지점이고 그 지점이
/// 스스로 닫혀 있어야 하기 때문이다. 형제 전송에서 정확히 이 검사가 없어서,
/// 언페어링과 스냅샷 틱이 겹치는 창에 **방금 해제한 기기에게 평문 JSON** 이
/// 나갔다(`network::snapshot_line` 의 doc). 지금은 대상 선정과 봉인 사이에
/// await 가 없어 그 창이 없지만, "창이 없다"에 기대는 대신 검사를 둔다.
///
/// **v1 세션에는 아무것도 보내지 않는다.** 형제 전송은 채널이 없으면 평문 JSON
/// 을 보낸다 — BLE 는 10m 안에서, iroh 는 상대가 걸어온 QUIC 위에서다. LAN 은
/// 망 전체에 열려 있고, 이 전송이 받아들여진 근거 자체가 "미러가 봉인돼 있어
/// 같은 WiFi 의 다른 기기가 읽을 수 없다"였다(스펙 7.2). 그러니 여기서 평문으로
/// 떨어지는 것은 전환기 호환이 아니라 downgrade 다. 세대 정책은 여전히
/// `PairingManager` 가 정한다 — v1 로 붙는 것을 이 전송이 막지는 않는다
/// (`handle_auth` 의 doc). 다만 **그 세션에 평문을 실어 보내지는 않는다.**
///
/// LAN 클라이언트는 E2EE v2 이후에만 존재하므로 실제로 이 갈래에 닿을 기기는
/// 없다. 닿았다면 그 자체가 알아야 할 사실이라 흔적을 남긴다.
///
/// **카운터는 되돌리지 않는다.** 봉인이 끝난 순간 `(키, 논스)` 한 쌍이 소비된다.
/// 이 프레임이 끝내 나가지 못해도 다음 프레임은 다음 카운터를 쓴다 — 수신 측은
/// 카운터의 빈 칸을 견디지만(`SealedChannel::open`), 같은 논스를 두 번 쓰는 것은
/// ChaCha20-Poly1305 에서 회복 불가능한 사고다.
fn sealed_frame(
    pairing: &mut PairingManager,
    central: &CentralId,
    json: &[u8],
) -> Option<Vec<u8>> {
    if !pairing.is_authorized(central) {
        return None;
    }
    match pairing.channel_mut(central) {
        Some(ch) => Some(ch.seal(json)),
        None => {
            tracing::warn!(
                id = %central.0,
                "LAN 세션에 봉인 채널이 없다 — 평문을 내보내는 대신 건너뛴다"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::pairing::test_client::{self, hex_encode, V2Client};
    use crate::ble::pairing::AuthReply;
    use crate::crypto::{self, channel::SealedChannel};
    use crate::types::{
        ActivityStatus, AgentKind, AgentState, ProjectActivity, TokenCounts,
    };
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;
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
    ///
    /// 클라이언트 쪽 `SealedChannel` 도 함께 돌려준다. 그래야 스냅샷 테스트가
    /// "봉인된 것처럼 보인다"가 아니라 **이 기기가 실제로 열 수 있는가**를 볼 수
    /// 있다 — 앞의 것은 키가 어긋나도 통과한다.
    fn v2_pair(
        b: &mut LanBridge,
        p: &mut PairingManager,
        central: &CentralId,
        now: SystemTime,
    ) -> (AuthOutcome, SealedChannel) {
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
        let (_token, ch) = test_client::open_pairing_and_session(&ss, &nonce, &sealed);
        (out, ch)
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

        let (out, _ch) = v2_pair(&mut b, &mut p, &id, t(1000));

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

    /// 인증하지 않은 피어가 프레임만 보내서 프론트를 계속 다시 그리게 만들 수
    /// 있으면 안 된다. 상태를 실제로 바꾼 결과만 이벤트가 된다.
    /// (응답 10종의 판정 자체는 `ble::pairing` 의 `every_reply_has_signals` 가 본다.)
    #[test]
    fn only_a_real_change_is_worth_telling_the_frontend() {
        let out = |now_authorized, granted| AuthOutcome {
            payload: Vec::new(),
            now_authorized,
            granted,
        };
        assert!(out(true, true).changed_visible_state(), "새 페어링은 알려야 한다");
        assert!(out(true, false).changed_visible_state(), "재연결도 알려야 한다");
        assert!(
            !out(false, false).changed_visible_state(),
            "거절·핸드셰이크 중간 응답은 패널에 보이는 것을 바꾸지 않는다"
        );
    }

    /// 인증 프레임을 아무리 보내도, 통과하지 못하면 알릴 것이 없다.
    /// `handle_auth` 를 실제로 태워 확인한다 — 위 표가 손으로 만든 값이라면
    /// 이쪽은 진짜 응답에서 나온 값이다.
    #[tokio::test]
    async fn a_rejected_frame_is_not_worth_telling_the_frontend() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();

        // 창이 닫혀 있으므로 무엇을 보내도 Rejected 다.
        for frame in [&b"HELLO"[..], b"HELLO2:short", b"PROOF:zz"] {
            let out = b.handle_auth(&id, frame, t(1000), &mut p).expect("서비스 중이다");
            assert!(
                !out.changed_visible_state(),
                "통과하지 못한 프레임이 프론트 갱신을 유발하면 안 된다"
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

    /// 같은 오류를 다시 쓰는 것은 사용자에게 새 소식이 아니다. 호출부가 그걸로
    /// 프론트 갱신 여부를 정하므로, 바뀌었는지를 이 함수가 답해야 한다.
    #[tokio::test]
    async fn setting_the_same_error_twice_is_not_news() {
        let (mut b, _rx) = bridge();
        assert!(b.set_last_error(Some("저장 실패".into())), "처음은 바뀐 것이다");
        assert!(!b.set_last_error(Some("저장 실패".into())), "같은 값은 새 소식이 아니다");
        assert!(b.set_last_error(Some("다른 실패".into())), "내용이 바뀌면 알려야 한다");
        assert!(b.set_last_error(None), "지우는 것도 바뀐 것이다");
        assert!(!b.set_last_error(None), "이미 비어 있으면 바뀐 것이 없다");
    }

    /// 리스너가 없을 때 응답을 보내려 해도 조용히 버려진다. 상대가 사라진 것은
    /// 사용자가 고칠 수 있는 실패가 아니므로 `last_error` 를 건드리면 안 된다.
    #[test]
    fn replying_with_no_listener_is_harmless() {
        let (b, _rx) = bridge();
        b.send_auth_reply(&server::central_id(0), b"{}".to_vec());
        assert!(b.last_error().is_none());
    }

    /// 리스너가 없을 때의 인가 통지도 마찬가지다. 토글이 꺼져 있으면 애초에
    /// 아무도 인가되지 않지만(`serves`), 그 성질에 기대지 않는다.
    #[test]
    fn marking_authorized_with_no_listener_is_harmless() {
        let (b, _rx) = bridge();
        b.mark_authorized(&server::central_id(0));
        assert!(b.last_error().is_none());
    }

    // --- 봉인 스냅샷 (Task 4) ---

    /// 미러 한 장. `rate` 를 인자로 받는 이유는 게이트가 내용 해시를 보기
    /// 때문이다 — 같은 값을 두 번 넣으면 두 번째는 나가지 않는다.
    fn sample_snapshot(rate: f32) -> Snapshot {
        Snapshot {
            emitted_at: UNIX_EPOCH + Duration::from_secs(1_755_500_000),
            agents: vec![AgentState {
                kind: AgentKind::Claude,
                rate_tok_per_sec: rate,
                tokens_5h: TokenCounts {
                    tokens_in: 1_000,
                    tokens_out: 2_000,
                    tokens_cache_read: 40_000,
                    tokens_cache_create: 7_000,
                },
                quota_limit: None,
                quota_reset_at: Some(UNIX_EPOCH + Duration::from_secs(1_755_512_400)),
                quota_used_pct: Some(62.0),
                quota_reset_at_weekly: None,
                quota_used_pct_weekly: None,
                projects: vec![ProjectActivity {
                    path: PathBuf::from("/Users/me/dev/foo"),
                    name: "foo".to_string(),
                    model: "claude-opus-5".to_string(),
                    rate_tok_per_sec: 98.25,
                    last_event_at: UNIX_EPOCH + Duration::from_secs(1_755_499_987),
                    status: ActivityStatus::Active,
                }],
            }],
        }
    }

    /// 봉인 프레임 하나가 나가고, **그 기기가 실제로 연다.** "평문이 아니다"
    /// 까지만 보면 키가 어긋나도 통과하므로 여기서 열어 본다.
    ///
    /// LAN 은 청킹하지 않는다 — WebSocket 이 프레이밍을 하므로 프레임 하나가
    /// 그대로 메시지 하나다.
    #[tokio::test]
    async fn a_v2_session_gets_one_sealed_frame_it_can_open() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        let (_out, mut client) = v2_pair(&mut b, &mut p, &id, t(1000));

        let snap = sample_snapshot(1.0);
        let frames = b.prepare_snapshot(&snap, t(1001), &mut p);

        assert_eq!(frames.len(), 1, "청킹하지 않으므로 한 건뿐이다");
        let (target, frame) = &frames[0];
        assert_eq!(target, &id);
        assert!(!frame.starts_with(b"{"), "평문 JSON 이 나갔다");
        assert!(frame.len() > 8 + 16, "카운터 8 + 태그 16 보다 길어야 한다");

        let opened = client.open(frame).expect("이 기기의 세션 키로 열려야 한다");
        assert_eq!(
            opened,
            serde_json::to_vec(&MirrorSnapshot::from(&snap)).unwrap(),
            "열고 나면 이번 틱의 미러 DTO 그대로여야 한다"
        );
    }

    /// 인가되지 않은 연결에는 **0바이트**다. 붙어 있다는 것과 볼 자격이 있다는
    /// 것은 다르다.
    #[tokio::test]
    async fn an_unauthorized_connection_receives_zero_bytes() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        p.begin_pairing(t(1000));

        assert!(b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).is_empty());

        // 핸드셰이크 중간(AwaitingCode2)도 아직 아니다 — 사람이 코드를 넣기
        // 전까지는 인가가 아니다.
        let c = V2Client::new();
        b.handle_auth(&id, format!("HELLO2:{}", hex_encode(&c.public)).as_bytes(), t(1001), &mut p);
        assert!(b.prepare_snapshot(&sample_snapshot(2.0), t(1002), &mut p).is_empty());
    }

    /// 인가된 기기 옆에 낯선 기기가 붙어 있어도 그쪽에는 아무것도 나가지 않고,
    /// 그렇다고 인가된 기기까지 막히지도 않는다.
    #[tokio::test]
    async fn a_stranger_beside_a_paired_device_gets_nothing() {
        let paired = server::central_id(0);
        let stranger = server::central_id(1);
        let (mut b, _rx) = live_bridge(&paired);
        b.apply_event(&ServerEvent::Connected(stranger.clone()));
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &paired, t(1000));

        let frames = b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, paired, "낯선 기기에게 나간 프레임이 있다");
    }

    /// 인가된 뒤에도 공유를 끄면 아무것도 나가지 않는다.
    #[tokio::test]
    async fn turning_sharing_off_stops_the_bytes() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));

        b.set_enabled(false);
        b.apply_event(&ServerEvent::Connected(id.clone())); // 지각 이벤트
        assert!(b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).is_empty());
    }

    /// **언페어링된 기기에게는 다음 틱부터 0바이트.** 링크는 아직 붙어 있고
    /// 브리지의 `centrals` 에도 남아 있지만, 인가가 사라진 것으로 충분하다 —
    /// 형제 전송은 이 검사가 없어서 방금 해제한 기기에게 평문을 보냈다.
    #[tokio::test]
    async fn a_device_that_just_lost_authorization_gets_nothing() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));
        assert_eq!(b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).len(), 1);

        // 사용자가 이 기기를 해제했다(`unpair` → `revoke_peer`).
        let peer = p.paired_peers()[0].peer_id.clone();
        assert!(!p.revoke_peer(&peer).is_empty(), "해제할 세션이 있어야 한다");

        assert!(
            b.prepare_snapshot(&sample_snapshot(2.0), t(1002), &mut p).is_empty(),
            "해제한 기기에게 한 바이트라도 나가면 안 된다"
        );
    }

    /// **v1 세션에는 평문을 보내지 않는다.** 형제 전송은 채널이 없으면 평문
    /// JSON 을 보내지만, LAN 은 망 전체에 열려 있고 이 전송이 받아들여진 근거
    /// 자체가 "미러가 봉인돼 있다"였다(스펙 7.2). 여기서 평문으로 떨어지는 것은
    /// 전환기 호환이 아니라 downgrade 다.
    #[tokio::test]
    async fn a_v1_session_gets_no_plaintext_over_the_lan() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        let code = p.begin_pairing(t(1000));

        b.handle_auth(&id, b"HELLO", t(1001), &mut p);
        let out = b
            .handle_auth(&id, format!("CODE:{code}").as_bytes(), t(1002), &mut p)
            .expect("서비스 중이다");
        assert!(out.now_authorized, "v1 도 인가는 된다 — 세대 판단은 전송의 것이 아니다");
        assert!(p.channel_mut(&id).is_none(), "v1 이므로 봉인 채널이 없다");

        assert!(
            b.prepare_snapshot(&sample_snapshot(1.0), t(1003), &mut p).is_empty(),
            "봉인할 수 없으면 평문으로 떨어지지 말고 아무것도 보내지 않아야 한다"
        );
    }

    /// 미러는 1Hz 다. 앱의 틱은 250ms 이므로, 게이트가 없으면 초당 네 장이
    /// 나가고 `server::SINK_QUEUE`(8) 가 뜻하는 시간이 8초에서 2초로 줄어든다.
    #[tokio::test]
    async fn the_mirror_is_gated_to_one_frame_per_second() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));

        assert_eq!(b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).len(), 1);
        assert!(
            b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).is_empty(),
            "내용이 같으면 다시 보내지 않는다"
        );
        assert!(
            b.prepare_snapshot(&sample_snapshot(2.0), t(1001), &mut p).is_empty(),
            "내용이 달라도 1초 안이면 아직이다"
        );
        assert_eq!(
            b.prepare_snapshot(&sample_snapshot(2.0), t(1002), &mut p).len(),
            1,
            "1초가 지나고 내용도 바뀌었으면 나가야 한다"
        );
    }

    /// 아무도 인가되지 않았을 때는 게이트를 **소비하지 않는다.** 소비해 버리면
    /// 기기가 붙은 직후의 첫 미러가 "방금 보냈다"는 이유로 1초 밀린다.
    #[tokio::test]
    async fn an_empty_tick_does_not_spend_the_gate() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();

        // 아직 아무도 인가되지 않았다.
        assert!(b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).is_empty());

        v2_pair(&mut b, &mut p, &id, t(1001));
        assert_eq!(
            b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p).len(),
            1,
            "붙자마자의 첫 미러가 게이트에 걸렸다"
        );
    }

    /// **카운터는 전진하기만 한다.** 봉인이 끝난 순간 `(키, 논스)` 한 쌍이
    /// 소비되고, 그 프레임이 끝내 나가지 못해도 되돌리지 않는다 — 수신 측은
    /// 빈 칸을 견디지만 같은 논스를 두 번 쓰는 것은 회복 불가능한 사고다.
    ///
    /// 첫 프레임을 **보내지 않고 버린** 뒤 두 번째 프레임만 여는 것으로 그
    /// 성질을 확인한다. 카운터를 되돌렸다면 두 번째 프레임이 0번이 되어,
    /// 이미 0번을 본 적 없는 클라이언트에게는 그대로 열리므로 이 테스트는
    /// 카운터 자체를 본다.
    #[tokio::test]
    async fn a_dropped_frame_never_makes_a_counter_repeat() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        let (_out, mut client) = v2_pair(&mut b, &mut p, &id, t(1000));

        let first = b.prepare_snapshot(&sample_snapshot(1.0), t(1001), &mut p);
        let second = b.prepare_snapshot(&sample_snapshot(2.0), t(1002), &mut p);

        let counter = |f: &[u8]| u64::from_be_bytes(f[..8].try_into().unwrap());
        assert_eq!(counter(&first[0].1), 0);
        assert_eq!(counter(&second[0].1), 1, "버려진 프레임의 번호를 재사용했다");

        // 첫 프레임은 나가지 못했다고 치고 두 번째만 연다 — 수신 측은 빈 칸을
        // 견뎌야 한다.
        client.open(&second[0].1).expect("빈 칸이 있어도 열려야 한다");
    }

    /// **봉인된 스냅샷이 64 KiB 프레임 상한에 실제로 들어가는가.** 상한을
    /// 넘으면 서버가 자기 프레임을 보내지 못하는 것이 아니라(상한은 수신
    /// 방향이다) 기기 쪽 조립이 무너지므로, 값을 재서 못박아 둔다.
    ///
    /// 현실적인 최대치를 만든다: 에이전트 셋 각각에 프로젝트 24개, 이름과
    /// 모델명을 길게. 실제 사용에서 이보다 큰 미러는 나오기 어렵다.
    ///
    /// 2026-08-26 측정값: 이 무거운 스냅샷이 평문 9,649 · 봉인 9,673 바이트로
    /// 상한의 **14.8%** 다. 평범한 한 장(에이전트 하나·프로젝트 하나)은 188
    /// 바이트다. 상한을 올려야 할 이유는 지금 없다.
    #[tokio::test]
    async fn a_sealed_snapshot_fits_the_frame_budget_with_room_to_spare() {
        let id = server::central_id(0);
        let (mut b, _rx) = live_bridge(&id);
        let mut p = PairingManager::new();
        v2_pair(&mut b, &mut p, &id, t(1000));

        let mut snap = sample_snapshot(1.0);
        let base = snap.agents[0].clone();
        snap.agents = [AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity]
            .into_iter()
            .map(|kind| {
                let mut a = base.clone();
                a.kind = kind;
                a.projects = (0..24)
                    .map(|i| ProjectActivity {
                        path: PathBuf::from(format!("/Users/someone/work/monorepo/services/{i}")),
                        name: format!("service-with-a-fairly-long-name-{i}"),
                        model: "claude-opus-5-with-a-long-identifier".to_string(),
                        rate_tok_per_sec: 12.5,
                        last_event_at: UNIX_EPOCH + Duration::from_secs(1_755_499_987),
                        status: ActivityStatus::Active,
                    })
                    .collect();
                a
            })
            .collect();

        let frames = b.prepare_snapshot(&snap, t(1001), &mut p);
        let sealed = &frames[0].1;
        let plain = serde_json::to_vec(&MirrorSnapshot::from(&snap)).unwrap();

        // 봉인은 카운터 8 + 태그 16 만 더한다 — 압축도 패딩도 없다.
        assert_eq!(sealed.len(), plain.len() + 24);
        assert!(
            sealed.len() * 4 < server::MAX_FRAME_BYTES,
            "봉인된 스냅샷 {}바이트 — 64 KiB 상한의 4분의 1을 넘었다. \
             상한을 올리기 전에 미러 DTO 가 왜 이렇게 커졌는지 먼저 보라.",
            sealed.len()
        );
    }

    /// 봉인 프레임이 **진짜 소켓으로, 바이너리 프레임으로** 나가는 것까지 본다.
    /// 위의 테스트들은 전부 I/O 없는 상태 기계 호출이라, 펌프가 이것을 텍스트로
    /// 옮기거나(봉인 바이트는 UTF-8 이 아니라 손실된다) 아예 흘려버려도 잡히지
    /// 않는다. 여기가 그 한 겹을 덮는다.
    #[tokio::test]
    async fn a_sealed_snapshot_arrives_over_the_socket_as_one_binary_frame() {
        use server::test_socket::*;

        let port = server::test_socket::free_port().await;
        let (tx, mut rx) = channel(server::EVENT_QUEUE);
        let mut b = LanBridge::with_port(tx, port);
        let mut p = PairingManager::new();
        b.set_enabled(true);
        wait_until_listening(port).await;

        let mut sock = handshake(port).await;
        let ev = rx.recv().await.expect("Connected 가 와야 한다");
        b.apply_event(&ev);
        let ServerEvent::Connected(id) = ev else {
            panic!("Connected 를 기대했다");
        };

        // `v2_pair` 는 `handle_auth` 만 태운다 — 응답을 소켓에 쓰는 것은
        // `send_auth_reply` 이고 여기서는 부르지 않으므로, 소켓으로 나가는
        // 첫 바이트는 아래 스냅샷이 된다.
        let (_out, mut client) = v2_pair(&mut b, &mut p, &id, t(1000));

        let snap = sample_snapshot(1.0);
        let frames = b.prepare_snapshot(&snap, t(1001), &mut p);
        b.send_prepared(frames).await;

        let got = read_binary_frame(&mut sock).await;
        assert_eq!(
            client.open(&got).expect("이 기기의 세션 키로 열려야 한다"),
            serde_json::to_vec(&MirrorSnapshot::from(&snap)).unwrap()
        );

        b.set_enabled(false);
    }
}
