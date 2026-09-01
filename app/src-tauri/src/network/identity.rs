//! 네트워크(iroh) 신원 영속화. `ble::peers` 와 같은 tmp+rename+0600 규약을
//! 따른다 — 이 파일이 곧 이 Mac의 `EndpointId`(공개키) 근거이므로, 재시작마다
//! 바뀌면 이미 페어링된 폰이 재스캔 없이 재연결할 수 없다.
use iroh::SecretKey;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub fn path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ai-agent-monitor/network-identity.key")
}

/// 저장된 키가 있으면 그대로 읽고, 없거나 손상됐으면 새로 만들어 저장한다.
/// BLE 페어링 토큰과 달리 이 값은 "복구 실패 시 재페어링을 요구하는" 수준의
/// 자격증명이 아니라 순전히 신원(공개키) 안정성을 위한 것이라, 손상됐을 때
/// 조용히 새로 발급해도 안전하다 — 다만 그 경우 기존에 저장해둔 폰들은
/// 재스캔이 필요해진다.
pub fn load_or_create(path: &Path) -> anyhow::Result<SecretKey> {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(SecretKey::from_bytes(&arr));
        }
    }
    let secret = SecretKey::generate();
    save_to(path, &secret)?;
    Ok(secret)
}

fn save_to(path: &Path, secret: &SecretKey) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::fs::Permissions;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, Permissions::from_mode(0o700))?;
            }
        }
    }
    let tmp = path.with_extension(format!("key.{}.tmp", std::process::id()));
    if tmp.exists() {
        std::fs::remove_file(&tmp)?;
    }
    let bytes = secret.to_bytes();

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&bytes)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, bytes)?;
    }

    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uses_network_specific_filename() {
        assert!(path().ends_with("ai-agent-monitor/network-identity.key"));
    }

    #[test]
    fn creates_and_persists_a_stable_key() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("network-identity.key");

        let first = load_or_create(&p).unwrap();
        let second = load_or_create(&p).unwrap();

        assert_eq!(first.to_bytes(), second.to_bytes(), "재시작해도 같은 EndpointId여야 한다");
    }

    #[test]
    fn corrupt_file_falls_back_to_a_fresh_key_without_erroring() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("network-identity.key");
        std::fs::write(&p, b"too short").unwrap();

        let secret = load_or_create(&p).unwrap();
        assert_eq!(secret.to_bytes().len(), 32);
    }

    #[test]
    #[cfg(unix)]
    fn creates_file_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("network-identity.key");
        load_or_create(&p).unwrap();

        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
