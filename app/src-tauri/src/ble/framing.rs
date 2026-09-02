//! BLE notify 청킹. 패킷 = [frame_id][chunk_idx][chunk_count][payload…] (스펙 4.2)
//! 재조립 규칙은 Swift `FrameReassembler` 와 반드시 동일해야 하며,
//! 골든 벡터(docs/ble-protocol/golden/frames-sample.json)로 양쪽을 묶어둔다.

pub const HEADER_LEN: usize = 3;
const MAX_CHUNKS: usize = 255;

#[derive(Debug, PartialEq, Eq)]
pub enum FramingError {
    /// max_chunk 가 헤더보다 작거나 같아 본문을 담을 수 없다
    ChunkTooSmall,
    /// 255 청크를 넘는 메시지
    TooLarge,
}

pub fn chunk(
    frame_id: u8,
    payload: &[u8],
    max_chunk: usize,
) -> Result<Vec<Vec<u8>>, FramingError> {
    if max_chunk <= HEADER_LEN {
        return Err(FramingError::ChunkTooSmall);
    }
    let body = max_chunk - HEADER_LEN;
    let count = if payload.is_empty() {
        1
    } else {
        payload.len().div_ceil(body)
    };
    if count > MAX_CHUNKS {
        return Err(FramingError::TooLarge);
    }

    let mut out = Vec::with_capacity(count);
    for idx in 0..count {
        let start = idx * body;
        let end = usize::min(start + body, payload.len());
        let mut packet = Vec::with_capacity(HEADER_LEN + (end - start));
        packet.push(frame_id);
        packet.push(idx as u8);
        packet.push(count as u8);
        packet.extend_from_slice(&payload[start..end]);
        out.push(packet);
    }
    Ok(out)
}

/// 수신 측 재조립기. Rust 에서는 테스트와 골든 벡터 생성에만 쓰이지만,
/// Swift 포팅의 기준 구현 역할을 하므로 여기에 둔다.
#[derive(Debug, Default)]
pub struct Reassembler {
    frame_id: Option<u8>,
    expected_idx: u8,
    count: u8,
    buf: Vec<u8>,
    /// 이 프레임을 이미 버렸는지(중간 구독·순서 이탈)
    aborted: bool,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 완성된 메시지를 만들면 `Some(payload)`.
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < HEADER_LEN {
            return None;
        }
        let (id, idx, count) = (packet[0], packet[1], packet[2]);
        if count == 0 {
            return None;
        }

        // 규칙 1·2: 새 frame_id 이거나 0번 청크면 새로 시작한다.
        if self.frame_id != Some(id) || idx == 0 {
            if idx != 0 {
                // 규칙 3: 0번을 못 본 프레임은 통째로 버린다.
                self.frame_id = Some(id);
                self.aborted = true;
                return None;
            }
            self.frame_id = Some(id);
            self.expected_idx = 0;
            self.count = count;
            self.buf.clear();
            self.aborted = false;
        }

        if self.aborted || idx != self.expected_idx || count != self.count {
            self.aborted = true;
            return None;
        }

        self.buf.extend_from_slice(&packet[HEADER_LEN..]);
        self.expected_idx = self.expected_idx.saturating_add(1);

        if u16::from(self.expected_idx) == u16::from(self.count) {
            let done = std::mem::take(&mut self.buf);
            self.frame_id = None;
            return Some(done);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_payload_yields_one_chunk() {
        let f = chunk(0, b"", 20).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0], vec![0, 0, 1], "헤더만 있고 본문 없음");
    }

    #[test]
    fn payload_fitting_exactly_yields_one_chunk() {
        // max_chunk 20 → 본문 17바이트까지 한 청크
        let f = chunk(3, &[0xAB; 17], 20).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(&f[0][..3], &[3, 0, 1]);
        assert_eq!(f[0].len(), 20);
    }

    #[test]
    fn one_byte_over_splits_into_two() {
        let f = chunk(3, &[0xAB; 18], 20).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(&f[0][..3], &[3, 0, 2]);
        assert_eq!(&f[1][..3], &[3, 1, 2]);
        assert_eq!(f[1].len(), 4, "마지막 청크는 남은 1바이트만");
    }

    #[test]
    fn rejects_max_chunk_too_small() {
        assert!(matches!(chunk(0, b"x", 3), Err(FramingError::ChunkTooSmall)));
    }

    #[test]
    fn rejects_payload_needing_more_than_255_chunks() {
        let payload = vec![0u8; 256 * 17 + 1];
        assert!(matches!(chunk(0, &payload, 20), Err(FramingError::TooLarge)));
    }

    /// 정확한 경계값 고정. `count > MAX_CHUNKS` 가 `>=` 로 미끄러지거나 캐스팅이 어긋나면
    /// `count as u8` 이 0 이 되어(256 → 0) 수신 측이 모든 패킷을 버리고 미러가 조용히 영구 정지한다.
    #[test]
    fn accepts_exactly_255_chunks() {
        // max_chunk 20 → 본문 17바이트. 255*17 이 정확히 255 청크.
        let payload = vec![0u8; 255 * 17];
        let f = chunk(0, &payload, 20).expect("255 청크는 허용된다");
        assert_eq!(f.len(), 255);
        assert_eq!(f[0][2], 255, "헤더의 chunk_count 가 255 여야 한다");
        assert_eq!(f[254][1], 254, "마지막 chunk_idx 는 254");
    }

    #[test]
    fn rejects_exactly_256_chunks() {
        let payload = vec![0u8; 255 * 17 + 1];
        assert!(
            matches!(chunk(0, &payload, 20), Err(FramingError::TooLarge)),
            "256 청크는 u8 에 담기지 않으므로 반드시 거부한다"
        );
    }

    #[test]
    fn round_trips_through_reassembler() {
        let payload: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
        let frames = chunk(9, &payload, 20).unwrap();
        let mut r = Reassembler::new();
        let mut out = None;
        for f in &frames {
            if let Some(msg) = r.push(f) {
                out = Some(msg);
            }
        }
        assert_eq!(out.unwrap(), payload);
    }

    #[test]
    fn discards_frame_when_subscribed_mid_stream() {
        let frames = chunk(1, &[0xEE; 100], 20).unwrap();
        let mut r = Reassembler::new();
        // 첫 청크를 놓친 채 중간부터 수신
        for f in &frames[1..] {
            assert_eq!(r.push(f), None, "0번 청크 없이는 완성되면 안 된다");
        }
    }

    #[test]
    fn new_frame_id_discards_incomplete_previous() {
        let a = chunk(1, &[0xAA; 100], 20).unwrap();
        let b = chunk(2, &[0xBB; 30], 20).unwrap();
        let mut r = Reassembler::new();
        r.push(&a[0]);
        r.push(&a[1]); // 미완성 상태
        let mut out = None;
        for f in &b {
            if let Some(m) = r.push(f) {
                out = Some(m);
            }
        }
        assert_eq!(out.unwrap(), vec![0xBB; 30], "새 frame_id 가 오면 이전 것을 버린다");
    }

    #[test]
    fn out_of_order_chunk_discards_frame() {
        let frames = chunk(5, &[0xCC; 100], 20).unwrap();
        let mut r = Reassembler::new();
        r.push(&frames[0]);
        assert_eq!(r.push(&frames[2]), None, "순서 이탈이면 폐기");
        assert_eq!(r.push(&frames[3]), None, "폐기 후에는 계속 무시");
    }

    /// Swift 쪽 FrameReassembler 와 같은 파일을 읽어 언어 간 프레이밍 불일치를 잡는다.
    /// 벡터를 갱신하려면: UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::framing::tests::golden
    #[test]
    fn golden_vectors_match() {
        use std::path::PathBuf;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/ble-protocol/golden/frames-sample.json");

        let message = "AI Agent Monitor BLE 미러 골든 벡터 — 한글 멀티바이트 포함";
        let chunk_size = 20usize;
        let frame_id = 7u8;
        let frames = chunk(frame_id, message.as_bytes(), chunk_size).unwrap();
        let hex: Vec<String> = frames
            .iter()
            .map(|f| f.iter().map(|b| format!("{b:02x}")).collect())
            .collect();

        let actual = serde_json::json!({
            "chunk_size": chunk_size,
            "frame_id": frame_id,
            "message": message,
            "frames": hex,
        });

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
            return;
        }

        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect(
                "골든 벡터가 없다. UPDATE_GOLDEN=1 로 한 번 생성하고 커밋하라",
            ))
            .unwrap();
        assert_eq!(actual, expected, "프레이밍이 골든 벡터와 어긋났다");
    }
}
