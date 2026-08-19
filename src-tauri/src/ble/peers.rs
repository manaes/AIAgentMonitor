//! 페어링 토큰 영속화. 기존 `triggers.json` 과 같은 디렉토리 규약을 따른다.
//!
//! 저장은 임시 파일에 쓴 뒤 rename 한다. 쓰는 도중 앱이 죽어도 기존 파일이
//! 반쯤 덮인 채로 남지 않게 하기 위함이다 — 토큰이 깨지면 이미 페어링한
//! 기기가 전부 재페어링을 요구받는다.
use std::path::{Path, PathBuf};

pub struct PeerStore;

impl PeerStore {
    pub fn path() -> PathBuf {
        dirs_next::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai-agent-monitor/ble-peers.json")
    }

    /// 없거나 손상된 파일은 빈 목록으로 취급한다. 여기서 죽으면 앱이 시작조차 못 한다.
    pub fn load_from(path: &Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        match serde_json::from_str::<Vec<String>>(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "ble-peers.json 파싱 실패, 빈 목록으로 시작");
                Vec::new()
            }
        }
    }

    pub fn save_to(path: &Path, tokens: &[String]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(tokens)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_follows_existing_config_convention() {
        let p = PeerStore::path();
        assert!(p.ends_with("ai-agent-monitor/ble-peers.json"),
                "triggers.json 과 같은 디렉토리여야 한다: {p:?}");
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        assert!(PeerStore::load_from(&p).is_empty(), "없는 파일은 빈 목록");
    }

    #[test]
    fn round_trips_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        let tokens = vec!["a".repeat(32), "b".repeat(32)];
        PeerStore::save_to(&p, &tokens).unwrap();
        let back = PeerStore::load_from(&p);
        assert_eq!(back, tokens);
    }

    #[test]
    fn corrupt_file_loads_as_empty_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        std::fs::write(&p, b"{ this is not json").unwrap();
        assert!(PeerStore::load_from(&p).is_empty(),
                "손상된 파일 때문에 앱이 죽으면 안 된다 — 빈 목록으로 시작한다");
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/deeper/ble-peers.json");
        PeerStore::save_to(&p, &["c".repeat(32)]).unwrap();
        assert_eq!(PeerStore::load_from(&p).len(), 1);
    }

    #[test]
    fn save_is_atomic_leaving_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        PeerStore::save_to(&p, &["d".repeat(32)]).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "ble-peers.json")
            .collect();
        assert!(leftovers.is_empty(), "임시 파일이 남으면 안 된다: {leftovers:?}");
    }
}
