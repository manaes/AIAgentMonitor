//! 네트워크 전송의 페어링 토큰 영속화. `ble::peers::PeerStore`가 이미
//! 경로에 무관한 tmp+rename+0600 저장을 구현하고 있으므로 그대로 재사용하고,
//! 파일 경로만 분리한다(BLE와 네트워크는 별도 PairingManager 인스턴스를
//! 쓰므로 페어링 목록도 분리— Phase 5에서 신원 통합을 검토할 때 재논의).
use std::path::PathBuf;

pub use crate::ble::peers::{LoadOutcome, PeerStore, StoredPeer};

pub fn path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ai-agent-monitor/network-peers.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uses_network_specific_filename() {
        assert!(path().ends_with("ai-agent-monitor/network-peers.json"));
    }

    #[test]
    fn round_trips_via_shared_peer_store() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("network-peers.json");
        let peers = vec![StoredPeer { token: "a".repeat(32), paired_at: 1000 }];
        PeerStore::save_to(&p, &peers).unwrap();
        assert_eq!(PeerStore::load_from(&p), LoadOutcome::Loaded(peers));
    }
}
