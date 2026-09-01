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
}
