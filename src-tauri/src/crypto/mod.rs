//! 전송 독립 종단 암호화 (스펙 2026-08-25-e2ee-protocol-v2-design.md).
//!
//! 이 모듈은 **전송을 모른다.** BLE·네트워크(iroh)·LAN 이 같은 함수를 쓴다.
//! 난수와 시계에 의존하는 것은 `ephemeral_keypair` 하나뿐이고 나머지는 순수하다 —
//! 그래야 골든 벡터로 세 언어를 묶을 수 있다.

pub mod channel;

use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
