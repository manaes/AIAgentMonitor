//! 전송 독립 종단 암호화 (스펙 2026-08-25-e2ee-protocol-v2-design.md).
//!
//! 이 모듈은 **전송을 모른다.** BLE·네트워크(iroh)·LAN 이 같은 함수를 쓴다.
//! 난수와 시계에 의존하는 것은 `ephemeral_keypair` 하나뿐이고 나머지는 순수하다 —
//! 그래야 골든 벡터로 세 언어를 묶을 수 있다.

pub mod channel;

use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey};

type HmacSha256 = Hmac<Sha256>;

/// HKDF info 문자열. 세 언어가 바이트 단위로 같아야 한다.
pub const INFO_PAIR: &[u8] = b"aim-pair-v2";
pub const INFO_S2C: &[u8] = b"aim-sess-v2-s2c";
pub const INFO_C2S: &[u8] = b"aim-sess-v2-c2s";

pub struct EphemeralKeyPair {
    secret: EphemeralSecret,
    pub public: [u8; 32],
}

pub fn ephemeral_keypair() -> EphemeralKeyPair {
    let secret = EphemeralSecret::random();
    let public = *PublicKey::from(&secret).as_bytes();
    EphemeralKeyPair { secret, public }
}

/// 두 임시 공개키를 이어붙인 64바이트. **항상 클라이언트 키가 먼저다** —
/// 역할과 무관하게 양쪽이 같은 순서로 만들어야 cbind 와 proof 가 일치한다.
pub fn transcript(client_pub: &[u8; 32], server_pub: &[u8; 32]) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(client_pub);
    out[32..].copy_from_slice(server_pub);
    out
}

/// 공유 비밀. 상대가 저차 점을 보내면 `None` 이다 — 그 경우 공유 비밀이
/// 상수가 되어 키 합의가 아무 의미도 없어진다.
///
/// `kp` 를 소비한다. 임시 키는 연결마다 새로 만들고 한 번만 쓴다.
pub fn agree(kp: EphemeralKeyPair, peer_pub: &[u8; 32]) -> Option<[u8; 32]> {
    let ss = kp.secret.diffie_hellman(&PublicKey::from(*peer_pub));
    if !ss.was_contributory() {
        return None;
    }
    Some(*ss.as_bytes())
}

fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut okm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut okm)
        .expect("32바이트는 SHA-256 HKDF 의 유효한 출력 길이다");
    okm
}

/// 페어링 단계에서 토큰 전달 한 건만 봉인하는 키.
/// 이 시점에는 토큰이 없으므로 ikm 이 공유 비밀뿐이다.
pub fn derive_pair_key(ss: &[u8; 32], nonce: &[u8]) -> [u8; 32] {
    hkdf32(ss, nonce, INFO_PAIR)
}

/// 세션 키 두 개. `ikm = ss || token` 이라 **둘 다 있어야** 키가 나온다 —
/// X25519 가 깨져도 토큰이 필요하고, 토큰이 새도 임시 개인키가 필요하다.
pub fn derive_session_keys(ss: &[u8; 32], token: &[u8], nonce: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut ikm = Vec::with_capacity(ss.len() + token.len());
    ikm.extend_from_slice(ss);
    ikm.extend_from_slice(token);
    (hkdf32(&ikm, nonce, INFO_S2C), hkdf32(&ikm, nonce, INFO_C2S))
}

/// 6자리 코드를 **키로** 써서 두 임시 공개키를 MAC 한다.
///
/// 코드 자체는 어느 방향으로도 링크를 건너지 않는다(v1 은 `CODE:123456` 으로
/// 그대로 보냈다). 동시에 이 값이 두 임시 공개키를 묶으므로 능동적 중간자가
/// 자기 키를 끼워넣으면 값이 맞지 않는다.
///
/// 코드의 엔트로피는 20비트뿐이지만, 창당 5회라는 시도 예산이 온라인 추측을
/// 5번으로 묶는다. **그 예산은 절대 넓히지 않는다.**
pub fn code_binding(code: &str, transcript: &[u8; 64]) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(code.as_bytes()).expect("HMAC 은 임의 길이 키를 받는다");
    mac.update(transcript);
    mac.finalize().into_bytes().into()
}

/// 재연결 증명. v1 의 `HMAC(token, nonce)` 에 transcript 를 붙인 것이다.
pub fn session_proof(token_bytes: &[u8], nonce_bytes: &[u8], transcript: &[u8; 64]) -> [u8; 32] {
    let mut mac =
        HmacSha256::new_from_slice(token_bytes).expect("HMAC 은 임의 길이 키를 받는다");
    mac.update(nonce_bytes);
    mac.update(transcript);
    mac.finalize().into_bytes().into()
}

/// 소문자 hex 64자만 받는다. 형식이 다르면 디코드를 시도하지 않는다 —
/// 모르는 것을 관대하게 받아주지 않는 기존 방침을 따른다.
fn hex_decode_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// 상수 시간 비교. `hmac::Mac::verify_slice` 를 쓰는 기존 `verify_proof` 와
/// 같은 이유다 — 바이트별 조기 반환은 타이밍으로 값을 흘린다.
pub fn verify_code_binding(code: &str, transcript: &[u8; 64], given_hex: &str) -> bool {
    let Some(given) = hex_decode_32(given_hex) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(code.as_bytes()) else {
        return false;
    };
    mac.update(transcript);
    mac.verify_slice(&given).is_ok()
}

pub fn verify_session_proof(
    token_bytes: &[u8],
    nonce_bytes: &[u8],
    transcript: &[u8; 64],
    given_hex: &str,
) -> bool {
    let Some(given) = hex_decode_32(given_hex) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(token_bytes) else {
        return false;
    };
    mac.update(nonce_bytes);
    mac.update(transcript);
    mac.verify_slice(&given).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn both_sides_agree_on_the_same_secret() {
        let a = ephemeral_keypair();
        let b = ephemeral_keypair();
        let (a_pub, b_pub) = (a.public, b.public);
        let ss_a = agree(a, &b_pub).expect("정상 키끼리는 합의된다");
        let ss_b = agree(b, &a_pub).expect("정상 키끼리는 합의된다");
        assert_eq!(ss_a, ss_b, "양쪽이 같은 공유 비밀을 얻어야 한다");
    }

    /// 상대가 저차 점을 보내면 공유 비밀이 상수가 되어 키 합의가 무의미해진다.
    /// 전부 0 인 32바이트가 대표적인 저차 점이다.
    #[test]
    fn rejects_low_order_point() {
        let a = ephemeral_keypair();
        assert_eq!(agree(a, &[0u8; 32]), None, "저차 점은 거부해야 한다");
    }

    /// transcript 순서가 역할과 무관해야 한다 — 양쪽이 다른 순서로 만들면
    /// cbind 와 proof 가 영원히 어긋난다.
    #[test]
    fn transcript_is_client_key_first() {
        let c = [1u8; 32];
        let s = [2u8; 32];
        let t = transcript(&c, &s);
        assert_eq!(&t[..32], &c, "앞 32바이트는 클라이언트 공개키");
        assert_eq!(&t[32..], &s, "뒤 32바이트는 서버 공개키");
    }

    #[test]
    fn session_keys_differ_by_direction() {
        let ss = [7u8; 32];
        let (s2c, c2s) = derive_session_keys(&ss, b"tokenbytes000000", b"nonce");
        assert_ne!(s2c, c2s, "방향이 다르면 키도 달라야 한다");
    }

    /// 토큰이 ikm 에 들어가는지 확인한다. 안 들어가면 X25519 만 깨도 세션이 열린다.
    #[test]
    fn session_keys_depend_on_the_token() {
        let ss = [7u8; 32];
        let (a, _) = derive_session_keys(&ss, b"tokenbytes000000", b"nonce");
        let (b, _) = derive_session_keys(&ss, b"tokenbytes000001", b"nonce");
        assert_ne!(a, b, "토큰이 다르면 세션 키도 달라야 한다");
    }

    #[test]
    fn pair_key_depends_on_the_nonce() {
        let ss = [7u8; 32];
        assert_ne!(
            derive_pair_key(&ss, b"nonce-a"),
            derive_pair_key(&ss, b"nonce-b"),
            "논스가 salt 로 실제로 쓰여야 한다"
        );
    }

    /// 중간자가 자기 임시 키를 끼워넣으면 transcript 가 달라져 cbind 가 맞지
    /// 않는다. 중간자는 6자리 코드를 모르므로 올바른 값을 만들 수 없다 —
    /// 이것이 페어링의 중간자 방어 전부다.
    #[test]
    fn code_binding_changes_when_the_transcript_changes() {
        let t1 = transcript(&[1u8; 32], &[2u8; 32]);
        let t2 = transcript(&[1u8; 32], &[9u8; 32]);
        assert_ne!(code_binding("123456", &t1), code_binding("123456", &t2));
    }

    #[test]
    fn code_binding_changes_when_the_code_changes() {
        let t = transcript(&[1u8; 32], &[2u8; 32]);
        assert_ne!(code_binding("123456", &t), code_binding("123457", &t));
    }

    #[test]
    fn verifies_a_correct_code_binding() {
        let t = transcript(&[1u8; 32], &[2u8; 32]);
        let given = hex(&code_binding("123456", &t));
        assert!(verify_code_binding("123456", &t, &given));
        assert!(!verify_code_binding("999999", &t, &given), "다른 코드는 통과 못 한다");
    }

    /// 길이나 대소문자가 다르면 디코드를 시도하지 않고 거부한다 — 토큰·proof 에
    /// 이미 적용된 기준과 같다.
    #[test]
    fn rejects_malformed_binding_hex() {
        let t = transcript(&[1u8; 32], &[2u8; 32]);
        assert!(!verify_code_binding("123456", &t, "짧다"));
        assert!(!verify_code_binding("123456", &t, &"AB".repeat(32)), "대문자 hex 거부");
    }

    /// v1 proof 는 HMAC(token, nonce) 였다. v2 는 transcript 를 붙여 키 합의를
    /// 토큰에 묶는다 — 중간자가 임시 키를 바꿔치기하면 proof 가 맞지 않는다.
    #[test]
    fn session_proof_binds_the_transcript() {
        let token = [3u8; 16];
        let nonce = [4u8; 16];
        let t1 = transcript(&[1u8; 32], &[2u8; 32]);
        let t2 = transcript(&[1u8; 32], &[9u8; 32]);
        assert_ne!(
            session_proof(&token, &nonce, &t1),
            session_proof(&token, &nonce, &t2),
            "transcript 가 proof 에 실제로 들어가야 한다"
        );
    }

    #[test]
    fn verifies_a_correct_session_proof() {
        let token = [3u8; 16];
        let nonce = [4u8; 16];
        let t = transcript(&[1u8; 32], &[2u8; 32]);
        let given = hex(&session_proof(&token, &nonce, &t));
        assert!(verify_session_proof(&token, &nonce, &t, &given));
        assert!(!verify_session_proof(&[9u8; 16], &nonce, &t, &given), "다른 토큰은 통과 못 한다");
    }

    /// Swift·C 와 공유하는 골든 벡터.
    /// 갱신: UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml crypto::tests::golden
    #[test]
    fn golden_e2ee_v2_matches() {
        use crate::crypto::channel::SealedChannel;
        use std::path::PathBuf;

        // 고정 입력 — 세 언어가 이 값들로 시작한다.
        let ss = [0x11u8; 32];
        let cpk = [0x22u8; 32];
        let spk = [0x33u8; 32];
        let nonce_bytes = [0x44u8; 16];
        let token_bytes = [0x55u8; 16];
        let code = "123456";

        let tr = transcript(&cpk, &spk);
        let (s2c, c2s) = derive_session_keys(&ss, &token_bytes, &nonce_bytes);
        let mut server = SealedChannel::new(s2c, c2s);
        // 같은 채널에서 연속으로 두 장. 매크로 안에서 부르면 평가 순서에 기대게
        // 되므로 카운터 순서를 여기서 못박는다.
        let frame0 = server.seal(b"{\"v\":2}");
        let frame1 = server.seal(b"{\"v\":2}");

        let actual = serde_json::json!({
            "note": "모든 hex 는 소문자다. HMAC 의 키와 메시지는 hex 문자열의 \
                     UTF-8 바이트가 아니라 디코드한 원시 바이트다.",
            "input": {
                "shared_secret": hex(&ss),
                "client_pub": hex(&cpk),
                "server_pub": hex(&spk),
                "nonce": hex(&nonce_bytes),
                "token": hex(&token_bytes),
                "code": code,
            },
            "transcript": hex(&tr),
            "code_binding": hex(&code_binding(code, &tr)),
            "session_proof": hex(&session_proof(&token_bytes, &nonce_bytes, &tr)),
            "pair_key": hex(&derive_pair_key(&ss, &nonce_bytes)),
            "k_s2c": hex(&s2c),
            "k_c2s": hex(&c2s),
            // 서버가 카운터 0 으로 봉인한 첫 프레임.
            "sealed_frame_0": hex(&frame0),
            // 같은 평문을 카운터 1 로 한 번 더. 카운터 0 짜리 한 장만 있으면
            // 논스 조립을 고정하지 못한다 — 0 에서는 `[0,0,0,0] || BE(counter)`
            // 와 `BE(counter) || [0,0,0,0)` 이 둘 다 12바이트 0 이고, 빅엔디언과
            // 리틀엔디언도 구별되지 않는다. 카운터 1 에서 셋 다 갈라진다.
            "sealed_frame_1": hex(&frame1),
        });

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/ble-protocol/golden/e2ee-v2-sample.json");
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
            return;
        }
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .expect("골든 벡터가 없다. UPDATE_GOLDEN=1 로 생성하고 커밋하라"),
        )
        .unwrap();
        assert_eq!(actual, expected, "E2EE v2 골든 벡터가 어긋났다");
    }
}
