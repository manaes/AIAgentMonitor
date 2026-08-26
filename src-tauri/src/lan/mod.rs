//! LAN 전송 브리지 (스펙 2026-08-25-cyd-client-design.md).
//!
//! `network/mod.rs`(iroh)와 같은 표면을 갖는다. WebSocket 연결 하나가 세션
//! 하나이고 `CentralId` 하나에 대응한다 — BLE 링크와 같은 모델이다. 서버
//! 자체(`server`, `discovery`)는 이후 태스크에서 붙는다 — 지금은 골격과
//! 상태만 정의한다.

use crate::ble::peripheral::CentralId;
use std::collections::HashSet;

/// LAN 전송이 지금 서비스 중인 central 들과, 사용자에게 보여줄 마지막 오류를
/// 들고 있는다. `network::NetworkBridge`와 표면을 맞춘 이유는 `lib.rs` 배선을
/// 세 전송(BLE/network/lan) 모두 같은 모양으로 유지해, 하나만 고치고 다른
/// 쪽을 잊는 드리프트를 줄이기 위해서다.
pub struct LanBridge {
    enabled: bool,
    /// 현재 붙어 있는 연결들. 서버 태스크가 갱신한다(이후 태스크).
    centrals: HashSet<String>,
    /// 사용자에게 보여줄 마지막 오류. 이 앱은 로그 파일을 남기지 않으므로
    /// Devices 패널이 실패 원인(포트 점유·권한 거부 등)을 알 수 있는 유일한
    /// 경로다.
    last_error: Option<String>,
}

impl LanBridge {
    pub fn new() -> Self {
        Self {
            enabled: false,
            centrals: HashSet::new(),
            last_error: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// BLE·network 브리지와 같은 이유로 상태를 정리한다 — 꺼졌다 켜졌을 때
    /// 예전 연결이 여전히 붙어 있는 것으로 남지 않게. LAN 공유는 기본 꺼짐이고
    /// 리스너는 이 토글이 켜져 있는 동안만 존재한다(스펙 4장) — 여기서는 아직
    /// 리스너를 띄우지 않지만 상태 정리 규칙은 미리 맞춰 둔다.
    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if !on {
            self.centrals.clear();
        }
    }

    /// 이 전송이 지금 서비스 중인 central 목록. BLE·network 의
    /// `served_centrals`와 같은 목적이다.
    pub fn served_centrals(&self) -> Vec<CentralId> {
        self.centrals.iter().cloned().map(CentralId).collect()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.clone()
    }
}

impl Default for LanBridge {
    fn default() -> Self {
        Self::new()
    }
}

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
        b.set_enabled(true);
        b.set_enabled(true);
        assert!(b.is_enabled());
        b.set_enabled(false);
        b.set_enabled(false);
        assert!(!b.is_enabled());
    }

    #[test]
    fn has_no_centrals_when_disabled() {
        assert!(LanBridge::new().served_centrals().is_empty());
    }
}
