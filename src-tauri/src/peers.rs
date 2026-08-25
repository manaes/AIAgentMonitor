//! 페어링 토큰 영속화. 앱 설정 디렉토리(`config_dir()`) 규약을 따른다.
//!
//! **BLE 와 네트워크가 이 저장소를 공유한다**(2026-08-25 스펙 5장). 예전에는
//! 전송마다 파일이 따로였는데(`ble-peers.json` / `network-peers.json`), 페어링
//! 자체가 앱 레벨로 올라가면서 토큰 집합도 하나가 됐다 — 토큰이 곧 기기
//! 정체성이므로(`peer_id = hex(SHA-256(토큰))[..8]`) 전송별로 나뉠 이유가 없다.
//!
//! 저장은 임시 파일에 쓴 뒤 rename 한다. 쓰는 도중 앱이 죽어도 기존 파일이
//! 반쯤 덮인 채로 남지 않게 하기 위함이다 — 토큰이 깨지면 이미 페어링한
//! 기기가 전부 재페어링을 요구받는다. `rename` 은 대상 파일을 다른 inode 로
//! 교체하므로(직접 쓰기는 같은 inode 를 덮어쓴다), 절반만 쓰인 내용이
//! 보이는 창을 없앤다.
//!
//! 저장된 토큰은 영구 자격증명이다 — 이 파일을 읽을 수 있는 프로세스는
//! 논스에 서명해 인가를 통과할 수 있다. 그래서 파일은 **만들어질 때부터**
//! 0600(소유자만 읽기/쓰기)이어야 한다. 만든 뒤 `set_permissions` 로 고치면
//! 그 사이 짧은 창에 다른 프로세스가 읽을 수 있다.
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// 파일에 저장되는 항목 하나. 토큰이 기기 정체성의 근거이므로(스펙 6장,
/// `pairing::PairingManager::peer_id_of`), 여기서는 원본 토큰과 페어링 시각만
/// 그대로 들고 있는다 — peer_id 파생은 `pairing` 모듈의 책임이다.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct StoredPeer {
    pub token: String,
    pub paired_at: u64,
}

/// `load_from` 의 결과. 손상된 파일과 없는 파일을 구분해 돌려준다 — 이
/// 프로젝트에는 `tracing-subscriber` 가 없어(의존성에도, 초기화 코드에도
/// 없음) `tracing::warn!` 이 전부 버려진다. 파일이 깨지면 페어링이 전부
/// 조용히 사라지므로, 호출자가 `Corrupt` 를 받아 Devices 패널에 직접 띄울
/// 수 있어야 한다(1단계가 BLE 오류에 쓴 방식과 동일 — `DEVICE-TEST.md` §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// 파일이 없다 — 첫 실행. 정상이다.
    Missing,
    /// 정상적으로 읽었다.
    Loaded(Vec<StoredPeer>),
    /// 파일은 있는데 파싱에 실패했다. 페어링이 전부 사라지므로 사용자에게 알려야 한다.
    Corrupt { detail: String },
}

pub struct PeerStore;

impl PeerStore {
    pub fn path() -> PathBuf {
        Self::config_dir().join("ai-agent-monitor/paired-peers.json")
    }

    /// 통합 이전에 BLE 전송이 쓰던 파일. 마이그레이션에서만 읽는다.
    pub fn legacy_ble_path() -> PathBuf {
        Self::config_dir().join("ai-agent-monitor/ble-peers.json")
    }

    /// 통합 이전에 네트워크 전송이 쓰던 파일. 마이그레이션에서만 읽는다.
    pub fn legacy_network_path() -> PathBuf {
        Self::config_dir().join("ai-agent-monitor/network-peers.json")
    }

    fn config_dir() -> PathBuf {
        dirs_next::config_dir().unwrap_or_else(|| PathBuf::from("."))
    }

    /// 통합 저장소를 읽는다. 아직 없으면 옛 두 파일을 합쳐 만들어 둔다 —
    /// 기존 사용자가 재페어링하지 않아도 되도록.
    ///
    /// 같은 토큰이 양쪽에 있으면 `paired_at` 이 **이른 쪽**을 남긴다(그 기기를
    /// 처음 페어링한 시각이 맞다). 옛 파일은 지우지 않는다 — 이 버전을 되돌릴
    /// 여지를 남긴다.
    ///
    /// 마이그레이션 저장이 실패해도 읽어낸 목록은 그대로 돌려준다. 이번 실행은
    /// 정상 동작하고 다음 실행에서 다시 시도된다 — 여기서 죽으면 페어링이
    /// 전부 사라진 것처럼 보인다.
    pub fn load_or_migrate(
        path: &Path,
        legacy_ble: &Path,
        legacy_network: &Path,
    ) -> LoadOutcome {
        match Self::load_from(path) {
            LoadOutcome::Missing => {}
            outcome => return outcome,
        }

        let mut merged: Vec<StoredPeer> = Vec::new();
        for legacy in [legacy_ble, legacy_network] {
            let LoadOutcome::Loaded(peers) = Self::load_from(legacy) else {
                continue;
            };
            for peer in peers {
                match merged.iter_mut().find(|m| m.token == peer.token) {
                    Some(existing) => existing.paired_at = existing.paired_at.min(peer.paired_at),
                    None => merged.push(peer),
                }
            }
        }

        if merged.is_empty() {
            return LoadOutcome::Missing;
        }
        if let Err(e) = Self::save_to(path, &merged) {
            tracing::error!(%e, "통합 페어링 저장소 마이그레이션 저장 실패");
        }
        LoadOutcome::Loaded(merged)
    }

    /// 파일이 없으면 `Missing`, 손상됐으면 `Corrupt`(원인 포함), 정상이면
    /// `Loaded` 를 돌려준다. 어느 경우든 패닉하지 않는다 — 여기서 죽으면
    /// 앱이 시작조차 못 한다.
    pub fn load_from(path: &Path) -> LoadOutcome {
        let Ok(text) = std::fs::read_to_string(path) else {
            return LoadOutcome::Missing;
        };
        match serde_json::from_str::<Vec<StoredPeer>>(&text) {
            Ok(v) => LoadOutcome::Loaded(v),
            Err(e) => {
                // 지금은 no-op 이지만(구독자 없는 tracing), 나중에 전역
                // 로깅이 살아나면 여기서 바로 잡힌다.
                tracing::warn!(%e, "ble-peers.json 파싱 실패");
                LoadOutcome::Corrupt { detail: e.to_string() }
            }
        }
    }

    /// 임시 파일에 0600 으로 쓴 뒤 rename 한다. 부모 디렉토리가 없으면
    /// 0700 으로 새로 만들되, 이미 있으면 권한을 건드리지 않는다 — 같은
    /// 디렉토리에 사는 다른 설정 파일 소유의 디렉토리 권한을 남이
    /// 바꾸게 되기 때문이다.
    pub fn save_to(path: &Path, peers: &[StoredPeer]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        // pid 를 이름에 섞는 이유: 이 확장자만 쓰면 실패로 남은 tmp 파일이
        // 항상 같은 이름이라 다음 저장의 `create_new` 를 막는다. 앱은
        // 단일 인스턴스라 서로 다른 프로세스가 동시에 쓰는 위험은 낮지만,
        // pid 를 섞어두면 남의 진행 중인 저장을 지우지 않는다.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(peers)?;

        // 이름에 우리 pid 가 박혀 있으므로 이 파일이 남아 있다면 반드시
        // 우리가 앞서 실패하며 흘린 것이다 — 남의 진행 중인 저장이 아니다.
        // 지우지 않으면 아래 `create_new` 가 계속 AlreadyExists 로 실패해서
        // 재시작 전까지 저장이 통째로 막힌다. tracing 출력이 버려지는
        // 상태라(이 파일 상단 참고) 그 실패는 조용히 일어난다.
        if tmp.exists() {
            std::fs::remove_file(&tmp)?;
        }

        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&json)?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, &json)?;
        }

        // rename 은 대상을 새 inode 로 교체한다(직접 쓰기는 같은 inode 를
        // 덮어쓴다) — 그래서 절반만 쓰인 내용이 보이는 순간이 없다. rename
        // 은 권한도 보존하므로 최종 파일도 0600 그대로다.
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
        assert!(p.ends_with("ai-agent-monitor/paired-peers.json"),
                "앱 설정 디렉토리 아래여야 한다: {p:?}");
        assert!(PeerStore::legacy_ble_path().ends_with("ai-agent-monitor/ble-peers.json"));
        assert!(PeerStore::legacy_network_path().ends_with("ai-agent-monitor/network-peers.json"));
    }

    // ── 통합 저장소 마이그레이션 (2026-08-25 스펙 5장) ──

    fn peer(token_char: char, paired_at: u64) -> StoredPeer {
        StoredPeer { token: token_char.to_string().repeat(32), paired_at }
    }

    #[test]
    fn migration_merges_both_legacy_files() {
        let dir = tempfile::tempdir().unwrap();
        let (new, ble, net) = (
            dir.path().join("paired-peers.json"),
            dir.path().join("ble-peers.json"),
            dir.path().join("network-peers.json"),
        );
        PeerStore::save_to(&ble, &[peer('a', 100)]).unwrap();
        PeerStore::save_to(&net, &[peer('b', 200)]).unwrap();

        let LoadOutcome::Loaded(mut merged) = PeerStore::load_or_migrate(&new, &ble, &net) else {
            panic!("합쳐진 목록이 나와야 한다");
        };
        merged.sort_by(|x, y| x.token.cmp(&y.token));
        assert_eq!(merged, vec![peer('a', 100), peer('b', 200)]);

        // 다음 실행부터는 통합 파일만 읽도록 실제로 저장돼 있어야 한다.
        assert_eq!(PeerStore::load_from(&new), LoadOutcome::Loaded(merged));
        assert!(ble.exists() && net.exists(), "옛 파일은 지우지 않는다 — 되돌릴 여지");
    }

    #[test]
    fn migration_keeps_the_earlier_paired_at_for_a_shared_token() {
        let dir = tempfile::tempdir().unwrap();
        let (new, ble, net) = (
            dir.path().join("paired-peers.json"),
            dir.path().join("ble-peers.json"),
            dir.path().join("network-peers.json"),
        );
        PeerStore::save_to(&ble, &[peer('a', 500)]).unwrap();
        PeerStore::save_to(&net, &[peer('a', 100)]).unwrap();

        let LoadOutcome::Loaded(merged) = PeerStore::load_or_migrate(&new, &ble, &net) else {
            panic!()
        };
        assert_eq!(merged, vec![peer('a', 100)], "처음 페어링한 시각이 맞다");
    }

    #[test]
    fn migration_does_not_run_when_the_unified_file_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (new, ble, net) = (
            dir.path().join("paired-peers.json"),
            dir.path().join("ble-peers.json"),
            dir.path().join("network-peers.json"),
        );
        PeerStore::save_to(&new, &[peer('c', 1)]).unwrap();
        PeerStore::save_to(&ble, &[peer('a', 100)]).unwrap();

        assert_eq!(
            PeerStore::load_or_migrate(&new, &ble, &net),
            LoadOutcome::Loaded(vec![peer('c', 1)]),
            "통합 파일이 있으면 옛 파일을 읽지 않는다(마이그레이션은 1회)"
        );
    }

    #[test]
    fn migration_reports_missing_when_there_is_nothing_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            PeerStore::load_or_migrate(
                &dir.path().join("paired-peers.json"),
                &dir.path().join("ble-peers.json"),
                &dir.path().join("network-peers.json"),
            ),
            LoadOutcome::Missing,
            "첫 실행 — 없는 게 정상이고 파일을 만들지도 않는다"
        );
        assert!(!dir.path().join("paired-peers.json").exists());
    }

    #[test]
    fn migration_survives_one_corrupt_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let (new, ble, net) = (
            dir.path().join("paired-peers.json"),
            dir.path().join("ble-peers.json"),
            dir.path().join("network-peers.json"),
        );
        std::fs::write(&ble, b"not json").unwrap();
        PeerStore::save_to(&net, &[peer('b', 200)]).unwrap();

        assert_eq!(
            PeerStore::load_or_migrate(&new, &ble, &net),
            LoadOutcome::Loaded(vec![peer('b', 200)]),
            "한쪽이 깨져도 나머지는 살린다"
        );
    }

    #[test]
    fn missing_file_is_reported_as_missing_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        assert_eq!(PeerStore::load_from(&p), LoadOutcome::Missing, "없는 파일은 Missing 이어야 한다");
    }

    #[test]
    fn round_trips_peers() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        let peers = vec![
            StoredPeer { token: "a".repeat(32), paired_at: 1000 },
            StoredPeer { token: "b".repeat(32), paired_at: 2000 },
        ];
        PeerStore::save_to(&p, &peers).unwrap();
        assert_eq!(PeerStore::load_from(&p), LoadOutcome::Loaded(peers));
    }

    #[test]
    fn valid_file_is_reported_as_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        let peers = vec![StoredPeer { token: "c".repeat(32), paired_at: 1 }];
        PeerStore::save_to(&p, &peers).unwrap();
        assert!(matches!(PeerStore::load_from(&p), LoadOutcome::Loaded(v) if v == peers));
    }

    #[test]
    fn corrupt_file_is_reported_as_corrupt_with_detail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        std::fs::write(&p, b"{ this is not json").unwrap();
        match PeerStore::load_from(&p) {
            LoadOutcome::Corrupt { detail } => assert!(!detail.is_empty(), "원인 문구가 비어 있으면 안 된다"),
            other => panic!("손상된 파일은 Corrupt 여야 한다: {other:?}"),
        }
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/deeper/ble-peers.json");
        PeerStore::save_to(&p, &[StoredPeer { token: "c".repeat(32), paired_at: 1 }]).unwrap();
        assert!(matches!(PeerStore::load_from(&p), LoadOutcome::Loaded(v) if v.len() == 1));
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        PeerStore::save_to(&p, &[StoredPeer { token: "d".repeat(32), paired_at: 1 }]).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "ble-peers.json")
            .collect();
        assert!(leftovers.is_empty(), "임시 파일이 남으면 안 된다: {leftovers:?}");
    }

    /// 저장이 tmp 생성 뒤 rename 전에 실패하면 tmp 가 남는다. 이름에 우리
    /// pid 가 박혀 있어 다음 저장도 같은 이름을 노리므로, 지우지 않으면
    /// `create_new` 가 계속 AlreadyExists 로 실패해 **재시작 전까지 저장이
    /// 통째로 막힌다.** tracing 이 no-op 이라 조용히 일어나므로, 새 기기를
    /// 페어링해도 저장되지 않고 재시작하면 사라진다.
    #[test]
    fn a_leftover_temp_file_does_not_block_future_saves() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");

        // 앞선 저장이 흘리고 간 tmp 를 흉내 낸다 — 이름은 구현과 같은 규칙.
        let stale = p.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&stale, b"half-written garbage").unwrap();

        let peer = StoredPeer { token: "e".repeat(32), paired_at: 7 };
        PeerStore::save_to(&p, std::slice::from_ref(&peer))
            .expect("남은 tmp 가 이후 저장을 막으면 안 된다");

        assert_eq!(PeerStore::load_from(&p), LoadOutcome::Loaded(vec![peer]));
        assert!(!stale.exists(), "저장 후에는 남은 tmp 도 사라져야 한다");
    }

    /// tmp+rename 을 `fs::write(path, ...)` 직접 쓰기로 "단순화"해도
    /// `save_leaves_no_temp_file` 은 여전히 통과한다 — 그건 tmp 잔존만 볼 뿐
    /// 원자성 자체를 보지 않기 때문이다. `rename` 은 대상을 새 inode 로
    /// 교체하고 직접 쓰기는 같은 inode 를 그대로 덮어쓰므로, inode 변화
    /// 여부로 그 차이를 고정한다.
    #[test]
    #[cfg(unix)]
    fn save_replaces_the_file_rather_than_writing_in_place() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        PeerStore::save_to(&p, &[StoredPeer { token: "e".repeat(32), paired_at: 1 }]).unwrap();
        let ino_before = std::fs::metadata(&p).unwrap().ino();

        PeerStore::save_to(&p, &[StoredPeer { token: "f".repeat(32), paired_at: 2 }]).unwrap();
        let ino_after = std::fs::metadata(&p).unwrap().ino();

        assert_ne!(ino_before, ino_after,
                   "저장이 같은 inode 를 덮어쓰면(직접 쓰기) 원자성이 깨진다 — rename 으로 교체해야 한다");
    }

    #[test]
    #[cfg(unix)]
    fn save_creates_file_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ble-peers.json");
        PeerStore::save_to(&p, &[StoredPeer { token: "a".repeat(32), paired_at: 1 }]).unwrap();

        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600,
                   "토큰은 영구 자격증명이다 — 파일이 만들어질 때부터 소유자만 읽을 수 있어야 한다: {mode:o}");
    }
}
