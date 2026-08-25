# E2EE 프로토콜 v2 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 맥과 클라이언트 사이의 모든 페이로드를 전송과 무관하게 종단 암호화하고, 페어링 순간 평문으로 건너던 128비트 토큰과 6자리 코드를 링크에서 없앤다.

**Architecture:** 전송 계층 **위**에 순수한 암호 계층(`src-tauri/src/crypto/`)을 하나 둔다. 연결마다 X25519 임시 키쌍을 만들어 공유 비밀을 얻고, HKDF-SHA256 으로 방향별 키를 파생해 ChaCha20-Poly1305 로 봉인한다. BLE·네트워크(iroh)·LAN 이 같은 코드를 쓴다. v1 동사는 전환 기간 동안 그대로 받는다.

**Tech Stack:** Rust (`x25519-dalek 3.0.0` + `chacha20poly1305 0.11.0` + `hkdf 0.13.0` + 기존 `hmac 0.13` / `sha2 0.11`), Swift (CryptoKit)

**Spec:** `docs/superpowers/specs/2026-08-25-e2ee-protocol-v2-design.md`

## Global Constraints

이 값들은 스펙에서 그대로 옮긴 것이며 **어떤 태스크에서도 바꾸지 않는다.**

- 페어링 코드 TTL **120초** (`CODE_TTL`), 논스 TTL **30초** (`NONCE_TTL`)
- 사용자가 연 창당 시도 **5회** (`MAX_ATTEMPTS`) — **절대 넓히지 않는다.** 무차별 대입 논증과 §5.1 의 중간자 방어가 모두 여기 걸려 있다
- 토큰은 **128비트 소문자 hex** (32자)
- 6자리 코드는 **어느 방향으로도 링크를 건너지 않는다**
- 토큰은 발급 순간부터 **봉인되어** 건넌다
- 인가되지 않은 상대는 스냅샷을 **0바이트** 받는다
- BLE·네트워크·LAN 공유는 **기본 꺼짐**
- `begin_pairing` 은 맥 쪽 **명시적 사용자 제스처**로만 호출된다
- AEAD: ChaCha20-Poly1305, 키 32바이트, 논스 12바이트, 태그 16바이트
- AEAD 논스 = `[0,0,0,0] || counter.to_be_bytes()` (u64 빅엔디안), AAD = `b"aim-v2"`
- 봉인 프레임 = `counter(8바이트 BE) || ciphertext || tag(16바이트)`
- `transcript` = `cpk_bytes || spk_bytes` — **항상 클라이언트 키가 먼저**
- HKDF info 문자열: `"aim-pair-v2"`, `"aim-sess-v2-s2c"`, `"aim-sess-v2-c2s"`
- 모든 바이너리는 **소문자 hex** 로 실린다. HMAC 키·메시지는 hex 문자열의 UTF-8 이 아니라 **디코드한 원시 바이트**다
- **(키, 논스) 쌍은 절대 재사용하지 않는다** — 이 계획에서 가장 지키기 중요한 불변식

### 확정된 크레이트 API (2026-08-25 실제 컴파일로 검증함)

```rust
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;                              // sha2 0.11 — hkdf 0.13 과 같은 digest 0.11 세대
use x25519_dalek::{EphemeralSecret, PublicKey};

let sec = EphemeralSecret::random();           // features = ["getrandom"] 필요(기본 아님)
let pk  = PublicKey::from(&sec);
let raw: [u8; 32] = *pk.as_bytes();
let peer = PublicKey::from(raw);               // [u8;32] 에서 복원
let ss = sec.diffie_hellman(&peer);            // sec 를 소비한다
ss.was_contributory() -> bool                  // 저차 점 검사
ss.as_bytes() -> &[u8; 32]

Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut okm)?;

let key = Key::try_from(&okm[..])?;            // Key::from_slice 는 deprecated
let aead = ChaCha20Poly1305::new(&key);
let nonce = Nonce::try_from(&nb[..])?;
aead.encrypt(&nonce, Payload { msg, aad })     // -> Vec<u8> (평문 + 16바이트 태그)
aead.decrypt(&nonce, Payload { msg, aad })
```

**주의:** `sha2` 는 반드시 `0.11` 이어야 한다. `hkdf 0.13` 은 `digest 0.11` 계열인데
`sha2 0.10` 은 `digest 0.10` 이라 트레잇 경계가 맞지 않는다(실제로 확인함). 이 저장소의
`Cargo.toml` 은 이미 `sha2 = "0.11.0"` 을 선언하고 있다.

### 기존 코드에서 재사용할 것

`src-tauri/src/ble/pairing.rs` 안의 것들이다. **새로 만들지 말고 쓴다.**

- `PairingManager::hex_decode(s: &str) -> Option<Vec<u8>>`
- `PairingManager::is_valid_lowercase_hex(s: &str, len: usize) -> bool`
- `PairingManager::random_hex128() -> String` — `/dev/urandom` 을 직접 읽는다
- `PairingManager::random_u64() -> u64`
- `type HmacSha256 = Hmac<Sha256>`
- 테스트 헬퍼: `t(secs) -> SystemTime`, `id(s) -> CentralId`, `hex_encode(&[u8]) -> String`

## 파일 구조

| 파일 | 책임 |
|---|---|
| `src-tauri/src/crypto/mod.rs` (신규) | 키 합의·KDF·MAC. 순수 함수만. 전송을 모른다 |
| `src-tauri/src/crypto/channel.rs` (신규) | `SealedChannel` — 방향별 키와 카운터, seal/open |
| `src-tauri/src/ble/pairing.rs` (수정) | v2 동사 파싱, v2 분기, 세션 채널 보관 |
| `src-tauri/src/ble/peripheral.rs` (수정) | `offer_frame` 을 central 별로 |
| `src-tauri/src/ble/mod.rs` (수정) | central 별 봉인 스냅샷 |
| `src-tauri/src/ble/macos.rs` (수정) | central 별 전송 |
| `src-tauri/src/network/mod.rs` (수정) | 봉인 스냅샷 |
| `docs/ble-protocol/golden/e2ee-v2-sample.json` (신규) | 크로스 언어 골든 벡터 |
| `ios/Sources/BLETransport/CryptoV2.swift` (신규) | CryptoKit 포팅 |
| `ios/Sources/BLETransport/PairingClient.swift` (수정) | v2 프레임 생성 |
| `ios/Tests/BLETransportTests/CryptoV2Tests.swift` (신규) | 골든 벡터 대조 |
| `docs/ble-protocol/DEVICE-TEST.md` (수정) | v2 실기기 절차 |

`crypto` 를 `ble` 밑이 아니라 최상위에 두는 이유: LAN 브리지도 쓸 것이고, 이름이
`ble::crypto` 면 전송 독립이라는 사실이 코드에서 거짓말이 된다.

---

## Task 1: crypto 모듈 — 키 합의와 KDF

**Files:**
- Create: `src-tauri/src/crypto/mod.rs`
- Modify: `src-tauri/Cargo.toml` (의존성 3개 추가)
- Modify: `src-tauri/src/lib.rs` (`mod crypto;` 선언)

**Interfaces:**
- Consumes: 없음 (첫 태스크)
- Produces:
  - `pub struct EphemeralKeyPair { secret: EphemeralSecret, pub public: [u8; 32] }`
  - `pub fn ephemeral_keypair() -> EphemeralKeyPair`
  - `pub fn transcript(client_pub: &[u8; 32], server_pub: &[u8; 32]) -> [u8; 64]`
  - `pub fn agree(kp: EphemeralKeyPair, peer_pub: &[u8; 32]) -> Option<[u8; 32]>` — 저차 점이면 `None`
  - `pub fn derive_pair_key(ss: &[u8; 32], nonce: &[u8]) -> [u8; 32]`
  - `pub fn derive_session_keys(ss: &[u8; 32], token: &[u8], nonce: &[u8]) -> ([u8; 32], [u8; 32])` — `(k_s2c, k_c2s)`

- [ ] **Step 1: 의존성을 추가한다**

```bash
cd src-tauri
cargo add x25519-dalek@3.0.0 --features getrandom
cargo add chacha20poly1305@0.11.0
cargo add hkdf@0.13.0
```

`sha2` 가 `0.11.0` 인지 확인한다. `0.10` 이면 `hkdf 0.13` 과 트레잇이 맞지 않는다.

```bash
grep -n '^sha2' Cargo.toml   # sha2 = "0.11.0" 이어야 한다
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/crypto/mod.rs` 파일 맨 아래:

```rust
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
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::`
Expected: FAIL — `cannot find function ephemeral_keypair`

- [ ] **Step 4: 구현한다**

`src-tauri/src/crypto/mod.rs` 맨 위:

```rust
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
```

`src-tauri/src/lib.rs` 의 다른 `mod` 선언 옆에 추가:

```rust
mod crypto;
```

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::`
Expected: PASS — 6개

- [ ] **Step 6: 뮤테이션으로 테스트가 실제로 잡는지 확인한다**

`agree` 의 `if !ss.was_contributory() { return None; }` 를 잠시 지우고
`cargo test crypto::rejects_low_order_point` 를 돌린다. **반드시 실패해야 한다.**
실패하지 않으면 그 테스트는 아무것도 지키지 않는 것이므로 테스트를 고친다.
확인 후 코드를 되돌린다.

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/crypto/mod.rs src-tauri/src/lib.rs
git commit -m "feat(crypto): X25519 키 합의와 HKDF 파생"
```

---

## Task 2: SealedChannel — 봉인·해제와 재전송 방어

**Files:**
- Create: `src-tauri/src/crypto/channel.rs`

**Interfaces:**
- Consumes: Task 1 의 `derive_session_keys`
- Produces:
  - `pub struct SealedChannel`
  - `pub fn new(send_key: [u8; 32], recv_key: [u8; 32]) -> SealedChannel`
  - `pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8>` — 봉인 프레임 전체
  - `pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, ChannelError>`
  - `pub enum ChannelError { TooShort, Replay, BadTag }`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/crypto/channel.rs` 맨 아래:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 맥과 클라이언트를 흉내낸다 — 한쪽의 송신 키가 다른 쪽의 수신 키다.
    fn pair() -> (SealedChannel, SealedChannel) {
        let s2c = [1u8; 32];
        let c2s = [2u8; 32];
        (SealedChannel::new(s2c, c2s), SealedChannel::new(c2s, s2c))
    }

    #[test]
    fn round_trips() {
        let (mut mac, mut client) = pair();
        let frame = mac.seal(b"hello");
        assert_eq!(client.open(&frame).unwrap(), b"hello");
    }

    #[test]
    fn counter_increments_so_two_identical_messages_differ() {
        let (mut mac, mut client) = pair();
        let a = mac.seal(b"same");
        let b = mac.seal(b"same");
        assert_ne!(a, b, "같은 평문이라도 카운터가 달라 암호문이 달라야 한다");
        assert_eq!(client.open(&a).unwrap(), b"same");
        assert_eq!(client.open(&b).unwrap(), b"same");
    }

    /// 이 검사가 이 파일에서 가장 중요하다 — 같은 (키, 논스) 로 두 번
    /// 봉인하면 ChaCha20-Poly1305 의 보장이 통째로 무너진다.
    #[test]
    fn rejects_replayed_frame() {
        let (mut mac, mut client) = pair();
        let frame = mac.seal(b"once");
        assert!(client.open(&frame).is_ok());
        assert_eq!(
            client.open(&frame),
            Err(ChannelError::Replay),
            "같은 카운터를 두 번 받으면 거부한다"
        );
    }

    #[test]
    fn rejects_out_of_order_frame() {
        let (mut mac, mut client) = pair();
        let first = mac.seal(b"1");
        let second = mac.seal(b"2");
        assert!(client.open(&second).is_ok());
        assert_eq!(
            client.open(&first),
            Err(ChannelError::Replay),
            "이미 지나간 카운터는 거부한다"
        );
    }

    /// 프레임이 유실돼도 그 다음 프레임은 열려야 한다 — BLE 청크 재조립은
    /// 순서가 어긋나면 프레임을 버리므로 실제로 일어난다.
    #[test]
    fn tolerates_a_gap_in_counters() {
        let (mut mac, mut client) = pair();
        let _lost = mac.seal(b"lost");
        let next = mac.seal(b"next");
        assert_eq!(client.open(&next).unwrap(), b"next", "빈 칸을 건너뛸 수 있어야 한다");
    }

    #[test]
    fn rejects_tampered_tag() {
        let (mut mac, mut client) = pair();
        let mut frame = mac.seal(b"hello");
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        assert_eq!(client.open(&frame), Err(ChannelError::BadTag));
    }

    #[test]
    fn rejects_frame_sealed_with_the_wrong_direction_key() {
        let (mut mac, _client) = pair();
        let frame = mac.seal(b"hello");
        // 맥이 자기 송신 키로 봉인한 것을 자기가 열려고 하면 안 된다.
        assert_eq!(mac.open(&frame), Err(ChannelError::BadTag));
    }

    #[test]
    fn rejects_short_frame() {
        let (_mac, mut client) = pair();
        assert_eq!(client.open(&[0u8; 8]), Err(ChannelError::TooShort));
    }
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::channel::`
Expected: FAIL — `cannot find type SealedChannel`

- [ ] **Step 3: 구현한다**

```rust
//! 방향별 키와 카운터를 갖는 봉인 채널.
//!
//! **(키, 논스) 쌍은 절대 재사용하지 않는다.** 세션마다 키가 다르므로 카운터를
//! 0 에서 시작해도 안전하고, 카운터는 u64 라 실질적으로 순환하지 않는다.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// AEAD 부가 인증 데이터. 프로토콜 버전을 태그에 묶는다.
pub const AAD: &[u8] = b"aim-v2";
/// 봉인 프레임 앞에 붙는 카운터 길이.
pub const COUNTER_LEN: usize = 8;
/// Poly1305 태그 길이.
pub const TAG_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// 카운터와 태그를 담기에도 짧다.
    TooShort,
    /// 이미 본 카운터 이하 — 재전송이거나 순서 역행이다.
    Replay,
    /// 복호·인증 실패. 변조됐거나 키가 다르다.
    BadTag,
}

pub struct SealedChannel {
    send: ChaCha20Poly1305,
    recv: ChaCha20Poly1305,
    send_counter: u64,
    /// 마지막으로 **받아들인** 카운터. 첫 프레임을 받기 전에는 None 이다 —
    /// 0 으로 두면 카운터 0 인 첫 프레임을 재전송으로 오인한다.
    last_recv: Option<u64>,
}

impl SealedChannel {
    pub fn new(send_key: [u8; 32], recv_key: [u8; 32]) -> Self {
        let mk = |k: [u8; 32]| {
            let key = Key::try_from(&k[..]).expect("32바이트는 ChaCha20 의 유효한 키 길이다");
            ChaCha20Poly1305::new(&key)
        };
        Self {
            send: mk(send_key),
            recv: mk(recv_key),
            send_counter: 0,
            last_recv: None,
        }
    }

    fn nonce_bytes(counter: u64) -> [u8; 12] {
        let mut nb = [0u8; 12];
        nb[4..].copy_from_slice(&counter.to_be_bytes());
        nb
    }

    /// 봉인 프레임 = counter(8바이트 BE) || ciphertext || tag(16바이트).
    ///
    /// 카운터를 프레임에 싣는 이유: 수신자가 자기 카운터만 세면 프레임 하나만
    /// 유실돼도 영구히 어긋난다. BLE 청크 재조립은 순서가 어긋나면 프레임을
    /// 버리므로 실제로 일어나는 일이다.
    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let counter = self.send_counter;
        self.send_counter = self
            .send_counter
            .checked_add(1)
            .expect("u64 카운터는 실질적으로 순환하지 않는다");
        let nb = Self::nonce_bytes(counter);
        let nonce = Nonce::try_from(&nb[..]).expect("12바이트는 유효한 논스 길이다");
        let ct = self
            .send
            .encrypt(&nonce, Payload { msg: plaintext, aad: AAD })
            .expect("ChaCha20-Poly1305 봉인은 실패하지 않는다");
        let mut out = Vec::with_capacity(COUNTER_LEN + ct.len());
        out.extend_from_slice(&counter.to_be_bytes());
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&mut self, frame: &[u8]) -> Result<Vec<u8>, ChannelError> {
        if frame.len() < COUNTER_LEN + TAG_LEN {
            return Err(ChannelError::TooShort);
        }
        let mut cb = [0u8; COUNTER_LEN];
        cb.copy_from_slice(&frame[..COUNTER_LEN]);
        let counter = u64::from_be_bytes(cb);
        if let Some(last) = self.last_recv {
            if counter <= last {
                return Err(ChannelError::Replay);
            }
        }
        let nb = Self::nonce_bytes(counter);
        let nonce = Nonce::try_from(&nb[..]).expect("12바이트는 유효한 논스 길이다");
        let pt = self
            .recv
            .decrypt(&nonce, Payload { msg: &frame[COUNTER_LEN..], aad: AAD })
            .map_err(|_| ChannelError::BadTag)?;
        // 인증에 성공한 뒤에만 카운터를 전진시킨다 — 그렇지 않으면 공격자가
        // 큰 카운터의 쓰레기 프레임 하나로 이후 정상 프레임을 전부 막을 수 있다.
        self.last_recv = Some(counter);
        Ok(pt)
    }
}
```

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::channel::`
Expected: PASS — 8개

- [ ] **Step 5: 뮤테이션으로 두 곳을 확인한다**

1. `open` 의 `if counter <= last { return Err(Replay) }` 를 지운다 →
   `rejects_replayed_frame` 과 `rejects_out_of_order_frame` 이 **반드시 실패해야 한다.**
2. `self.last_recv = Some(counter);` 를 `decrypt` **앞으로** 옮긴다 →
   `rejects_tampered_tag` 뒤에 정상 프레임이 막히는지 확인하는 테스트가 없으므로
   아래 테스트를 추가하고, 옮긴 상태에서 실패하는지 본다.

```rust
    /// 변조된 프레임이 이후 정상 프레임을 막아서는 안 된다. 카운터를 인증
    /// 전에 전진시키면, 공격자가 카운터 u64::MAX 짜리 쓰레기 하나로 세션을
    /// 영구히 죽일 수 있다.
    #[test]
    fn a_tampered_frame_does_not_block_later_valid_frames() {
        let (mut mac, mut client) = pair();
        let good = mac.seal(b"good");
        let mut junk = mac.seal(b"junk");
        junk[0..8].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(client.open(&junk), Err(ChannelError::BadTag));
        assert_eq!(client.open(&good).unwrap(), b"good");
    }
```

확인 후 코드를 되돌린다.

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/crypto/channel.rs
git commit -m "feat(crypto): 방향별 키와 카운터를 갖는 봉인 채널"
```

---

## Task 3: 코드 바인딩과 v2 proof

**Files:**
- Modify: `src-tauri/src/crypto/mod.rs`

**Interfaces:**
- Consumes: Task 1 의 `transcript`
- Produces:
  - `pub fn code_binding(code: &str, transcript: &[u8; 64]) -> [u8; 32]`
  - `pub fn verify_code_binding(code: &str, transcript: &[u8; 64], given_hex: &str) -> bool`
  - `pub fn session_proof(token_bytes: &[u8], nonce_bytes: &[u8], transcript: &[u8; 64]) -> [u8; 32]`
  - `pub fn verify_session_proof(token_bytes: &[u8], nonce_bytes: &[u8], transcript: &[u8; 64], given_hex: &str) -> bool`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/crypto/mod.rs` 의 `mod tests` 안에 추가:

```rust
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
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
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::tests`
Expected: FAIL — `cannot find function code_binding`

- [ ] **Step 3: 구현한다**

`src-tauri/src/crypto/mod.rs` 에 추가:

```rust
use hmac::{Hmac, Mac};

type HmacSha256 = Hmac<Sha256>;

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
```

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::tests`
Expected: PASS — 12개 (Task 1 의 6개 + 여기 6개)

- [ ] **Step 5: 뮤테이션 확인**

`session_proof` 에서 `mac.update(transcript);` 를 지운다 →
`session_proof_binds_the_transcript` 가 **반드시 실패해야 한다.** 확인 후 되돌린다.

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/crypto/mod.rs
git commit -m "feat(crypto): 코드 바인딩과 transcript 를 묶는 v2 proof"
```

---

## Task 4: 골든 벡터

**Files:**
- Modify: `src-tauri/src/crypto/mod.rs` (골든 테스트 추가)
- Create: `docs/ble-protocol/golden/e2ee-v2-sample.json`

**Interfaces:**
- Consumes: Task 1~3 전부
- Produces: `docs/ble-protocol/golden/e2ee-v2-sample.json` — Swift 와 C 가 읽는다

임시 개인키는 난수라 재현되지 않으므로, 골든 벡터는 **공유 비밀부터** 고정한다.
`agree()` 자체는 Task 1 의 `both_sides_agree_on_the_same_secret` 가 지킨다.

- [ ] **Step 1: 골든 테스트를 쓴다**

`src-tauri/src/crypto/mod.rs` 의 `mod tests` 안에:

```rust
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
            "sealed_frame_0": hex(&server.seal(b"{\"v\":2}")),
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
```

- [ ] **Step 2: 벡터가 없어 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml crypto::tests::golden`
Expected: FAIL — "골든 벡터가 없다"

- [ ] **Step 3: 벡터를 생성한다**

```bash
UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml crypto::tests::golden
```

- [ ] **Step 4: 통과와 결정성을 확인한다**

```bash
cargo test --manifest-path src-tauri/Cargo.toml crypto::tests::golden
cargo test --manifest-path src-tauri/Cargo.toml crypto::tests::golden
```
Expected: 두 번 다 PASS. 값이 매번 바뀌면 어딘가에 난수가 섞인 것이므로 멈추고 찾는다.

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/crypto/mod.rs docs/ble-protocol/golden/e2ee-v2-sample.json
git commit -m "test(crypto): E2EE v2 크로스 언어 골든 벡터"
```

---

## Task 5: v2 동사 파싱

**Files:**
- Modify: `src-tauri/src/ble/pairing.rs:53-62` (`AuthRequest`), `:114-135` (`parse_auth_request`)

**Interfaces:**
- Consumes: 없음
- Produces:
  - `AuthRequest::Hello2(String)` — 클라이언트 임시 공개키 hex
  - `AuthRequest::Code2(String)` — cbind hex
  - `AuthRequest::Auth2(String)` — 클라이언트 임시 공개키 hex
  - `AuthRequest::Proof2(String)` — proof hex

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/pairing.rs` 의 `mod tests` 안, `parses_each_request_form` 옆:

```rust
    #[test]
    fn parses_v2_request_forms() {
        assert!(matches!(
            parse_auth_request(b"HELLO2:aabb"),
            AuthRequest::Hello2(p) if p == "aabb"
        ));
        assert!(matches!(
            parse_auth_request(b"CODE2:ccdd"),
            AuthRequest::Code2(p) if p == "ccdd"
        ));
        assert!(matches!(
            parse_auth_request(b"AUTH2:eeff"),
            AuthRequest::Auth2(p) if p == "eeff"
        ));
        assert!(matches!(
            parse_auth_request(b"PROOF2:0011"),
            AuthRequest::Proof2(p) if p == "0011"
        ));
    }

    /// v1 동사가 그대로 남아야 한다 — 이미 페어링된 아이폰이 전환 기간에
    /// 끊기지 않는 근거다(스펙 8장).
    #[test]
    fn v1_verbs_still_parse_alongside_v2() {
        assert!(matches!(parse_auth_request(b"HELLO"), AuthRequest::Hello));
        assert!(matches!(parse_auth_request(b"AUTH"), AuthRequest::Auth));
        assert!(matches!(parse_auth_request(b"CODE:123456"), AuthRequest::Code(c) if c == "123456"));
    }

    /// `HELLO2` 는 접두사 뒤에 공개키가 반드시 와야 한다. 콜론 없는 `HELLO2`
    /// 가 `HELLO` 로 오인되면 v2 클라이언트가 조용히 v1 경로로 떨어진다.
    #[test]
    fn hello2_without_payload_is_malformed() {
        assert!(matches!(parse_auth_request(b"HELLO2"), AuthRequest::Malformed));
        assert!(matches!(parse_auth_request(b"AUTH2"), AuthRequest::Malformed));
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::tests::parses_v2`
Expected: FAIL — `no variant named Hello2`

- [ ] **Step 3: 구현한다**

`AuthRequest` 에 추가:

```rust
    /// v2 페어링 시작. 클라이언트 임시 공개키(64 hex).
    Hello2(String),
    /// v2 코드 제출. `HMAC(6자리코드, transcript)` 의 hex — **코드 자체가 아니다.**
    Code2(String),
    /// v2 재연결 시작. 클라이언트 임시 공개키(64 hex).
    Auth2(String),
    /// v2 재연결 증명. `HMAC(token, nonce || transcript)` 의 hex.
    Proof2(String),
```

`parse_auth_request` 에서 **v2 접두사를 v1 보다 먼저** 검사한다:

```rust
    // v2 를 먼저 본다. `HELLO` 를 먼저 검사하면 `HELLO2:...` 가 걸리지 않지만,
    // 순서를 명시해 두는 편이 나중에 동사가 늘 때 안전하다.
    if let Some(rest) = s.strip_prefix("HELLO2:") {
        return AuthRequest::Hello2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("CODE2:") {
        return AuthRequest::Code2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("AUTH2:") {
        return AuthRequest::Auth2(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("PROOF2:") {
        return AuthRequest::Proof2(rest.to_string());
    }
```

`handle` 의 `match` 에 임시 분기를 넣어 컴파일을 통과시킨다 (Task 6·7 에서 채운다):

```rust
    AuthRequest::Hello2(_)
    | AuthRequest::Code2(_)
    | AuthRequest::Auth2(_)
    | AuthRequest::Proof2(_) => AuthReply::Rejected,
```

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::`
Expected: PASS — 기존 테스트가 하나도 깨지지 않아야 한다

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/ble/pairing.rs
git commit -m "feat(pairing): v2 동사 파싱 — v1 과 나란히 받는다"
```

---

## Task 6: v2 페어링 경로

**Files:**
- Modify: `src-tauri/src/ble/pairing.rs` (`AuthReply`, `PairingManager`, `handle`)

**Interfaces:**
- Consumes: Task 1~3 의 `crypto::*`, Task 5 의 `Hello2`/`Code2`
- Produces:
  - `AuthReply::AwaitingCode2 { epk: String, nonce: String }`
  - `AuthReply::Granted2 { sealed: String }`
  - `PairingManager` 내부: `handshakes: HashMap<String, PendingHandshake>`
  - `PairingManager::channel_mut(&mut self, id: &CentralId) -> Option<&mut SealedChannel>`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
    use crate::crypto::{self, channel::SealedChannel};

    /// 테스트에서 "v2 클라이언트" 역할을 한다.
    struct V2Client {
        kp: Option<crypto::EphemeralKeyPair>,
        public: [u8; 32],
    }

    impl V2Client {
        fn new() -> Self {
            let kp = crypto::ephemeral_keypair();
            let public = kp.public;
            Self { kp: Some(kp), public }
        }
        /// 서버 응답으로부터 공유 비밀과 transcript 를 만든다.
        fn agree(&mut self, epk_hex: &str) -> ([u8; 32], [u8; 64]) {
            let spk = hex32(epk_hex);
            let kp = self.kp.take().expect("임시 키는 한 번만 쓴다");
            let ss = crypto::agree(kp, &spk).expect("정상 키끼리는 합의된다");
            (ss, crypto::transcript(&self.public, &spk))
        }
    }

    fn hex32(s: &str) -> [u8; 32] {
        let v = PairingManager::hex_decode(s).expect("유효한 hex");
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        out
    }

    fn epk_and_nonce(reply: &AuthReply) -> (String, String) {
        match reply {
            AuthReply::AwaitingCode2 { epk, nonce } => (epk.clone(), nonce.clone()),
            other => panic!("AwaitingCode2 를 기대했다: {other:?}"),
        }
    }

    #[test]
    fn v2_pairing_delivers_the_token_sealed() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();

        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, nonce) = epk_and_nonce(&reply);
        let (ss, tr) = c.agree(&epk);

        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));
        let AuthReply::Granted2 { sealed } = reply else {
            panic!("Granted2 를 기대했다: {reply:?}")
        };

        // 클라이언트가 토큰을 꺼낸다.
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let k_pair = crypto::derive_pair_key(&ss, &nonce_bytes);
        let mut ch = SealedChannel::new(k_pair, k_pair);
        let frame = PairingManager::hex_decode(&sealed).unwrap();
        let plain = ch.open(&frame).expect("k_pair 로 열려야 한다");
        let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        let token = json["token"].as_str().expect("토큰이 들어 있어야 한다");

        assert_eq!(token.len(), 32, "토큰은 128비트 소문자 hex 다");
        assert!(m.is_authorized(&id("A")));
        assert_eq!(m.issued_peers().len(), 1);
    }

    /// **이 스펙의 존재 이유다.** 토큰이 평문으로 나가면 안 된다.
    #[test]
    fn v2_never_puts_the_token_in_cleartext() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _nonce) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));

        let bytes = reply.to_json_bytes();
        let text = String::from_utf8(bytes).unwrap();
        let token = m.issued_peers()[0].0.clone();
        assert!(!text.contains(&token), "토큰이 평문으로 나갔다: {text}");
        assert!(!text.contains("\"token\""), "token 필드 자체가 없어야 한다: {text}");
    }

    /// 6자리 코드도 어느 방향으로도 나가지 않는다.
    #[test]
    fn v2_never_puts_the_code_on_the_wire() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let text = String::from_utf8(reply.to_json_bytes()).unwrap();
        assert!(!text.contains(&code), "코드가 응답에 실렸다: {text}");
    }

    #[test]
    fn v2_wrong_code_binding_spends_an_attempt() {
        let mut m = PairingManager::new();
        let _code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);

        let wrong = hex_encode(&crypto::code_binding("999999", &tr));
        let reply = m.handle(&id("A"), AuthRequest::Code2(wrong), t(1002));
        assert!(matches!(reply, AuthReply::Denied { left: 4 }), "시도를 소모한다: {reply:?}");
    }

    /// 형식 오류는 코드 추측이 아니므로 예산을 소모하지 않는다.
    #[test]
    fn v2_malformed_public_key_does_not_spend_an_attempt() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Hello2("짧다".into()), t(1001));
        assert!(matches!(reply, AuthReply::Rejected));
        match m.pairing_window(t(1002)) {
            PairingWindow::Open { attempts_left, .. } => {
                assert_eq!(attempts_left, MAX_ATTEMPTS, "형식 오류는 예산을 안 깎는다")
            }
            other => panic!("창이 열려 있어야 한다: {other:?}"),
        }
    }

    /// 저차 점을 보내면 공유 비밀이 상수가 된다. 거부해야 한다.
    #[test]
    fn v2_rejects_low_order_public_key() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&[0u8; 32])), t(1001));
        assert!(matches!(reply, AuthReply::Rejected), "저차 점 거부: {reply:?}");
    }

    /// HELLO2 없이 온 CODE2 는 transcript 가 없으므로 검증할 수 없다.
    #[test]
    fn v2_code_without_hello_is_rejected() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        let reply = m.handle(&id("A"), AuthRequest::Code2("ab".repeat(32)), t(1001));
        assert!(matches!(reply, AuthReply::Rejected));
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::tests::v2_`
Expected: FAIL — `no variant named AwaitingCode2`

- [ ] **Step 3: 구현한다**

`AuthReply` 에 추가:

```rust
    /// v2 의 `AwaitingCode`. 맥의 임시 공개키와 논스를 함께 싣는다.
    /// **코드는 여전히 담지 않는다** — 코드는 맥 화면으로만 간다.
    AwaitingCode2 { epk: String, nonce: String },
    /// v2 의 `Granted`. 토큰을 평문 필드가 아니라 봉인 프레임으로 싣는다.
    Granted2 { sealed: String },
```

`to_json_bytes` 에 추가:

```rust
            AuthReply::AwaitingCode2 { epk, nonce } => {
                format!(r#"{{"ok":false,"v":2,"await":"code","epk":"{epk}","nonce":"{nonce}"}}"#)
                    .into_bytes()
            }
            AuthReply::Granted2 { sealed } => {
                format!(r#"{{"ok":true,"v":2,"sealed":"{sealed}"}}"#).into_bytes()
            }
```

`PairingManager` 에 필드를 추가한다:

```rust
    /// v2 핸드셰이크 중간 상태. central 마다 하나이며, CODE2/PROOF2 를 받는
    /// 순간 소비된다. 임시 개인키는 `EphemeralKeyPair` 안에 있고 한 번만 쓰인다.
    handshakes: HashMap<String, PendingHandshake>,
    /// 인가된 세션의 봉인 채널. `authorized` 와 수명이 같다.
    channels: HashMap<String, SealedChannel>,
```

```rust
struct PendingHandshake {
    /// 이 핸드셰이크로 만든 공유 비밀.
    ss: [u8; 32],
    transcript: [u8; 64],
    /// 이 핸드셰이크에 쓰인 논스(hex). 세션 키 파생의 salt 다.
    nonce: String,
}
```

`handle` 의 v2 분기 (Hello2·Code2):

```rust
            AuthRequest::Hello2(cpk_hex) => {
                let Some(cpk) = Self::hex32(&cpk_hex) else {
                    // 형식 오류는 코드 추측이 아니므로 예산을 소모하지 않는다.
                    return AuthReply::Rejected;
                };
                if self.open_window(now).is_none() {
                    return AuthReply::Rejected;
                }
                let kp = crypto::ephemeral_keypair();
                let spk = kp.public;
                let Some(ss) = crypto::agree(kp, &cpk) else {
                    // 저차 점 — 공유 비밀이 상수가 된다.
                    return AuthReply::Rejected;
                };
                let nonce = Self::random_hex128();
                self.handshakes.insert(
                    id.0.clone(),
                    PendingHandshake {
                        ss,
                        transcript: crypto::transcript(&cpk, &spk),
                        nonce: nonce.clone(),
                    },
                );
                AuthReply::AwaitingCode2 { epk: hex_encode_bytes(&spk), nonce }
            }
            AuthRequest::Code2(given) => {
                // 핸드셰이크는 성공·실패와 무관하게 소비한다 — 같은 transcript
                // 로 여러 번 추측하지 못하게 한다. 재시도하려면 HELLO2 부터다.
                let Some(hs) = self.handshakes.remove(&id.0) else {
                    return AuthReply::Rejected;
                };
                let Some(p) = self.open_window(now) else {
                    return AuthReply::Rejected;
                };
                if !crypto::verify_code_binding(&p.code, &hs.transcript, &given) {
                    p.attempts_left -= 1;
                    return AuthReply::Denied { left: p.attempts_left };
                }
                let token = Self::random_hex128();
                let paired_at = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let token_bytes = Self::hex_decode(&token).expect("방금 만든 hex 다");
                let nonce_bytes = Self::hex_decode(&hs.nonce).expect("방금 만든 hex 다");

                // 토큰 한 건만 k_pair 로 봉인한다.
                let k_pair = crypto::derive_pair_key(&hs.ss, &nonce_bytes);
                let mut pair_ch = SealedChannel::new(k_pair, k_pair);
                let payload = format!(r#"{{"token":"{token}"}}"#);
                let sealed = hex_encode_bytes(&pair_ch.seal(payload.as_bytes()));

                // 그 즉시 세션 키로 전환한다(스펙 6.1) — 왕복이 더 필요 없다.
                let (s2c, c2s) = crypto::derive_session_keys(&hs.ss, &token_bytes, &nonce_bytes);
                self.channels.insert(id.0.clone(), SealedChannel::new(s2c, c2s));

                self.tokens.insert(token.clone(), paired_at);
                self.authorized.insert(id.0.clone(), token);
                self.pending = None;
                AuthReply::Granted2 { sealed }
            }
```

보조 함수 두 개를 `PairingManager` 옆에 둔다:

```rust
    fn hex32(s: &str) -> Option<[u8; 32]> {
        if !Self::is_valid_lowercase_hex(s, 64) {
            return None;
        }
        let v = Self::hex_decode(s)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        Some(out)
    }
```

```rust
/// 모듈 수준 hex 인코더. 기존 `hex_encode` 는 `mod tests` 안에만 있어
/// 프로덕션 코드에서 쓸 수 없다.
fn hex_encode_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
```

`end_session` / `revoke_peer` / `revoke_all` / `end_all_sessions` 에서
`self.channels.remove(...)` 와 `self.handshakes.remove(...)` 를 함께 한다 —
`authorized` 를 지우는 모든 자리에서 채널도 지워야 한다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::`
Expected: PASS — v2 7개 포함, 기존 테스트 전부 유지

- [ ] **Step 5: 뮤테이션 확인**

`Code2` 분기의 `p.attempts_left -= 1;` 을 지운다 →
`v2_wrong_code_binding_spends_an_attempt` 가 **반드시 실패해야 한다.** 되돌린다.

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/ble/pairing.rs
git commit -m "feat(pairing): v2 페어링 — 토큰을 봉인해서 전달한다"
```

---

## Task 7: v2 재연결 경로

**Files:**
- Modify: `src-tauri/src/ble/pairing.rs`

**Interfaces:**
- Consumes: Task 6 의 `PendingHandshake`, `channels`
- Produces:
  - `AuthReply::Nonce2 { epk: String, nonce: String }`
  - `AuthReply::Authorized2`
  - `PairingManager::channel_mut(&mut self, id: &CentralId) -> Option<&mut SealedChannel>`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
    /// 페어링해서 토큰과 세션 키를 얻은 뒤, 연결을 끊고 재연결한다.
    #[test]
    fn v2_reconnect_with_token_authorizes() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));

        // ── 1차: 페어링 ──
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, nonce) = epk_and_nonce(&reply);
        let (ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        let AuthReply::Granted2 { sealed } = m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002))
        else {
            panic!("Granted2 를 기대했다")
        };
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let k_pair = crypto::derive_pair_key(&ss, &nonce_bytes);
        let mut pair_ch = SealedChannel::new(k_pair, k_pair);
        let plain = pair_ch
            .open(&PairingManager::hex_decode(&sealed).unwrap())
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        let token = json["token"].as_str().unwrap().to_string();

        m.end_session(&id("A"));
        assert!(!m.is_authorized(&id("A")));

        // ── 2차: 재연결 ──
        let mut c2 = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c2.public)), t(2000));
        let AuthReply::Nonce2 { epk, nonce } = reply else {
            panic!("Nonce2 를 기대했다: {reply:?}")
        };
        let (_ss2, tr2) = c2.agree(&epk);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce2_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce2_bytes, &tr2));

        let reply = m.handle(&id("A"), AuthRequest::Proof2(proof), t(2001));
        assert_eq!(reply, AuthReply::Authorized2);
        assert!(m.is_authorized(&id("A")));
    }

    /// v1 과 같다 — proof 실패는 코드 추측이 아니므로 예산을 소모하지 않는다.
    #[test]
    fn v2_bad_proof_does_not_spend_an_attempt() {
        let mut m = PairingManager::new();
        m.begin_pairing(t(1000));
        m.load_peers(vec![("aa".repeat(16), 900)]);
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        assert!(matches!(reply, AuthReply::Nonce2 { .. }));
        let reply = m.handle(&id("A"), AuthRequest::Proof2("00".repeat(32)), t(1002));
        assert!(matches!(reply, AuthReply::Rejected));
        match m.pairing_window(t(1003)) {
            PairingWindow::Open { attempts_left, .. } => assert_eq!(attempts_left, MAX_ATTEMPTS),
            other => panic!("창이 열려 있어야 한다: {other:?}"),
        }
    }

    /// **transcript 바인딩이 실제로 동작하는지** 확인한다. 중간자가 임시 키를
    /// 바꿔치기한 상황을 흉내낸다 — 올바른 토큰으로 만든 proof 라도 transcript
    /// 가 다르면 통과하면 안 된다.
    #[test]
    fn v2_proof_from_a_different_transcript_is_rejected() {
        let mut m = PairingManager::new();
        let token = "aa".repeat(16);
        m.load_peers(vec![(token.clone(), 900)]);
        let c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Auth2(hex_encode(&c.public)), t(1001));
        let AuthReply::Nonce2 { nonce, .. } = reply else { panic!() };

        // 서버가 준 epk 가 아니라 엉뚱한 키로 transcript 를 만든다.
        let bogus_tr = crypto::transcript(&c.public, &[0x77u8; 32]);
        let token_bytes = PairingManager::hex_decode(&token).unwrap();
        let nonce_bytes = PairingManager::hex_decode(&nonce).unwrap();
        let proof = hex_encode(&crypto::session_proof(&token_bytes, &nonce_bytes, &bogus_tr));

        assert!(matches!(
            m.handle(&id("A"), AuthRequest::Proof2(proof), t(1002)),
            AuthReply::Rejected
        ));
    }

    /// AUTH2 없이 온 PROOF2 는 핸드셰이크가 없으므로 거부한다.
    #[test]
    fn v2_proof_without_auth_is_rejected() {
        let mut m = PairingManager::new();
        m.load_peers(vec![("aa".repeat(16), 900)]);
        assert!(matches!(
            m.handle(&id("A"), AuthRequest::Proof2("00".repeat(32)), t(1000)),
            AuthReply::Rejected
        ));
    }

    /// 인가가 사라지면 봉인 채널도 사라져야 한다 — 남으면 해제된 기기에
    /// 계속 봉인해 보낼 수 있다.
    #[test]
    fn v2_ending_a_session_drops_its_channel() {
        let mut m = PairingManager::new();
        let code = m.begin_pairing(t(1000));
        let mut c = V2Client::new();
        let reply = m.handle(&id("A"), AuthRequest::Hello2(hex_encode(&c.public)), t(1001));
        let (epk, _) = epk_and_nonce(&reply);
        let (_ss, tr) = c.agree(&epk);
        let cbind = hex_encode(&crypto::code_binding(&code, &tr));
        m.handle(&id("A"), AuthRequest::Code2(cbind), t(1002));
        assert!(m.channel_mut(&id("A")).is_some());

        m.end_session(&id("A"));
        assert!(m.channel_mut(&id("A")).is_none(), "세션이 끝나면 채널도 없어야 한다");
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::tests::v2_reconnect`
Expected: FAIL — `no variant named Nonce2`

- [ ] **Step 3: 구현한다**

`AuthReply` 에 추가:

```rust
    /// v2 의 `Nonce`. 맥의 임시 공개키를 함께 싣는다.
    Nonce2 { epk: String, nonce: String },
    /// v2 의 `Authorized`. 되돌려보낼 비밀이 없으므로 필드도 없다.
    Authorized2,
```

`to_json_bytes` 에 추가:

```rust
            AuthReply::Nonce2 { epk, nonce } => {
                format!(r#"{{"ok":false,"v":2,"epk":"{epk}","nonce":"{nonce}"}}"#).into_bytes()
            }
            AuthReply::Authorized2 => br#"{"ok":true,"v":2}"#.to_vec(),
```

`handle` 의 v2 분기 (Auth2·Proof2):

```rust
            AuthRequest::Auth2(cpk_hex) => {
                let Some(cpk) = Self::hex32(&cpk_hex) else {
                    return AuthReply::Rejected;
                };
                let kp = crypto::ephemeral_keypair();
                let spk = kp.public;
                let Some(ss) = crypto::agree(kp, &cpk) else {
                    return AuthReply::Rejected;
                };
                let nonce = Self::random_hex128();
                // v1 의 논스와 같은 수명 정책을 따른다 — sweep_expired_nonces 가
                // NONCE_TTL 을 지난 것을 지운다. 핸드셰이크도 같이 지운다.
                self.nonces.insert(
                    id.0.clone(),
                    PendingNonce { nonce: nonce.clone(), issued_at: now },
                );
                self.handshakes.insert(
                    id.0.clone(),
                    PendingHandshake {
                        ss,
                        transcript: crypto::transcript(&cpk, &spk),
                        nonce: nonce.clone(),
                    },
                );
                AuthReply::Nonce2 { epk: hex_encode_bytes(&spk), nonce }
            }
            AuthRequest::Proof2(given) => {
                // v1 과 같이 성공·실패와 무관하게 즉시 폐기한다(1회용).
                let Some(pending) = self.nonces.remove(&id.0) else {
                    return AuthReply::Rejected;
                };
                let Some(hs) = self.handshakes.remove(&id.0) else {
                    return AuthReply::Rejected;
                };
                let Ok(nonce_bytes) = Self::hex_decode(&pending.nonce).ok_or(()) else {
                    return AuthReply::Rejected;
                };
                let matched = self
                    .tokens
                    .keys()
                    .find(|token| {
                        Self::hex_decode(token).is_some_and(|tb| {
                            crypto::verify_session_proof(&tb, &nonce_bytes, &hs.transcript, &given)
                        })
                    })
                    .cloned();
                let Some(token) = matched else {
                    return AuthReply::Rejected;
                };
                let token_bytes = Self::hex_decode(&token).expect("저장된 토큰은 유효한 hex 다");
                let (s2c, c2s) = crypto::derive_session_keys(&hs.ss, &token_bytes, &nonce_bytes);
                self.channels.insert(id.0.clone(), SealedChannel::new(s2c, c2s));
                self.authorized.insert(id.0.clone(), token);
                AuthReply::Authorized2
            }
```

접근자:

```rust
    /// 인가된 세션의 봉인 채널. 브리지가 스냅샷을 봉인할 때 쓴다.
    /// 채널이 없으면 v1 세션이거나 인가되지 않은 세션이다.
    pub fn channel_mut(&mut self, id: &CentralId) -> Option<&mut SealedChannel> {
        self.channels.get_mut(&id.0)
    }
```

`sweep_expired_nonces` 가 논스를 지울 때 **같은 키의 핸드셰이크도 지운다.**

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml pairing::`
Expected: PASS — 전체

- [ ] **Step 5: 뮤테이션 확인**

`Proof2` 분기에서 `crypto::verify_session_proof` 의 `&hs.transcript` 를
`&[0u8; 64]` 로 바꾼다 → `v2_reconnect_with_token_authorizes` 가 실패해야 한다.
`v2_proof_from_a_different_transcript_is_rejected` 는 통과할 수도 있으니,
**두 테스트를 함께 본다.** 되돌린다.

- [ ] **Step 6: 커밋**

```bash
git add src-tauri/src/ble/pairing.rs
git commit -m "feat(pairing): v2 재연결 — transcript 를 토큰에 묶는다"
```

---

## Task 8: `offer_frame` 을 central 별로 바꾼다 (동작 불변 리팩터링)

봉인은 세션 키로 하므로 central 마다 **다른 바이트**가 나간다. 지금 API 는 한 벌의
청크를 여러 central 에 보내는 모양이라 그대로는 쓸 수 없다.

이 태스크는 **동작을 바꾸지 않는다.** 기존 테스트가 전부 그대로 통과해야 한다.

**부수 효과 하나가 중요하다.** central 별로 프레이밍하면 청크 크기도 각자의
`max_notify_len` 으로 정할 수 있다. 지금은 인가된 구독자 전체의 최솟값을 쓰기 때문에
(`ble/mod.rs:86`), MTU 가 낮은 기기 하나가 아이폰의 청크까지 잘게 만들고, 스냅샷이
17×255=4,335바이트를 넘으면 `TooLarge` 로 **모두의 프레임이 사라진다**(`mod.rs:110`).
이 리팩터링이 그 결함을 함께 없앤다.

**Files:**
- Modify: `src-tauri/src/ble/peripheral.rs:53-86` (트레잇), `:127-150` (`FakePeripheral`)
- Modify: `src-tauri/src/ble/mod.rs:80-113` (`on_snapshot`)
- Modify: `src-tauri/src/ble/macos.rs` (`offer_frame` 구현)

**Interfaces:**
- Consumes: 없음
- Produces: `fn offer_frame_to(&self, ch: CharId, central: &CentralId, chunks: Vec<Vec<u8>>)`
  — 기존 `offer_frame(ch, chunks, authorized)` 를 대체한다

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/mod.rs` 의 `mod tests` 에:

```rust
    /// MTU 가 작은 기기가 붙어도 큰 기기의 청크가 작아지면 안 된다.
    /// 예전에는 인가된 구독자 전체의 최솟값을 썼다.
    #[test]
    fn each_central_gets_chunks_sized_for_its_own_mtu() {
        let p = std::sync::Arc::new(FakePeripheral::new());
        p.set_subscribers(vec![
            Subscriber { id: CentralId("BIG".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("SMALL".into()), max_notify_len: 23 },
        ]);
        // (여기서 두 central 을 모두 인가한 뒤 on_snapshot 을 호출한다.
        //  구체적인 준비는 이 파일의 기존 테스트가 쓰는 헬퍼를 그대로 따른다.)
        let frames = p.taken_frames_by_central();
        let big = frames.iter().find(|(c, _, _)| c.0 == "BIG").expect("BIG 에게 갔다");
        let small = frames.iter().find(|(c, _, _)| c.0 == "SMALL").expect("SMALL 에게 갔다");
        assert!(
            big.2[0].len() > small.2[0].len(),
            "MTU 가 큰 기기는 더 큰 청크를 받아야 한다"
        );
    }
```

`FakePeripheral` 에 기록용 접근자를 더한다:

```rust
    /// central 별로 기록된 프레임을 꺼내고 비운다.
    pub fn taken_frames_by_central(&self) -> Vec<(CentralId, CharId, Vec<Vec<u8>>)> {
        std::mem::take(&mut *self.per_central.lock().unwrap())
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::mod::tests::each_central`
Expected: FAIL — `no method named taken_frames_by_central`

- [ ] **Step 3: 트레잇을 바꾼다**

`peripheral.rs` 의 `offer_frame` 을 지우고:

```rust
    /// 한 central 에게 프레임을 넘긴다. 실제 전송과 백프레셔는 구현체가
    /// 책임진다(fire-and-forget).
    ///
    /// **central 마다 따로 부른다.** E2EE v2 에서는 세션 키가 central 마다
    /// 다르므로 바이트도 달라진다. 덤으로 청크 크기를 그 central 의 MTU 에
    /// 맞출 수 있어, MTU 가 작은 기기 하나가 모두의 청크를 잘게 만들던 문제가
    /// 사라진다.
    fn offer_frame_to(&self, ch: CharId, central: &CentralId, chunks: Vec<Vec<u8>>);
```

- [ ] **Step 4: `on_snapshot` 을 central 별 루프로 바꾼다**

`ble/mod.rs` 의 `on_snapshot` 에서 `min()` 을 없애고:

```rust
        let authorized = self
            .peripheral
            .authorized_subscribers(&|id| pairing.is_authorized(id));
        if authorized.is_empty() {
            return;
        }
        if !self.gate.should_emit(snap, now) {
            return;
        }
        let dto = MirrorSnapshot::from(snap);
        let json = match serde_json::to_vec(&dto) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("스냅샷 직렬화 실패: {e}");
                self.gate.reset();
                return;
            }
        };
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);

        for sub in &authorized {
            // 청크 크기는 이 central 의 MTU 로 정한다.
            match framing::chunk(frame_id, &json, sub.max_notify_len) {
                Ok(chunks) => self.peripheral.offer_frame_to(CharId::Snapshot, &sub.id, chunks),
                Err(e) => {
                    // 한 기기가 못 받는다고 다른 기기까지 막지 않는다.
                    tracing::error!("청킹 실패({}): {e:?}", sub.id.0);
                }
            }
        }
```

- [ ] **Step 5: `macos.rs` 와 `FakePeripheral` 을 새 시그니처에 맞춘다**

`macos.rs` 의 기존 `offer_frame` 은 `authorized` 목록으로 대상을 좁혔다. 이제
호출자가 대상을 하나로 정해서 주므로, 구현은 그 central 에게만 보낸다.
`revoke_targets` 는 그대로 둔다 — 인가가 철회된 central 을 전송 큐에서 즉시
빼는 역할은 여전히 필요하다.

- [ ] **Step 6: 전체 테스트를 돌린다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — 기존 테스트가 하나도 깨지지 않아야 한다. 깨지면 동작이 바뀐 것이므로 멈추고 원인을 찾는다.

- [ ] **Step 7: 커밋**

```bash
git add src-tauri/src/ble/
git commit -m "refactor(ble): 프레임을 central 별로 보낸다

E2EE v2 는 세션 키가 central 마다 다르므로 바이트도 달라진다. 덤으로 청크
크기를 각 central 의 MTU 로 정할 수 있게 되어, MTU 가 작은 기기 하나가
모두의 청크를 잘게 만들고 TooLarge 시 모두의 프레임을 없애던 결함이 사라진다."
```

---

## Task 9: 봉인된 스냅샷 배선

**Files:**
- Modify: `src-tauri/src/ble/mod.rs` (`on_snapshot`)
- Modify: `src-tauri/src/network/mod.rs` (`on_snapshot`)

**Interfaces:**
- Consumes: Task 7 의 `PairingManager::channel_mut`, Task 8 의 `offer_frame_to`
- Produces: 없음 (배선)

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```rust
    /// v2 세션에는 봉인된 바이트가 가야 한다. 평문 JSON 이 그대로 나가면
    /// 이 스펙 전체가 무의미하다.
    #[test]
    fn v2_session_receives_sealed_bytes_not_plaintext_json() {
        // (v2 페어링을 마친 central 하나를 준비한 뒤 on_snapshot 을 호출한다)
        let frames = p.taken_frames_by_central();
        let payload: Vec<u8> = frames[0].2.iter().flat_map(|c| c[3..].to_vec()).collect();
        assert!(!payload.starts_with(b"{"), "평문 JSON 이 나갔다");
        assert!(payload.len() > 8 + 16, "카운터 8 + 태그 16 보다 길어야 한다");
    }

    /// v1 세션은 그대로 평문이어야 한다 — 전환 기간 동안 기존 아이폰이 계속
    /// 동작해야 한다(스펙 8장).
    #[test]
    fn v1_session_still_receives_plaintext_json() {
        // (v1 로 인가된 central 하나를 준비한 뒤 on_snapshot 을 호출한다)
        let frames = p.taken_frames_by_central();
        let payload: Vec<u8> = frames[0].2.iter().flat_map(|c| c[3..].to_vec()).collect();
        assert!(payload.starts_with(b"{"), "v1 은 평문이어야 한다");
    }

    /// 인가되지 않은 구독자는 여전히 0바이트다.
    #[test]
    fn unauthorized_subscriber_still_gets_nothing() {
        // (구독만 하고 인가되지 않은 central 을 준비한 뒤 on_snapshot 을 호출한다)
        assert!(p.taken_frames_by_central().is_empty());
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::mod::tests::v2_session`
Expected: FAIL — 평문이 나간다

- [ ] **Step 3: 구현한다**

Task 8 의 루프에서 청킹 **직전에** 봉인한다. `pairing` 을 `&mut` 로 받아야 하므로
`on_snapshot` 의 시그니처를 `pairing: &mut PairingManager` 로 바꾼다 — 카운터가
전진해야 하기 때문이다.

```rust
        for sub in &authorized {
            // v2 세션이면 봉인하고, v1 세션이면 평문 그대로 보낸다.
            // 전환 기간 동안 두 세대가 공존한다(스펙 8장).
            let payload = match pairing.channel_mut(&sub.id) {
                Some(ch) => ch.seal(&json),
                None => json.clone(),
            };
            match framing::chunk(frame_id, &payload, sub.max_notify_len) {
                Ok(chunks) => self.peripheral.offer_frame_to(CharId::Snapshot, &sub.id, chunks),
                Err(e) => tracing::error!("청킹 실패({}): {e:?}", sub.id.0),
            }
        }
```

`network/mod.rs` 의 `on_snapshot` 에도 같은 분기를 넣는다. 네트워크는 청킹이 없으므로
`payload` 를 그대로 스트림에 쓴다.

`lib.rs` 에서 `on_snapshot` 호출부의 `&pairing` 을 `&mut pairing` 으로 고친다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — 전체

- [ ] **Step 5: 커밋**

```bash
git add src-tauri/src/
git commit -m "feat(e2ee): v2 세션에는 봉인된 스냅샷을 보낸다"
```

---

## Task 10: iOS — CryptoKit 포팅과 골든 벡터 대조

**Files:**
- Create: `ios/Sources/BLETransport/CryptoV2.swift`
- Create: `ios/Tests/BLETransportTests/CryptoV2Tests.swift`
- Modify: `ios/Project.swift` (골든 벡터를 테스트 리소스로 넣는다)

**Interfaces:**
- Consumes: Task 4 의 `docs/ble-protocol/golden/e2ee-v2-sample.json`
- Produces:
  - `enum CryptoV2` — `transcript`, `deriveSessionKeys`, `derivePairKey`, `codeBinding`, `sessionProof`
  - `final class SealedChannel` — `seal(_:)`, `open(_:)`

- [ ] **Step 1: 골든 벡터를 읽는 실패 테스트를 쓴다**

```swift
import CryptoKit
import XCTest
@testable import BLETransport

final class CryptoV2Tests: XCTestCase {
    /// Rust 가 만든 골든 벡터. 세 언어가 같은 파일을 읽는다.
    private func golden() throws -> [String: Any] {
        let url = Bundle.module.url(forResource: "e2ee-v2-sample", withExtension: "json")
        let data = try Data(contentsOf: try XCTUnwrap(url, "골든 벡터를 번들에 넣어야 한다"))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func testTranscriptMatchesGolden() throws {
        let g = try golden()
        let input = try XCTUnwrap(g["input"] as? [String: Any])
        let cpk = try XCTUnwrap(Data(hexString: input["client_pub"] as! String))
        let spk = try XCTUnwrap(Data(hexString: input["server_pub"] as! String))
        XCTAssertEqual(
            CryptoV2.transcript(clientPub: cpk, serverPub: spk).hexString,
            g["transcript"] as! String,
            "클라이언트 키가 먼저다"
        )
    }

    func testCodeBindingMatchesGolden() throws {
        let g = try golden()
        let input = try XCTUnwrap(g["input"] as? [String: Any])
        let tr = try XCTUnwrap(Data(hexString: g["transcript"] as! String))
        XCTAssertEqual(
            CryptoV2.codeBinding(code: input["code"] as! String, transcript: tr).hexString,
            g["code_binding"] as! String,
            "코드는 HMAC 의 키로 쓰인다 — UTF-8 바이트 그대로다"
        )
    }

    func testSessionProofMatchesGolden() throws {
        let g = try golden()
        let input = try XCTUnwrap(g["input"] as? [String: Any])
        let token = try XCTUnwrap(Data(hexString: input["token"] as! String))
        let nonce = try XCTUnwrap(Data(hexString: input["nonce"] as! String))
        let tr = try XCTUnwrap(Data(hexString: g["transcript"] as! String))
        XCTAssertEqual(
            CryptoV2.sessionProof(token: token, nonce: nonce, transcript: tr).hexString,
            g["session_proof"] as! String,
            "토큰은 hex 문자열이 아니라 디코드한 원시 바이트를 키로 쓴다"
        )
    }

    func testSessionKeysMatchGolden() throws {
        let g = try golden()
        let input = try XCTUnwrap(g["input"] as? [String: Any])
        let ss = try XCTUnwrap(Data(hexString: input["shared_secret"] as! String))
        let token = try XCTUnwrap(Data(hexString: input["token"] as! String))
        let nonce = try XCTUnwrap(Data(hexString: input["nonce"] as! String))
        let keys = CryptoV2.deriveSessionKeys(sharedSecret: ss, token: token, nonce: nonce)
        XCTAssertEqual(keys.s2c.hexString, g["k_s2c"] as! String)
        XCTAssertEqual(keys.c2s.hexString, g["k_c2s"] as! String)
    }

    /// Rust 가 봉인한 프레임을 Swift 가 열 수 있어야 한다. 이 하나가
    /// 실패하면 두 구현이 다른 프로토콜을 말하는 것이다.
    func testOpensGoldenSealedFrame() throws {
        let g = try golden()
        let input = try XCTUnwrap(g["input"] as? [String: Any])
        let ss = try XCTUnwrap(Data(hexString: input["shared_secret"] as! String))
        let token = try XCTUnwrap(Data(hexString: input["token"] as! String))
        let nonce = try XCTUnwrap(Data(hexString: input["nonce"] as! String))
        let keys = CryptoV2.deriveSessionKeys(sharedSecret: ss, token: token, nonce: nonce)
        // 클라이언트 입장: 서버의 s2c 가 내 수신 키다.
        let ch = SealedChannel(sendKey: keys.c2s, recvKey: keys.s2c)
        let frame = try XCTUnwrap(Data(hexString: g["sealed_frame_0"] as! String))
        XCTAssertEqual(try ch.open(frame), Data(#"{"v":2}"#.utf8))
    }
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5'`
Expected: FAIL — `cannot find 'CryptoV2' in scope`

- [ ] **Step 3: 구현한다**

```swift
import CryptoKit
import Foundation

/// E2EE v2 (스펙 2026-08-25-e2ee-protocol-v2-design.md).
/// Rust `src-tauri/src/crypto/` 와 골든 벡터로 묶여 있다.
enum CryptoV2 {
    static let infoPair = Data("aim-pair-v2".utf8)
    static let infoS2C = Data("aim-sess-v2-s2c".utf8)
    static let infoC2S = Data("aim-sess-v2-c2s".utf8)
    static let aad = Data("aim-v2".utf8)

    /// **항상 클라이언트 키가 먼저다.**
    static func transcript(clientPub: Data, serverPub: Data) -> Data {
        clientPub + serverPub
    }

    private static func hkdf32(ikm: Data, salt: Data, info: Data) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm),
            salt: salt,
            info: info,
            outputByteCount: 32
        )
    }

    static func derivePairKey(sharedSecret: Data, nonce: Data) -> SymmetricKey {
        hkdf32(ikm: sharedSecret, salt: nonce, info: infoPair)
    }

    static func deriveSessionKeys(
        sharedSecret: Data, token: Data, nonce: Data
    ) -> (s2c: SymmetricKey, c2s: SymmetricKey) {
        let ikm = sharedSecret + token
        return (hkdf32(ikm: ikm, salt: nonce, info: infoS2C),
                hkdf32(ikm: ikm, salt: nonce, info: infoC2S))
    }

    /// 6자리 코드를 **키로** 쓴다. 코드 자체는 링크를 건너지 않는다.
    static func codeBinding(code: String, transcript: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(
            for: transcript, using: SymmetricKey(data: Data(code.utf8))
        ))
    }

    /// 토큰은 **hex 를 디코드한 원시 바이트**를 키로 쓴다.
    static func sessionProof(token: Data, nonce: Data, transcript: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(
            for: nonce + transcript, using: SymmetricKey(data: token)
        ))
    }
}

enum SealedChannelError: Error, Equatable {
    case tooShort, replay, badTag
}

/// 방향별 키와 카운터. **(키, 논스) 쌍은 절대 재사용하지 않는다.**
final class SealedChannel {
    private let sendKey: SymmetricKey
    private let recvKey: SymmetricKey
    private var sendCounter: UInt64 = 0
    /// 마지막으로 받아들인 카운터. 첫 프레임 전에는 nil 이다 — 0 으로 두면
    /// 카운터 0 인 첫 프레임을 재전송으로 오인한다.
    private var lastRecv: UInt64?

    init(sendKey: SymmetricKey, recvKey: SymmetricKey) {
        self.sendKey = sendKey
        self.recvKey = recvKey
    }

    private static func nonce(_ counter: UInt64) throws -> ChaChaPoly.Nonce {
        var raw = Data(repeating: 0, count: 4)
        raw.append(contentsOf: withUnsafeBytes(of: counter.bigEndian) { Array($0) })
        return try ChaChaPoly.Nonce(data: raw)
    }

    func seal(_ plaintext: Data) throws -> Data {
        let counter = sendCounter
        sendCounter += 1
        let box = try ChaChaPoly.seal(
            plaintext, using: sendKey,
            nonce: Self.nonce(counter), authenticating: CryptoV2.aad
        )
        var out = Data(withUnsafeBytes(of: counter.bigEndian) { Array($0) })
        out.append(box.ciphertext)
        out.append(box.tag)
        return out
    }

    func open(_ frame: Data) throws -> Data {
        guard frame.count >= 8 + 16 else { throw SealedChannelError.tooShort }
        let counter = frame.prefix(8).reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
        if let last = lastRecv, counter <= last { throw SealedChannelError.replay }
        let body = frame.dropFirst(8)
        let ct = body.prefix(body.count - 16)
        let tag = body.suffix(16)
        let box = try ChaChaPoly.SealedBox(
            nonce: Self.nonce(counter), ciphertext: ct, tag: tag
        )
        guard let pt = try? ChaChaPoly.open(box, using: recvKey, authenticating: CryptoV2.aad)
        else { throw SealedChannelError.badTag }
        // 인증에 성공한 뒤에만 전진시킨다 — 그렇지 않으면 카운터가 큰 쓰레기
        // 프레임 하나로 이후 정상 프레임이 전부 막힌다.
        lastRecv = counter
        return pt
    }
}
```

`ios/Project.swift` 의 `BLETransportTests` 타깃에 골든 벡터를 리소스로 추가한다:

```swift
resources: ["../../docs/ble-protocol/golden/e2ee-v2-sample.json"]
```

- [ ] **Step 4: 통과를 확인한다**

Run: `cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5'`
Expected: PASS

- [ ] **Step 5: 커밋**

```bash
git add ios/Sources/BLETransport/CryptoV2.swift ios/Tests/BLETransportTests/CryptoV2Tests.swift ios/Project.swift
git commit -m "feat(ios): E2EE v2 CryptoKit 포팅 — 골든 벡터로 Rust 와 대조"
```

---

## Task 11: iOS — v2 상태 기계

**Files:**
- Modify: `ios/Sources/BLETransport/PairingClient.swift`
- Modify: `ios/Sources/BLETransport/BLEClient.swift` (`decide`)
- Modify: `ios/Sources/NetworkTransport/NetworkClient.swift`
- Modify: `ios/Tests/BLETransportTests/`

**Interfaces:**
- Consumes: Task 10 의 `CryptoV2`, `SealedChannel`
- Produces:
  - `PairingClient.hello2Frame(clientPub:)`, `code2Frame(binding:)`, `auth2Frame(clientPub:)`, `proof2Frame(proof:)`
  - `BLEClient.decide` 가 v2 응답을 처리한다

- [ ] **Step 1: 실패하는 테스트를 쓴다**

```swift
    /// v2 는 다운그레이드하지 않는다. 거부당해도 v1 으로 물러서지 않는다 —
    /// 물러서면 공격자가 v2 를 방해해 평문으로 끌어내릴 수 있다(스펙 8장).
    func testV2NeverFallsBackToV1() {
        let reply = Data(#"{"ok":false}"#.utf8)
        switch BLEClient.decide(reply) {
        case .resetAndAwaitCode, .failed, .awaitCode:
            break  // 어느 쪽이든 v1 프레임을 만들지만 않으면 된다
        case .signNonce, .storeTokenAndSubscribe, .subscribe:
            XCTFail("거부에 대해 v1 경로로 진행하면 안 된다")
        }
    }

    /// 방금 입력한 코드가 저장된 토큰보다 우선한다. 맥이 토큰을 이미 폐기한
    /// 경우 토큰 재인증은 반드시 거부되고, 그 사이 코드는 쓰이지도 못한다.
    /// (NetworkClient 에서 실제로 겪은 버그다.)
    func testAFreshCodeWinsOverAStoredTokenInV2() {
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: true, code: "123456", clientPub: Data(repeating: 1, count: 32)),
            PairingClient.hello2Frame(clientPub: Data(repeating: 1, count: 32)),
            "코드가 있으면 HELLO2 로 시작한다"
        )
    }

    func testStoredTokenWithoutCodeUsesAuth2() {
        XCTAssertEqual(
            NetworkClient.initialFrameV2(hasToken: true, code: nil, clientPub: Data(repeating: 1, count: 32)),
            PairingClient.auth2Frame(clientPub: Data(repeating: 1, count: 32)),
            "코드가 없고 토큰이 있으면 AUTH2"
        )
    }
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5'`
Expected: FAIL — `no member 'hello2Frame'`

- [ ] **Step 3: 구현한다**

```swift
extension PairingClient {
    static func hello2Frame(clientPub: Data) -> Data {
        Data("HELLO2:\(clientPub.hexString)".utf8)
    }
    static func code2Frame(binding: Data) -> Data {
        Data("CODE2:\(binding.hexString)".utf8)
    }
    static func auth2Frame(clientPub: Data) -> Data {
        Data("AUTH2:\(clientPub.hexString)".utf8)
    }
    static func proof2Frame(proof: Data) -> Data {
        Data("PROOF2:\(proof.hexString)".utf8)
    }
}
```

`BLEClient.decide` 에 v2 응답 케이스를 더한다. 응답에 `"v":2` 가 있으면 v2 경로다.
`epk` 와 `nonce` 를 꺼내 `CryptoV2` 로 키를 만들고, `sealed` 가 있으면 열어 토큰을 꺼낸다.

`NetworkClient.initialFrameV2(hasToken:code:clientPub:)` 는 기존
`initialFrame(hasToken:code:)` 와 같은 규칙이다 — **코드가 있으면 코드 우선.**

**재시도 정책은 그대로 유지한다.** `needsPairing`/`authFailed` 는 전용 catch 로 받아
재시도 없이 멈춘다 — 코드 없이는 통과할 수 없는 재시도가 화면을 깜빡이게 한다.

- [ ] **Step 4: 통과를 확인한다**

Run: 5개 스킴 전부

```bash
cd ios
for S in BLETransportTests NetworkTransportTests WireTests MirrorFormatTests DesignSystemTests; do
  xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme $S \
    -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 \
    | grep -oE "Executed [0-9]+ tests?, with [0-9]+ failures?" | tail -1
done
```
Expected: 전부 0 failures

- [ ] **Step 5: 커밋**

```bash
git add ios/
git commit -m "feat(ios): v2 상태 기계 — 다운그레이드하지 않는다"
```

---

## Task 12: 실기기 검증 절차 문서화

**Files:**
- Modify: `docs/ble-protocol/DEVICE-TEST.md`

**Interfaces:**
- Consumes: Task 1~11 전부
- Produces: 없음 (문서)

- [ ] **Step 1: v2 절을 추가한다**

`DEVICE-TEST.md` 끝에:

```markdown
## 7. E2EE v2 확인 (2026-08-25 스펙)

### 7-1. 배포 순서

**맥을 먼저 올린다.** 맥은 v1·v2 를 모두 받지만 클라이언트는 v2 만 말한다.
순서를 뒤집으면 새 아이폰이 옛 맥에 붙지 못한다.

### 7-2. 확인 항목

- [ ] 기존에 페어링된 아이폰(v1)이 맥 업데이트 후에도 그대로 붙는다
- [ ] 새 아이폰 빌드(v2)로 새로 페어링이 된다
- [ ] 재부팅 후 v2 로 자동 재인증된다
- [ ] 맥에서 그 기기를 해제하면 즉시 끊기고, 다시 붙으려 하면 코드 화면이 뜬다
- [ ] 코드를 5번 틀리면 창이 닫히고, 그 뒤 시도는 전부 거부된다
- [ ] 창을 새로 열면 5회가 회복된다

### 7-3. 평문이 나가지 않는지 실제로 본다

`packetlogger`(Xcode Additional Tools)로 BLE 트래픽을 캡처해 페어링을 한 번 한다.

- [ ] Auth 특성 트래픽에 6자리 코드가 **나타나지 않는다**
- [ ] Auth 특성 트래픽에 32자리 토큰이 **나타나지 않는다**
- [ ] Snapshot 특성 페이로드가 `{` 로 시작하지 **않는다** (평문 JSON 이 아니다)

이 세 줄이 이 스펙의 완료 판정이다.
```

- [ ] **Step 2: 커밋**

```bash
git add docs/ble-protocol/DEVICE-TEST.md
git commit -m "docs: E2EE v2 실기기 검증 절차"
```

---

## 자체 리뷰

**1. 스펙 커버리지**

| 스펙 절 | 태스크 |
|---|---|
| §3 암호 요소 | Task 1 Step 1 |
| §4 인코딩 규약 (transcript 순서, 원시 바이트) | Task 1, 3 |
| §5 페어링 핸드셰이크 | Task 6 |
| §5.1 코드가 링크를 건너지 않음 | Task 3, 6 (`v2_never_puts_the_code_on_the_wire`) |
| §5.2 실패 처리 (예산 소모 규칙, 저차 점) | Task 6 |
| §6 재연결 핸드셰이크 | Task 7 |
| §6.1 페어링 직후 키 전환 | Task 6 (`Code2` 분기 끝) |
| §7 데이터 단계 (카운터, 재전송, 방향 분리) | Task 2, 9 |
| §8 버전 협상 | Task 5 (v1 병존), Task 11 (다운그레이드 금지), Task 12 (배포 순서) |
| §9 코드 배치 | 파일 구조 표 |
| §10 테스트 (골든 벡터, 뮤테이션) | Task 4, 각 태스크의 뮤테이션 단계 |
| §11 주지 않는 것 | 구현 대상 아님 |
| §12 유지되는 제약 | Global Constraints |

**빠진 것 하나를 찾았고 채웠다** — §7 이 요구하는 "BLE 청크 유실 시 카운터 빈 칸을
견딘다"가 Task 2 의 `tolerates_a_gap_in_counters` 로 들어갔다.

**2. 시그니처 일관성 확인**

- `agree(kp, peer)` 가 `kp` 를 소비한다는 점이 Task 1·6·7 에서 일관된다
  (Task 6 의 `V2Client::agree` 가 `self.kp.take()` 를 쓰는 이유)
- `derive_session_keys` 의 반환 순서 `(s2c, c2s)` 가 Task 1·6·7·10 에서 같다
- `SealedChannel::new(send_key, recv_key)` 의 인자 순서가 Rust·Swift 에서 같고,
  클라이언트는 `(c2s, s2c)`, 서버는 `(s2c, c2s)` 로 뒤집어 넣는다 (Task 2·9·10)
- `AuthReply` v2 변형 이름이 Task 6·7 에서 일관된다

**3. 알려진 미해결**

- Task 8·9 의 테스트 코드에 `// (…준비는 기존 헬퍼를 따른다)` 주석이 있다. 이 파일의
  기존 테스트가 쓰는 준비 코드를 그대로 복사해야 하며, 구현자가 그 파일을 읽고
  채운다. 여기에 지금 없는 헬퍼 이름을 지어내는 것보다 낫다고 판단했다.
- `network/mod.rs` 의 `AuthOutcome` 이 v2 응답을 어떻게 나르는지는 Task 9 에서
  기존 구조를 읽고 맞춘다 — 현재 필드가 `{ payload, now_authorized, granted }` 라
  `granted` 의 의미가 v2 에서 달라질 수 있다.
