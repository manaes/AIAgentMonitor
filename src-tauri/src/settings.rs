//! 사용자가 화면에 표시할 에이전트 종류를 고르는 설정. `~/.config/ai-agent-monitor/`
//! 규약을 따른다(`ble/peers.rs`/`network/peers.rs` 와 동일).
//!
//! 이 파일은 자격증명을 담지 않는 단순 UI 선호도라 `ble/peers.rs` 만큼 엄격할 필요가
//! 없다 — 파일이 없거나 손상돼도 조용히 "전체 표시"(기존 동작)로 돌아간다.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::types::AgentKind;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub enabled_agents: HashSet<AgentKind>,
}

impl Default for AppSettings {
    /// 기존 사용자가 설정 탭을 한 번도 안 건드려도 동작이 그대로여야 한다 —
    /// 기본값은 전체 활성이다.
    fn default() -> Self {
        Self { enabled_agents: HashSet::from([AgentKind::Claude, AgentKind::Codex, AgentKind::Antigravity]) }
    }
}

pub struct SettingsStore;

impl SettingsStore {
    pub fn path() -> PathBuf {
        dirs_next::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai-agent-monitor/settings.json")
    }

    /// 파일이 없거나 손상됐으면 조용히 기본값(전체 활성)으로 돌아간다.
    pub fn load_from(path: &Path) -> AppSettings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 임시 파일에 쓴 뒤 rename 한다 — 쓰는 도중 죽어도 기존 파일이 반쯤
    /// 덮인 채로 남지 않는다(`ble/peers.rs` 와 같은 이유).
    pub fn save_to(path: &Path, settings: &AppSettings) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if tmp.exists() {
            std::fs::remove_file(&tmp)?;
        }
        std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_every_known_agent() {
        let s = AppSettings::default();
        assert!(s.enabled_agents.contains(&AgentKind::Claude));
        assert!(s.enabled_agents.contains(&AgentKind::Codex));
        assert!(s.enabled_agents.contains(&AgentKind::Antigravity));
    }

    #[test]
    fn missing_file_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        assert_eq!(SettingsStore::load_from(&p), AppSettings::default());
    }

    #[test]
    fn corrupt_file_loads_as_default_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, b"not json").unwrap();
        assert_eq!(SettingsStore::load_from(&p), AppSettings::default());
    }

    #[test]
    fn save_then_load_round_trips_a_partial_selection() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let saved = AppSettings { enabled_agents: HashSet::from([AgentKind::Codex]) };
        SettingsStore::save_to(&p, &saved).unwrap();
        assert_eq!(SettingsStore::load_from(&p), saved);
    }

    #[test]
    fn save_replaces_the_file_rather_than_writing_in_place() {
        // ble/peers.rs 의 원자성 테스트와 같은 이유 — rename 은 새 inode 로
        // 교체하므로, 절반만 쓰인 내용이 보이는 창이 없다.
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        SettingsStore::save_to(&p, &AppSettings::default()).unwrap();
        let ino1 = std::fs::metadata(&p).unwrap().ino();
        SettingsStore::save_to(&p, &AppSettings { enabled_agents: HashSet::from([AgentKind::Claude]) }).unwrap();
        let ino2 = std::fs::metadata(&p).unwrap().ino();
        assert_ne!(ino1, ino2);
    }

    #[test]
    fn a_leftover_temp_file_does_not_block_future_saves() {
        // peers.rs 에서 실제로 있었던 버그와 같은 계열 — 이름에 pid 를 넣으므로
        // 남은 tmp 는 반드시 우리가 흘린 것이고, 지우지 않으면 다음 저장이
        // create_new 없이도 그냥 write 라 사실 이 프로젝트에서는 안 막히지만,
        // 저장 시작 전 정리 로직 자체가 잘 동작하는지는 확인해둔다.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let stale = p.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&stale, b"garbage").unwrap();
        SettingsStore::save_to(&p, &AppSettings::default()).unwrap();
        assert!(!stale.exists());
    }
}
