# BLE 전송 계층 (1단계) 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mac이 BLE 주변장치로 스냅샷을 1Hz 스트리밍하고, iPhone 앱이 이를 받아 원본 JSON으로 표시한다.

**Architecture:** Aggregator의 스냅샷을 `tokio::sync::watch`(latest-wins)로 분기해 `BleBridge`가 소비한다. Bridge는 전용 DTO로 축약·직렬화하고 3바이트 헤더로 청킹한 뒤, `BlePeripheral` 트레이트 구현체에 프레임을 넘긴다. macOS 구현체는 `objc2-core-bluetooth`로 `CBPeripheralManager`를 직접 다루며, 송신 큐와 백프레셔를 **메인 스레드에서** 소유한다. iOS는 `CBCentralManager`로 구독해 청크를 재조립한다.

**Tech Stack:** Rust (tokio, serde, objc2 0.6 / objc2-core-bluetooth 0.3) · Svelte 5 · Swift 6 (UIKit, SnapKit, Tuist 4)

**Spec:** `docs/superpowers/specs/2026-08-18-ble-ios-mirror-design.md`

## Global Constraints

- 프로토콜 버전 `PROTOCOL_VERSION = 1`. 모든 메시지 최상위에 `"v":1`.
- GATT UUID는 스펙 4.1의 값을 **문자 그대로** 사용한다. 새로 생성하지 않는다.
  - Service `07A98A35-16C7-4BBA-A296-E28B78B7E683`
  - Info `F494FC3B-ED50-4561-AADE-1A310C5732E6`
  - Auth `1403603A-4C78-4899-A2B8-FDA198101900`
  - Snapshot `0AE789AA-EF38-4A35-9E72-A7CD7AD995D5`
  - Triggers `4F60A8C2-F181-4717-AEE3-07C4D7846597`
- 프레임 헤더는 3바이트 `[frame_id][chunk_idx][chunk_count]`.
- 모든 epoch 시각은 **`u64` 초**.
- 프로젝트 식별자는 **FNV-1a 32비트**. `DefaultHasher` 금지(시드 불안정).
- BLE 공유 기본값은 **off**. 1단계에는 인증이 없으므로 사용자가 켜지 않으면 광고하지 않는다.
- Rust BLE 코드는 전부 `#[cfg(target_os = "macos")]` 게이트. Windows 빌드가 깨지면 안 된다.
- 광고 로컬 이름은 `AIM-` + 호스트명 앞 8자.
- iOS 배포 타깃 **17.0**. 번들 ID 접두사 `com.dgitx.aiagentmonitor.mirror`.
- **BLE는 시뮬레이터에서 동작하지 않는다.** 순수 유닛(Wire·framing·SendQueue·FrameReassembler)은 실기기 없이 검증하고, 실기기는 Task 12에서만 쓴다.

---

## File Structure

**Rust (`src-tauri/src/ble/`)**

| 파일 | 책임 | 순수성 |
|---|---|---|
| `wire.rs` | 전송 DTO + `Snapshot` → DTO 매핑 + FNV-1a | 순수 · 단위 테스트 |
| `framing.rs` | 청커 + 리어셈블러 + 골든 벡터 | 순수 · 단위 테스트 |
| `send_queue.rs` | 백프레셔 송신 큐 (스펙 4.5) | 순수 · 단위 테스트 |
| `peripheral.rs` | `BlePeripheral` 트레이트 · 이벤트 타입 · `FakePeripheral` | 순수 · 테스트 지원 |
| `macos.rs` | `CBPeripheralManager` 실구현 (`#[cfg(macos)]`) | 부수효과 · 수동 검증 |
| `mod.rs` | `BleBridge` 조립 · watch 소비 · 게이트 | Fake로 통합 테스트 |

**수정**: `src-tauri/src/lib.rs`(watch 분기 + command 3개), `src-tauri/Cargo.toml`, `src-tauri/Info.plist`(신규), `src/routes/Detail.svelte`, `src/lib/tauri.ts`, `src/lib/store.svelte.ts`, `src/components/DevicePanel.svelte`(신규)

**공유 골든 벡터**: `docs/ble-protocol/golden/frames-sample.json`, `docs/ble-protocol/golden/snapshot-sample.json`

**iOS (`ios/`)**: `Project.swift`, `Tuist/Package.swift`, `Sources/Wire/`, `Sources/BLETransport/`, `Sources/App/`, `Tests/WireTests/`, `Tests/BLETransportTests/`

---

## Task 1: 전송 DTO와 매핑 (`ble/wire.rs`)

**Files:**
- Create: `src-tauri/src/ble/wire.rs`
- Create: `src-tauri/src/ble/mod.rs` (모듈 선언만)
- Modify: `src-tauri/src/lib.rs:1-8` (`mod ble;` 추가)

**Interfaces:**
- Consumes: `crate::types::{Snapshot, AgentState, ProjectActivity, AgentKind, ActivityStatus}`
- Produces: `PROTOCOL_VERSION: u8`, `MirrorSnapshot`, `MirrorAgent`, `MirrorProject`, `fnv1a_32(&[u8]) -> u32`, `impl From<&Snapshot> for MirrorSnapshot`

- [ ] **Step 1: 모듈을 만들고 lib.rs에 선언한다**

`src-tauri/src/ble/mod.rs`:
```rust
pub mod wire;
```

`src-tauri/src/lib.rs` 1행부터의 `mod` 목록에서 `mod aggregator;` 바로 아래에 한 줄 추가:
```rust
mod ble;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/wire.rs` 하단에 추가:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ActivityStatus, AgentKind, AgentState, ProjectActivity, Snapshot, TokenCounts,
    };
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            emitted_at: UNIX_EPOCH + Duration::from_secs(1_755_500_000),
            agents: vec![AgentState {
                kind: AgentKind::Claude,
                rate_tok_per_sec: 123.5,
                tokens_5h: TokenCounts {
                    tokens_in: 1_000,
                    tokens_out: 2_000,
                    tokens_cache_read: 40_000,
                    tokens_cache_create: 7_000,
                },
                quota_limit: None,
                quota_reset_at: Some(UNIX_EPOCH + Duration::from_secs(1_755_512_400)),
                quota_used_pct: Some(62.0),
                quota_reset_at_weekly: None,
                quota_used_pct_weekly: None,
                projects: vec![ProjectActivity {
                    path: PathBuf::from("/Users/me/dev/foo"),
                    name: "foo".to_string(),
                    model: "claude-opus-5".to_string(),
                    rate_tok_per_sec: 98.25,
                    last_event_at: UNIX_EPOCH + Duration::from_secs(1_755_499_987),
                    status: ActivityStatus::Active,
                }],
                triggered_by: None,
            }],
        }
    }

    #[test]
    fn fnv1a_matches_known_vector() {
        // FNV-1a 32bit 표준 테스트 벡터
        assert_eq!(fnv1a_32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a_32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a_32(b"foobar"), 0xbf9c_f968);
    }

    #[test]
    fn maps_snapshot_to_wire_dto() {
        let m = MirrorSnapshot::from(&sample_snapshot());
        assert_eq!(m.v, PROTOCOL_VERSION);
        assert_eq!(m.t, 1_755_500_000);
        assert_eq!(m.a.len(), 1);

        let a = &m.a[0];
        assert_eq!(a.k, 0, "claude 는 0");
        assert_eq!(a.r, 123.5);
        assert_eq!(a.t5, 3_000, "tokens_in + tokens_out 만 합산(캐시 제외)");
        assert_eq!(a.p5, Some(62.0));
        assert_eq!(a.r5, Some(1_755_512_400));
        assert_eq!(a.pw, None);
        assert_eq!(a.rw, None);

        let p = &a.pj[0];
        assert_eq!(p.id, fnv1a_32(b"/Users/me/dev/foo"));
        assert_eq!(p.n, "foo");
        assert_eq!(p.m, "claude-opus-5");
        assert_eq!(p.t, 1_755_499_987);
        assert_eq!(p.s, 0, "active 는 0");
    }

    #[test]
    fn omits_null_quota_fields_from_json() {
        let m = MirrorSnapshot::from(&sample_snapshot());
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"p5\":62"), "값이 있으면 포함: {json}");
        assert!(!json.contains("\"pw\""), "None 이면 키 자체를 생략: {json}");
        assert!(!json.contains("path"), "전체 경로는 절대 나가지 않는다: {json}");
    }

    #[test]
    fn codex_and_status_codes_map_correctly() {
        let mut s = sample_snapshot();
        s.agents[0].kind = AgentKind::Codex;
        s.agents[0].projects[0].status = ActivityStatus::Dormant;
        let m = MirrorSnapshot::from(&s);
        assert_eq!(m.a[0].k, 1, "codex 는 1");
        assert_eq!(m.a[0].pj[0].s, 2, "dormant 는 2");
    }

    #[test]
    fn epoch_before_unix_epoch_does_not_panic() {
        let mut s = sample_snapshot();
        s.emitted_at = UNIX_EPOCH - Duration::from_secs(10);
        assert_eq!(MirrorSnapshot::from(&s).t, 0, "역행 시각은 0 으로 clamp");
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::wire`
Expected: FAIL — `cannot find type MirrorSnapshot`, `cannot find function fnv1a_32`

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/wire.rs` 상단(테스트 모듈 위)에:
```rust
//! BLE 전송용 DTO. 내부 `Snapshot` 을 그대로 보내지 않는 이유는 스펙 4.3 참조.
//! 요약: SystemTime 직렬화가 장황하고, BLE 대역이 좁고, 내부 타입 변경으로부터 프로토콜을 보호한다.
use crate::types::{ActivityStatus, AgentKind, AgentState, ProjectActivity, Snapshot};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROTOCOL_VERSION: u8 = 1;

/// FNV-1a 32비트. `DefaultHasher` 는 시드가 불안정해 재시작마다 값이 바뀌므로 쓰지 않는다.
/// 프로젝트 식별자는 앱 재시작·버전 간에도 동일해야 iOS 목록의 diff 가 튀지 않는다.
pub fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in bytes {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn epoch_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MirrorSnapshot {
    pub v: u8,
    pub t: u64,
    pub a: Vec<MirrorAgent>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MirrorAgent {
    /// 0 = claude, 1 = codex
    pub k: u8,
    pub r: f32,
    /// tokens_in + tokens_out. QuotaBar 의 "동기화 전" 표시에만 쓰여 캐시 항목은 보내지 않는다.
    pub t5: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p5: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r5: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pw: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rw: Option<u64>,
    pub pj: Vec<MirrorProject>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MirrorProject {
    /// path 의 FNV-1a. 전체 경로는 프라이버시상 전송하지 않는다.
    pub id: u32,
    pub n: String,
    pub m: String,
    pub r: f32,
    pub t: u64,
    /// 0 = active, 1 = idle, 2 = dormant
    pub s: u8,
}

impl From<&ProjectActivity> for MirrorProject {
    fn from(p: &ProjectActivity) -> Self {
        Self {
            id: fnv1a_32(p.path.to_string_lossy().as_bytes()),
            n: p.name.clone(),
            m: p.model.clone(),
            r: p.rate_tok_per_sec,
            t: epoch_secs(p.last_event_at),
            s: match p.status {
                ActivityStatus::Active => 0,
                ActivityStatus::Idle => 1,
                ActivityStatus::Dormant => 2,
            },
        }
    }
}

impl From<&AgentState> for MirrorAgent {
    fn from(a: &AgentState) -> Self {
        Self {
            k: match a.kind {
                AgentKind::Claude => 0,
                AgentKind::Codex => 1,
            },
            r: a.rate_tok_per_sec,
            t5: a
                .tokens_5h
                .tokens_in
                .saturating_add(a.tokens_5h.tokens_out),
            p5: a.quota_used_pct,
            r5: a.quota_reset_at.map(epoch_secs),
            pw: a.quota_used_pct_weekly,
            rw: a.quota_reset_at_weekly.map(epoch_secs),
            pj: a.projects.iter().map(MirrorProject::from).collect(),
        }
    }
}

impl From<&Snapshot> for MirrorSnapshot {
    fn from(s: &Snapshot) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            t: epoch_secs(s.emitted_at),
            a: s.agents.iter().map(MirrorAgent::from).collect(),
        }
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::wire`
Expected: PASS — 5 tests

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/ src-tauri/src/lib.rs
git commit -m "feat(ble): 전송용 DTO와 Snapshot 매핑 추가"
```

---

## Task 2: 청킹·재조립과 골든 벡터 (`ble/framing.rs`)

**Files:**
- Create: `src-tauri/src/ble/framing.rs`
- Create: `docs/ble-protocol/golden/frames-sample.json` (테스트가 생성)
- Modify: `src-tauri/src/ble/mod.rs`

**Interfaces:**
- Consumes: 없음 (순수)
- Produces: `HEADER_LEN: usize`, `FramingError`, `chunk(frame_id: u8, payload: &[u8], max_chunk: usize) -> Result<Vec<Vec<u8>>, FramingError>`, `Reassembler::new()`, `Reassembler::push(&mut self, packet: &[u8]) -> Option<Vec<u8>>`

- [ ] **Step 1: 모듈을 선언한다**

`src-tauri/src/ble/mod.rs` 에 추가:
```rust
pub mod framing;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/framing.rs` 하단에:
```rust
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
            .join("../docs/ble-protocol/golden/frames-sample.json");

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
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::framing`
Expected: FAIL — `cannot find function chunk`, `cannot find type Reassembler`

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/framing.rs` 상단에:
```rust
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
```

- [ ] **Step 5: 골든 벡터를 생성한다**

Run: `UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::framing::tests::golden_vectors_match`
Expected: PASS. `docs/ble-protocol/golden/frames-sample.json` 이 생성된다.

파일을 열어 `frames` 배열의 각 항목이 `07`(frame_id) 로 시작하고 두 번째 바이트가 `00,01,02…` 로 증가하는지 눈으로 확인한다.

- [ ] **Step 6: 전체 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::framing`
Expected: PASS — 10 tests

- [ ] **Step 7: 커밋한다**

```bash
git add src-tauri/src/ble/framing.rs src-tauri/src/ble/mod.rs docs/ble-protocol/
git commit -m "feat(ble): 청킹·재조립과 언어 간 골든 벡터 추가"
```

---

## Task 3: 백프레셔 송신 큐 (`ble/send_queue.rs`)

스펙 4.5. `updateValue` 가 `false` 를 반환하면 그 청크는 **버려진다**. 반환값을 무시하면 프레임이 영원히 완성되지 않는다.

**Files:**
- Create: `src-tauri/src/ble/send_queue.rs`
- Modify: `src-tauri/src/ble/mod.rs`

**Interfaces:**
- Consumes: 없음 (순수)
- Produces: `SendQueue::new()`, `SendQueue::offer(&mut self, chunks: Vec<Vec<u8>>)`, `SendQueue::pump(&mut self, send: impl FnMut(&[u8]) -> bool)`, `SendQueue::on_ready(&mut self)`, `SendQueue::is_idle(&self) -> bool`

- [ ] **Step 1: 모듈을 선언한다**

`src-tauri/src/ble/mod.rs` 에 추가:
```rust
pub mod send_queue;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/send_queue.rs` 하단에:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// n번째 호출까지만 성공하고 이후 false 를 돌려주는 가짜 송신기
    fn limited(limit: usize) -> (impl FnMut(&[u8]) -> bool, std::rc::Rc<std::cell::RefCell<Vec<Vec<u8>>>>) {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let s = sent.clone();
        let f = move |c: &[u8]| {
            if s.borrow().len() >= limit {
                return false;
            }
            s.borrow_mut().push(c.to_vec());
            true
        };
        (f, sent)
    }

    #[test]
    fn sends_all_chunks_when_not_saturated() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(100);
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 3);
        assert!(q.is_idle());
    }

    #[test]
    fn pauses_on_saturation_and_resumes_on_ready() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(2);
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 2, "2개까지만 나가고 멈춘다");
        assert!(!q.is_idle());

        // 포화 상태에서 다시 pump 해도 나가지 않아야 한다
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 2);

        // 큐가 비워졌다는 신호가 오면 재개
        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(sent2.borrow().len(), 1, "남은 1개가 나간다");
        assert!(q.is_idle());
    }

    #[test]
    fn replaces_untouched_frame_with_latest() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2]]);
        q.offer(vec![vec![9]]); // 아직 한 청크도 안 보냈으므로 통째로 교체
        let (mut send, sent) = limited(100);
        q.pump(&mut send);
        assert_eq!(*sent.borrow(), vec![vec![9u8]], "오래된 프레임은 버린다");
    }

    #[test]
    fn finishes_started_frame_before_switching() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(1);
        q.pump(&mut send); // 1개 전송 후 포화 → 이 프레임은 "시작됨"
        assert_eq!(sent.borrow().len(), 1);

        q.offer(vec![vec![9]]); // 진행 중 프레임은 끝까지 보낸 뒤 교체
        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(
            *sent2.borrow(),
            vec![vec![2u8], vec![3u8], vec![9u8]],
            "시작한 프레임을 마친 뒤 최신 프레임을 보낸다"
        );
    }

    #[test]
    fn only_latest_pending_frame_is_kept() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2]]);
        let (mut send, _sent) = limited(1);
        q.pump(&mut send); // 시작됨

        q.offer(vec![vec![7]]);
        q.offer(vec![vec![8]]); // 7 은 버려지고 8 만 남는다

        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(*sent2.borrow(), vec![vec![2u8], vec![8u8]]);
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::send_queue`
Expected: FAIL — `cannot find type SendQueue`

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/send_queue.rs` 상단에:
```rust
//! 백프레셔 송신 큐 (스펙 4.5).
//!
//! CoreBluetooth 의 updateValue 는 전송 큐가 가득 차면 false 를 반환하고 **그 청크를 버린다**.
//! 반환값을 무시하면 프레임 중간 청크가 조용히 사라져 수신 측이 영원히 프레임을 완성하지 못한다.
//!
//! 정책: 최신값 우선. 아직 한 청크도 보내지 않은 프레임은 새 프레임으로 통째 교체하고,
//! 이미 일부를 보낸 프레임은 끝까지 보낸 뒤 교체한다(수신 측 부분 프레임 폐기 비용 절감).
use std::collections::VecDeque;

#[derive(Debug, Default)]
pub struct SendQueue {
    current: VecDeque<Vec<u8>>,
    /// current 의 청크를 하나라도 실제로 보냈는지
    started: bool,
    /// 다음에 보낼 최신 프레임. 새 offer 가 오면 통째로 덮어쓴다.
    next: Option<Vec<Vec<u8>>>,
    paused: bool,
}

impl SendQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offer(&mut self, chunks: Vec<Vec<u8>>) {
        if self.started && !self.current.is_empty() {
            // 진행 중인 프레임은 건드리지 않고, 대기 슬롯만 최신으로 교체한다.
            self.next = Some(chunks);
        } else {
            self.current = chunks.into();
            self.started = false;
            self.next = None;
        }
    }

    /// `send` 는 성공 시 true, 전송 큐 포화 시 false 를 반환해야 한다.
    pub fn pump(&mut self, mut send: impl FnMut(&[u8]) -> bool) {
        loop {
            if self.current.is_empty() {
                match self.next.take() {
                    Some(n) => {
                        self.current = n.into();
                        self.started = false;
                    }
                    None => return,
                }
            }
            if self.paused {
                return;
            }
            let Some(front) = self.current.front() else {
                return;
            };
            if send(front) {
                self.current.pop_front();
                self.started = true;
            } else {
                self.paused = true;
                return;
            }
        }
    }

    /// peripheralManagerIsReadyToUpdateSubscribers: 수신 시 호출한다.
    pub fn on_ready(&mut self) {
        self.paused = false;
    }

    pub fn is_idle(&self) -> bool {
        self.current.is_empty() && self.next.is_none()
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::send_queue`
Expected: PASS — 5 tests

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/send_queue.rs src-tauri/src/ble/mod.rs
git commit -m "feat(ble): 백프레셔 송신 큐 추가"
```

---

## Task 4: 주변장치 트레이트와 Fake (`ble/peripheral.rs`)

트레이트를 두는 목적은 구현 교체가 아니라 **BLE 없이 `BleBridge` 를 테스트하기 위함**이다.

**Files:**
- Create: `src-tauri/src/ble/peripheral.rs`
- Modify: `src-tauri/src/ble/mod.rs`

**Interfaces:**
- Consumes: 없음
- Produces: `CharId` (enum: `Snapshot`, `Triggers`, `Auth`, `Info`), `CentralId(String)`, `Subscriber { id: CentralId, max_notify_len: usize }`, `PeripheralEvent`, `trait BlePeripheral`, `FakePeripheral`
- 상수: `SERVICE_UUID`, `INFO_UUID`, `AUTH_UUID`, `SNAPSHOT_UUID`, `TRIGGERS_UUID` (&str)

- [ ] **Step 1: 모듈을 선언한다**

`src-tauri/src/ble/mod.rs` 에 추가:
```rust
pub mod peripheral;
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/peripheral.rs` 하단에:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_offered_frames() {
        let p = FakePeripheral::new();
        p.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 20,
        }]);
        p.offer_frame(CharId::Snapshot, vec![vec![1, 2, 3], vec![4]]);
        let frames = p.taken_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, CharId::Snapshot);
        assert_eq!(frames[0].1, vec![vec![1, 2, 3], vec![4]]);
    }

    #[test]
    fn fake_reports_smallest_subscriber_mtu() {
        let p = FakePeripheral::new();
        p.set_subscribers(vec![
            Subscriber { id: CentralId("A".into()), max_notify_len: 185 },
            Subscriber { id: CentralId("B".into()), max_notify_len: 23 },
        ]);
        assert_eq!(
            p.min_notify_len(),
            Some(23),
            "가장 작은 구독자에 맞춰야 모두가 받을 수 있다"
        );
    }

    #[test]
    fn min_notify_len_is_none_without_subscribers() {
        let p = FakePeripheral::new();
        assert_eq!(p.min_notify_len(), None);
    }

    #[test]
    fn uuids_match_spec() {
        assert_eq!(SERVICE_UUID, "07A98A35-16C7-4BBA-A296-E28B78B7E683");
        assert_eq!(SNAPSHOT_UUID, "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5");
        assert_eq!(TRIGGERS_UUID, "4F60A8C2-F181-4717-AEE3-07C4D7846597");
        assert_eq!(AUTH_UUID, "1403603A-4C78-4899-A2B8-FDA198101900");
        assert_eq!(INFO_UUID, "F494FC3B-ED50-4561-AADE-1A310C5732E6");
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::peripheral`
Expected: FAIL — `cannot find type FakePeripheral`

- [ ] **Step 4: 최소 구현을 쓴다**

`src-tauri/src/ble/peripheral.rs` 상단에:
```rust
//! BLE 주변장치 추상화.
//!
//! 트레이트를 두는 이유는 구현 교체가 아니라 **BLE 하드웨어 없이 BleBridge 를 테스트하기 위함**이다.
//! 실기기 의존을 줄이는 것이 이 프로젝트의 가장 큰 개발 비용 절감 수단이다.
use std::sync::Mutex;

pub const SERVICE_UUID: &str = "07A98A35-16C7-4BBA-A296-E28B78B7E683";
pub const INFO_UUID: &str = "F494FC3B-ED50-4561-AADE-1A310C5732E6";
pub const AUTH_UUID: &str = "1403603A-4C78-4899-A2B8-FDA198101900";
pub const SNAPSHOT_UUID: &str = "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5";
pub const TRIGGERS_UUID: &str = "4F60A8C2-F181-4717-AEE3-07C4D7846597";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharId {
    Info,
    Auth,
    Snapshot,
    Triggers,
}

impl CharId {
    pub fn uuid(self) -> &'static str {
        match self {
            CharId::Info => INFO_UUID,
            CharId::Auth => AUTH_UUID,
            CharId::Snapshot => SNAPSHOT_UUID,
            CharId::Triggers => TRIGGERS_UUID,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CentralId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    pub id: CentralId,
    /// CBCentral.maximumUpdateValueLength — central 마다 다르므로 구독 시점에 실측한다.
    pub max_notify_len: usize,
}

#[derive(Debug, Clone)]
pub enum PeripheralEvent {
    PoweredOn,
    PoweredOff,
    Subscribed(Subscriber),
    Unsubscribed(CentralId),
    AdvertisingStarted,
    Error(String),
}

pub trait BlePeripheral: Send + Sync {
    fn start(&self) -> anyhow::Result<()>;
    fn stop(&self);
    /// 프레임을 넘긴다. 실제 전송과 백프레셔는 구현체가 책임진다(fire-and-forget).
    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>);
    fn subscribers(&self) -> Vec<Subscriber>;
    /// 모든 구독자가 받을 수 있는 최대 청크 크기. 구독자가 없으면 None.
    fn min_notify_len(&self) -> Option<usize> {
        self.subscribers().iter().map(|s| s.max_notify_len).min()
    }
}

/// 테스트용 구현. 넘어온 프레임을 기록만 한다.
#[derive(Debug, Default)]
pub struct FakePeripheral {
    frames: Mutex<Vec<(CharId, Vec<Vec<u8>>)>>,
    subs: Mutex<Vec<Subscriber>>,
    started: Mutex<bool>,
}

impl FakePeripheral {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_subscribers(&self, subs: Vec<Subscriber>) {
        *self.subs.lock().unwrap() = subs;
    }
    /// 기록된 프레임을 꺼내고 비운다.
    pub fn taken_frames(&self) -> Vec<(CharId, Vec<Vec<u8>>)> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }
    pub fn is_started(&self) -> bool {
        *self.started.lock().unwrap()
    }
}

impl BlePeripheral for FakePeripheral {
    fn start(&self) -> anyhow::Result<()> {
        *self.started.lock().unwrap() = true;
        Ok(())
    }
    fn stop(&self) {
        *self.started.lock().unwrap() = false;
    }
    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>) {
        self.frames.lock().unwrap().push((ch, chunks));
    }
    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs.lock().unwrap().clone()
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::peripheral`
Expected: PASS — 4 tests

- [ ] **Step 6: 커밋한다**

```bash
git add src-tauri/src/ble/peripheral.rs src-tauri/src/ble/mod.rs
git commit -m "feat(ble): 주변장치 트레이트와 테스트용 Fake 추가"
```

---

## Task 5: BleBridge 조립 (`ble/mod.rs`)

**Files:**
- Modify: `src-tauri/src/ble/mod.rs`

**Interfaces:**
- Consumes: `wire::MirrorSnapshot`, `framing::chunk`, `peripheral::{BlePeripheral, CharId, FakePeripheral}`, `crate::emitter::EmitGate`, `crate::types::Snapshot`
- Produces: `BleBridge::new(peripheral: Arc<dyn BlePeripheral>) -> Self`, `BleBridge::on_snapshot(&mut self, snap: &Snapshot, now: SystemTime)`, `BleBridge::set_enabled(&mut self, bool)`, `BleBridge::is_enabled(&self) -> bool`

`EmitGate` 는 `crate::emitter::EmitGate` 로 그대로 쓸 수 있다. Rust 에서 크레이트 루트의 비공개 모듈은 후손 모듈에서 접근 가능하므로 `lib.rs` 의 `mod emitter;` 는 **건드리지 않는다**.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`src-tauri/src/ble/mod.rs` 하단에:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, AgentState, Snapshot, TokenCounts};
    use std::time::{Duration, UNIX_EPOCH};

    fn snap(rate: f32, at: u64) -> Snapshot {
        Snapshot {
            emitted_at: UNIX_EPOCH + Duration::from_secs(at),
            agents: vec![AgentState {
                kind: AgentKind::Claude,
                rate_tok_per_sec: rate,
                tokens_5h: TokenCounts::default(),
                quota_limit: None,
                quota_reset_at: None,
                quota_used_pct: None,
                quota_reset_at_weekly: None,
                quota_used_pct_weekly: None,
                projects: vec![],
                triggered_by: None,
            }],
        }
    }

    fn bridge() -> (BleBridge, Arc<FakePeripheral>) {
        let fake = Arc::new(FakePeripheral::new());
        (BleBridge::new(fake.clone()), fake)
    }

    #[test]
    fn does_nothing_while_disabled() {
        let (mut b, fake) = bridge();
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(), "꺼져 있으면 아무것도 보내지 않는다");
    }

    #[test]
    fn does_nothing_without_subscribers() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));
        assert!(fake.taken_frames().is_empty(), "구독자가 없으면 직렬화도 하지 않는다");
    }

    #[test]
    fn emits_chunked_snapshot_frame() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        b.on_snapshot(&snap(1.0, 1000), UNIX_EPOCH + Duration::from_secs(1000));

        let frames = fake.taken_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, CharId::Snapshot);

        // 재조립하면 원래 JSON 이 나와야 한다
        let mut r = framing::Reassembler::new();
        let mut msg = None;
        for c in &frames[0].1 {
            if let Some(m) = r.push(c) {
                msg = Some(m);
            }
        }
        let json = String::from_utf8(msg.expect("프레임이 완성되어야 한다")).unwrap();
        assert!(json.starts_with("{\"v\":1"), "실제 JSON: {json}");
    }

    #[test]
    fn throttles_to_one_hz() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        b.on_snapshot(&snap(1.0, 1000), t0);
        assert_eq!(fake.taken_frames().len(), 1);

        // 내용이 바뀌어도 1초가 안 지나면 보내지 않는다
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_millis(400));
        assert!(fake.taken_frames().is_empty());

        b.on_snapshot(&snap(3.0, 1000), t0 + Duration::from_millis(1100));
        assert_eq!(fake.taken_frames().len(), 1);
    }

    #[test]
    fn frame_id_increments_per_frame() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
        fake.set_subscribers(vec![Subscriber {
            id: CentralId("A".into()),
            max_notify_len: 185,
        }]);
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        b.on_snapshot(&snap(1.0, 1000), t0);
        let a = fake.taken_frames()[0].1[0][0];
        b.on_snapshot(&snap(2.0, 1000), t0 + Duration::from_secs(2));
        let c = fake.taken_frames()[0].1[0][0];
        assert_eq!(c, a.wrapping_add(1), "frame_id 는 프레임마다 증가한다");
    }

    #[test]
    fn disabling_stops_the_peripheral() {
        let (mut b, fake) = bridge();
        b.set_enabled(true);
        assert!(fake.is_started());
        b.set_enabled(false);
        assert!(!fake.is_started());
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::tests`
Expected: FAIL — `cannot find type BleBridge`

- [ ] **Step 3: 최소 구현을 쓴다**

`src-tauri/src/ble/mod.rs` 를 다음으로 교체(테스트 모듈은 남긴다):
```rust
//! BLE 미러 전송 계층. 조립 지점은 `BleBridge`.
pub mod framing;
pub mod peripheral;
pub mod send_queue;
pub mod wire;

use crate::emitter::EmitGate;
use crate::types::Snapshot;
use peripheral::{BlePeripheral, CentralId, CharId, FakePeripheral, Subscriber};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wire::MirrorSnapshot;

/// 스냅샷 송출 상한. 기존 EmitGate(500ms)보다 느슨하게 잡아 BLE 대역을 아낀다.
const BLE_THROTTLE: Duration = Duration::from_millis(1000);

pub struct BleBridge {
    peripheral: Arc<dyn BlePeripheral>,
    gate: EmitGate,
    enabled: bool,
    next_frame_id: u8,
}

impl BleBridge {
    pub fn new(peripheral: Arc<dyn BlePeripheral>) -> Self {
        Self {
            peripheral,
            gate: EmitGate::new(BLE_THROTTLE),
            enabled: false,
            next_frame_id: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if on {
            if let Err(e) = self.peripheral.start() {
                tracing::error!("BLE 시작 실패: {e}");
                self.enabled = false;
            }
        } else {
            self.peripheral.stop();
        }
    }

    /// 스냅샷 틱마다 호출한다. 게이트·구독자·직렬화·청킹을 모두 여기서 판단한다.
    pub fn on_snapshot(&mut self, snap: &Snapshot, now: SystemTime) {
        if !self.enabled {
            return;
        }
        // 구독자가 없으면 직렬화조차 하지 않는다(스펙 4.4).
        let Some(max_chunk) = self.peripheral.min_notify_len() else {
            return;
        };
        if !self.gate.should_emit(snap, now) {
            return;
        }

        let dto = MirrorSnapshot::from(snap);
        let json = match serde_json::to_vec(&dto) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("스냅샷 직렬화 실패: {e}");
                return;
            }
        };
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);

        match framing::chunk(frame_id, &json, max_chunk) {
            Ok(chunks) => self.peripheral.offer_frame(CharId::Snapshot, chunks),
            Err(e) => tracing::error!("청킹 실패: {e:?}"),
        }
    }
}
```

- [ ] **Step 4: 테스트가 통과하는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::`
Expected: PASS — Task 1~5 전체 통과

- [ ] **Step 5: 커밋한다**

```bash
git add src-tauri/src/ble/mod.rs
git commit -m "feat(ble): BleBridge 조립 및 게이트·청킹 연결"
```

---

## Task 6: macOS 주변장치 실구현 (`ble/macos.rs`)

0단계 스파이크에서 검증된 코드를 이식한다. **BLE 하드웨어를 쓰므로 단위 테스트가 없다** — 컴파일과 Task 12의 실기기 검증으로 확인한다.

**Files:**
- Create: `src-tauri/src/ble/macos.rs`
- Create: `src-tauri/Info.plist`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/tauri.conf.json`

**Interfaces:**
- Consumes: `peripheral::{BlePeripheral, CharId, CentralId, Subscriber, PeripheralEvent, SERVICE_UUID, …}`, `send_queue::SendQueue`
- Produces: `MacPeripheral::new(app: tauri::AppHandle, events: tokio::sync::mpsc::UnboundedSender<PeripheralEvent>) -> Self`, `MacPeripheral::apply_event(&self, &PeripheralEvent)`

- [ ] **Step 1: 모듈을 macOS 전용으로 선언한다**

`src-tauri/src/ble/mod.rs` 의 `pub mod wire;` 아래에 추가한다. **이 선언은 Task 6 소관이다** — 더 앞 태스크에서 선언하면 `macos.rs` 가 없어 컴파일이 깨진다:
```rust
#[cfg(target_os = "macos")]
pub mod macos;
```

- [ ] **Step 2: 의존성을 macOS 전용으로 추가한다**

`src-tauri/Cargo.toml` 의 `[dev-dependencies]` **위에** 추가한다. 타깃별 의존성이라 Windows 빌드에는 아예 들어가지 않는다:
```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSArray", "NSData", "NSDictionary", "NSObject", "NSError", "NSValue", "NSUUID"] }
objc2-core-bluetooth = { version = "0.3", features = ["CBPeripheralManager", "CBPeripheralManagerConstants", "CBAdvertisementData", "CBCentral", "CBService", "CBCharacteristic", "CBDescriptor", "CBUUID", "CBAttribute", "CBATTRequest", "CBError", "CBManager", "CBPeer", "CBDefines"] }
```

- [ ] **Step 3: Bluetooth 사용 설명을 추가한다**

이 문자열이 없으면 **공증된 빌드가 첫 CoreBluetooth 호출에서 즉시 종료된다**(스펙 5.3).

`src-tauri/Info.plist` 생성:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>NSBluetoothAlwaysUsageDescription</key>
  <string>iPhone 등 클라이언트 기기로 모니터링 화면을 전송하기 위해 블루투스를 사용합니다.</string>
</dict>
</plist>
```

Tauri 2 는 `src-tauri/Info.plist` 를 자동 병합한다. 별도 설정은 필요 없다.

- [ ] **Step 4: 구현을 쓴다**

`src-tauri/src/ble/macos.rs`:
```rust
//! CBPeripheralManager 직접 구현 (스펙 3.2). 0단계 스파이크에서 검증한 방식이다.
//!
//! 스레드 규약: CoreBluetooth 호출과 델리게이트 콜백을 모두 **메인 스레드**에서 처리한다.
//! CBPeripheralManager 를 queue=None 으로 만들면 콜백이 메인 큐로 오고, Tauri 가 이미
//! 메인 런루프를 돌리고 있으므로 별도 런루프가 필요 없다.
//! SendQueue 도 메인 스레드가 소유해 updateValue 의 bool 반환을 스레드 왕복 없이 처리한다.
//! tokio 쪽에서는 `offer_frame` 으로 프레임만 던지고(fire-and-forget) 즉시 돌아온다.
use super::peripheral::{
    BlePeripheral, CentralId, CharId, PeripheralEvent, Subscriber, SERVICE_UUID,
};
use super::send_queue::SendQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_bluetooth::*;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::mpsc::UnboundedSender;

fn uuid(s: &str) -> Retained<CBUUID> {
    unsafe { CBUUID::UUIDWithString(&NSString::from_str(s)) }
}

/// 메인 스레드에서만 접근하는 상태.
struct MainState {
    manager: Option<Retained<CBPeripheralManager>>,
    chars: HashMap<&'static str, Retained<CBMutableCharacteristic>>,
    subs: HashMap<String, (Retained<CBCentral>, usize)>,
    queues: HashMap<&'static str, SendQueue>,
    events: UnboundedSender<PeripheralEvent>,
}

impl MainState {
    /// 큐에 쌓인 청크를 가능한 만큼 내보낸다.
    fn pump(&mut self, ch_uuid: &'static str) {
        let (Some(mgr), Some(ch)) = (self.manager.clone(), self.chars.get(ch_uuid).cloned())
        else {
            return;
        };
        let centrals: Vec<Retained<CBCentral>> =
            self.subs.values().map(|(c, _)| c.clone()).collect();
        if centrals.is_empty() {
            return;
        }
        let refs: Vec<&CBCentral> = centrals.iter().map(|c| &**c).collect();
        let targets = NSArray::from_slice(&refs);
        let Some(q) = self.queues.get_mut(ch_uuid) else {
            return;
        };
        q.pump(|chunk| {
            let data = NSData::with_bytes(chunk);
            // onSubscribedCentrals 를 명시해 인가 대상만 지정할 수 있게 해둔다(3단계 페어링 대비).
            unsafe {
                mgr.updateValue_forCharacteristic_onSubscribedCentrals(
                    &data,
                    &ch,
                    Some(&targets),
                )
            }
        });
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AimBleDelegate"]
    #[ivars = RefCell<MainState>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl CBPeripheralManagerDelegate for Delegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, mgr: &CBPeripheralManager) {
            let powered = unsafe { mgr.state() } == CBManagerState::PoweredOn;
            let st = self.ivars().borrow();
            let _ = st.events.send(if powered {
                PeripheralEvent::PoweredOn
            } else {
                PeripheralEvent::PoweredOff
            });
            drop(st);
            if powered {
                self.publish(mgr);
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(&self, mgr: &CBPeripheralManager, _s: &CBService, err: Option<&NSError>) {
            if let Some(e) = err {
                let _ = self.ivars().borrow().events.send(PeripheralEvent::Error(e.to_string()));
                return;
            }
            self.advertise(mgr);
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_adv(&self, _m: &CBPeripheralManager, err: Option<&NSError>) {
            let st = self.ivars().borrow();
            let _ = st.events.send(match err {
                Some(e) => PeripheralEvent::Error(e.to_string()),
                None => PeripheralEvent::AdvertisingStarted,
            });
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(&self, _m: &CBPeripheralManager, central: &CBCentral, ch: &CBCharacteristic) {
            let mtu = unsafe { central.maximumUpdateValueLength() };
            let id = central_id(central);
            {
                let mut st = self.ivars().borrow_mut();
                st.subs.insert(id.clone(), (central.retain(), mtu));
                let _ = st.events.send(PeripheralEvent::Subscribed(Subscriber {
                    id: CentralId(id),
                    max_notify_len: mtu,
                }));
            }
            let _ = ch;
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(&self, _m: &CBPeripheralManager, central: &CBCentral, _c: &CBCharacteristic) {
            let id = central_id(central);
            let mut st = self.ivars().borrow_mut();
            st.subs.remove(&id);
            let _ = st.events.send(PeripheralEvent::Unsubscribed(CentralId(id)));
        }

        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn ready(&self, _m: &CBPeripheralManager) {
            // 스펙 4.5: 포화 해제 신호. 여기서 재개하지 않으면 프레임이 영원히 미완성으로 남는다.
            {
                let mut st = self.ivars().borrow_mut();
                for q in st.queues.values_mut() {
                    q.on_ready();
                }
            }
            self.ivars().borrow_mut().pump(CharId::Snapshot.uuid());
        }
    }
);

fn central_id(c: &CBCentral) -> String {
    let id: Retained<NSObject> = unsafe { msg_send![c, identifier] };
    format!("{id:?}")
}

impl Delegate {
    fn publish(&self, mgr: &CBPeripheralManager) {
        let snapshot_ch: Retained<CBMutableCharacteristic> = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &uuid(CharId::Snapshot.uuid()),
                CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Readable,
            )
        };
        let svc: Retained<CBMutableService> = unsafe {
            CBMutableService::initWithType_primary(CBMutableService::alloc(), &uuid(SERVICE_UUID), true)
        };
        let chars = NSArray::from_slice(&[&*snapshot_ch]);
        unsafe { svc.setCharacteristics(Some(&Retained::cast_unchecked(chars))) };
        self.ivars()
            .borrow_mut()
            .chars
            .insert(CharId::Snapshot.uuid(), snapshot_ch);
        unsafe { mgr.addService(&svc) };
    }

    fn advertise(&self, mgr: &CBPeripheralManager) {
        let host = hostname_prefix();
        let name = NSString::from_str(&format!("AIM-{host}"));
        let uuids = NSArray::from_slice(&[&*uuid(SERVICE_UUID)]);
        let ad: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
            Retained::cast_unchecked(NSDictionary::from_slices(
                &[CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey],
                &[
                    &*Retained::cast_unchecked::<NSObject>(name),
                    &*Retained::cast_unchecked::<NSObject>(uuids),
                ],
            ))
        };
        unsafe { mgr.startAdvertising(Some(&ad)) };
    }
}

fn hostname_prefix() -> String {
    let h = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    h.trim().chars().take(8).collect()
}

pub struct MacPeripheral {
    app: AppHandle,
    delegate: Mutex<Option<Retained<Delegate>>>,
    events: UnboundedSender<PeripheralEvent>,
    /// 메인 스레드가 아닌 곳에서 조회하기 위한 구독자 사본
    subs_mirror: Mutex<Vec<Subscriber>>,
}

// Retained<Delegate> 는 Send 가 아니므로 메인 스레드 밖으로 새어나가지 않게
// 모든 접근을 run_on_main_thread 안으로 가둔다.
unsafe impl Send for MacPeripheral {}
unsafe impl Sync for MacPeripheral {}

impl MacPeripheral {
    pub fn new(app: AppHandle, events: UnboundedSender<PeripheralEvent>) -> Self {
        Self {
            app,
            delegate: Mutex::new(None),
            events,
            subs_mirror: Mutex::new(Vec::new()),
        }
    }

    /// 델리게이트가 보내는 구독 이벤트를 받아 사본을 갱신한다. lib.rs 의 이벤트 루프가 호출한다.
    pub fn apply_event(&self, ev: &PeripheralEvent) {
        let mut m = self.subs_mirror.lock().unwrap();
        match ev {
            PeripheralEvent::Subscribed(s) => {
                m.retain(|x| x.id != s.id);
                m.push(s.clone());
            }
            PeripheralEvent::Unsubscribed(id) => m.retain(|x| &x.id != id),
            PeripheralEvent::PoweredOff => m.clear(),
            _ => {}
        }
    }
}

impl BlePeripheral for MacPeripheral {
    fn start(&self) -> anyhow::Result<()> {
        let events = self.events.clone();
        self.app.run_on_main_thread(move || {
            let state = MainState {
                manager: None,
                chars: HashMap::new(),
                subs: HashMap::new(),
                queues: HashMap::from([(CharId::Snapshot.uuid(), SendQueue::new())]),
                events,
            };
            let d = Delegate::alloc().set_ivars(RefCell::new(state));
            let d: Retained<Delegate> = unsafe { msg_send![super(d), init] };
            let proto = ProtocolObject::from_ref(&*d);
            let mgr: Retained<CBPeripheralManager> = unsafe {
                CBPeripheralManager::initWithDelegate_queue(
                    CBPeripheralManager::alloc(),
                    Some(proto),
                    None,
                )
            };
            d.ivars().borrow_mut().manager = Some(mgr);
            // 델리게이트를 살려두기 위해 누출시킨다. stop() 은 광고만 멈춘다.
            std::mem::forget(d);
        })?;
        Ok(())
    }

    fn stop(&self) {
        let _ = self.app.run_on_main_thread(|| {});
        self.subs_mirror.lock().unwrap().clear();
    }

    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>) {
        let _ = (ch, chunks);
        // Task 7 에서 델리게이트 핸들을 통해 메인 스레드로 전달한다.
    }

    fn subscribers(&self) -> Vec<Subscriber> {
        self.subs_mirror.lock().unwrap().clone()
    }
}
```

> **주의**: 위 `offer_frame` 과 `stop` 은 Task 7 에서 델리게이트 핸들 공유를 붙이며 완성한다. 이 태스크의 완료 기준은 **컴파일 성공**이다.

- [ ] **Step 5: macOS 빌드가 통과하는지 확인한다**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: 컴파일 성공 (경고는 허용)

- [ ] **Step 6: Windows 게이트가 유지되는지 확인한다**

Run: `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc 2>&1 | tail -5`
Expected: 타깃이 설치되어 있지 않으면 `target may not be installed` 로 끝난다 — 이 경우 `src-tauri/src/ble/mod.rs` 에서 `#[cfg(target_os = "macos")] pub mod macos;` 게이트가 있는지 눈으로 확인하는 것으로 대체한다.

- [ ] **Step 7: 커밋한다**

```bash
git add src-tauri/src/ble/macos.rs src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/Info.plist
git commit -m "feat(ble): macOS CBPeripheralManager 구현과 Bluetooth 권한 추가"
```

---

## Task 7: lib.rs 연결과 Tauri 명령

**Files:**
- Modify: `src-tauri/src/lib.rs` (mod 선언부, invoke_handler 425-433행, setup 내 틱 루프 533-536행)
- Modify: `src-tauri/src/ble/macos.rs` (offer_frame 완성)

**Interfaces:**
- Consumes: `ble::BleBridge`, `ble::macos::MacPeripheral`, `ble::peripheral::PeripheralEvent`
- Produces: Tauri command `ble_status() -> BleStatus`, `ble_set_enabled(enabled: bool)`, Tauri event `"ble_status"`
- `BleStatus { enabled: bool, advertising: bool, peers: Vec<BlePeer> }`, `BlePeer { id: String, mtu: usize }`

- [ ] **Step 1: macos.rs 의 offer_frame 을 완성한다**

`MacPeripheral` 에 델리게이트 핸들을 저장하도록 `start()` 의 `std::mem::forget(d)` 를 다음으로 바꾸고, 전역 슬롯에 보관한다:

```rust
// macos.rs 상단에 추가
use std::sync::OnceLock;
static DELEGATE: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

fn delegate_slot() -> &'static Mutex<Option<usize>> {
    DELEGATE.get_or_init(|| Mutex::new(None))
}
```

`start()` 의 `std::mem::forget(d);` 를 교체:
```rust
            let raw = Retained::into_raw(d) as usize; // 메인 스레드에서만 역참조한다
            *delegate_slot().lock().unwrap() = Some(raw);
```

`offer_frame` 을 교체:
```rust
    fn offer_frame(&self, ch: CharId, chunks: Vec<Vec<u8>>) {
        let uuid = ch.uuid();
        let _ = self.app.run_on_main_thread(move || {
            let Some(raw) = *delegate_slot().lock().unwrap() else {
                return;
            };
            // 메인 스레드에서만 실행되므로 역참조가 안전하다.
            let d: &Delegate = unsafe { &*(raw as *const Delegate) };
            {
                let mut st = d.ivars().borrow_mut();
                if let Some(q) = st.queues.get_mut(uuid) {
                    q.offer(chunks);
                }
            }
            d.ivars().borrow_mut().pump(uuid);
        });
    }
```

`stop()` 을 교체:
```rust
    fn stop(&self) {
        self.subs_mirror.lock().unwrap().clear();
        let _ = self.app.run_on_main_thread(|| {
            let Some(raw) = *delegate_slot().lock().unwrap() else {
                return;
            };
            let d: &Delegate = unsafe { &*(raw as *const Delegate) };
            if let Some(mgr) = d.ivars().borrow().manager.clone() {
                unsafe { mgr.stopAdvertising() };
            }
        });
    }
```

- [ ] **Step 2: lib.rs 에 상태 타입과 명령을 추가한다**

`src-tauri/src/lib.rs` 의 `use` 블록 뒤(24행 부근)에 추가:
```rust
use ble::peripheral::BlePeripheral;
use ble::BleBridge;

#[derive(Clone, serde::Serialize)]
pub struct BlePeer {
    pub id: String,
    pub mtu: usize,
}

#[derive(Clone, serde::Serialize)]
pub struct BleStatus {
    pub enabled: bool,
    pub advertising: bool,
    pub peers: Vec<BlePeer>,
}

pub struct BleHandle {
    pub bridge: Mutex<BleBridge>,
    #[cfg(target_os = "macos")]
    pub peripheral: std::sync::Arc<ble::macos::MacPeripheral>,
    pub advertising: AtomicBool,
}

#[tauri::command]
async fn ble_status(state: tauri::State<'_, Arc<BleHandle>>) -> Result<BleStatus, String> {
    let bridge = state.bridge.lock().await;
    #[cfg(target_os = "macos")]
    let peers = state
        .peripheral
        .subscribers()
        .into_iter()
        .map(|s| BlePeer { id: s.id.0, mtu: s.max_notify_len })
        .collect();
    #[cfg(not(target_os = "macos"))]
    let peers = Vec::new();
    Ok(BleStatus {
        enabled: bridge.is_enabled(),
        advertising: state.advertising.load(Ordering::Relaxed),
        peers,
    })
}

#[tauri::command]
async fn ble_set_enabled(
    enabled: bool,
    state: tauri::State<'_, Arc<BleHandle>>,
) -> Result<(), String> {
    state.bridge.lock().await.set_enabled(enabled);
    if !enabled {
        state.advertising.store(false, Ordering::Relaxed);
    }
    Ok(())
}
```

- [ ] **Step 3: invoke_handler 에 등록한다**

`src-tauri/src/lib.rs:425-433` 의 `generate_handler!` 목록에서 `sync_quota,` 아래에 두 줄 추가:
```rust
            ble_status,
            ble_set_enabled,
```

- [ ] **Step 4: setup 에서 BleHandle 을 만들고 관리 상태로 등록한다**

`setup` 클로저 안, `let app_handle = app.handle().clone();`(479행 부근) **바로 앞**에 추가:
```rust
                let (ble_tx, mut ble_rx) = mpsc::unbounded_channel::<ble::peripheral::PeripheralEvent>();
                #[cfg(target_os = "macos")]
                let ble_handle = {
                    let periph = std::sync::Arc::new(ble::macos::MacPeripheral::new(
                        app.handle().clone(),
                        ble_tx,
                    ));
                    Arc::new(BleHandle {
                        bridge: Mutex::new(BleBridge::new(periph.clone())),
                        peripheral: periph,
                        advertising: AtomicBool::new(false),
                    })
                };
                #[cfg(not(target_os = "macos"))]
                let ble_handle = {
                    drop(ble_tx);
                    Arc::new(BleHandle {
                        bridge: Mutex::new(BleBridge::new(std::sync::Arc::new(
                            ble::peripheral::FakePeripheral::new(),
                        ))),
                        advertising: AtomicBool::new(false),
                    })
                };
                {
                    use tauri::Manager;
                    app.manage(ble_handle.clone());
                }

                // BLE 이벤트 → 구독자 사본 갱신 + 프론트로 상태 push
                {
                    let h = ble_handle.clone();
                    let app_for_ble = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(ev) = ble_rx.recv().await {
                            #[cfg(target_os = "macos")]
                            h.peripheral.apply_event(&ev);
                            if let ble::peripheral::PeripheralEvent::AdvertisingStarted = ev {
                                h.advertising.store(true, Ordering::Relaxed);
                            }
                            if let ble::peripheral::PeripheralEvent::Error(ref e) = ev {
                                tracing::error!("BLE 오류: {e}");
                            }
                            let _ = app_for_ble.emit("ble_status", ());
                        }
                    });
                }
```

- [ ] **Step 5: 틱 루프에서 BleBridge 에 스냅샷을 넘긴다**

`src-tauri/src/lib.rs:533-536` 의 다음 부분을
```rust
                        let mut g = gate_for_tick.lock().await;
                        if g.should_emit(&snap, std::time::SystemTime::now()) {
                            let _ = app_handle.emit("snapshot", &snap);
                        }
```
이렇게 바꾼다. **BLE 실패가 기존 emit 경로에 영향을 주지 않도록 순서를 뒤에 둔다**:
```rust
                        let now = std::time::SystemTime::now();
                        let mut g = gate_for_tick.lock().await;
                        if g.should_emit(&snap, now) {
                            let _ = app_handle.emit("snapshot", &snap);
                        }
                        drop(g);
                        // BLE 미러는 자체 게이트(1Hz)를 가지며, 꺼져 있거나 구독자가 없으면 즉시 반환한다.
                        ble_for_tick.bridge.lock().await.on_snapshot(&snap, now);
```

틱 루프 spawn 앞의 클론 목록(`let codex_for_tick = codex_quota.clone();` 아래)에 추가:
```rust
                let ble_for_tick = ble_handle.clone();
```

- [ ] **Step 6: 빌드하고 기존 테스트가 깨지지 않았는지 확인한다**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — 기존 테스트 전부 + ble 모듈 테스트

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: 컴파일 성공

- [ ] **Step 7: 커밋한다**

```bash
git add src-tauri/src/lib.rs src-tauri/src/ble/macos.rs
git commit -m "feat(ble): 스냅샷 틱을 BleBridge에 연결하고 상태 명령 추가"
```

---

## Task 8: Detail 창 Devices 탭

**Files:**
- Create: `src/components/DevicePanel.svelte`
- Modify: `src/lib/tauri.ts`, `src/lib/store.svelte.ts`, `src/routes/Detail.svelte`

**Interfaces:**
- Consumes: Tauri command `ble_status`, `ble_set_enabled`, event `ble_status`
- Produces: `store.ble: BleStatus | null`, `store.initBle()`, `store.setBleEnabled(bool)`

- [ ] **Step 1: tauri.ts 에 타입과 래퍼를 추가한다**

`src/lib/tauri.ts` 끝에 추가:
```ts
// ── BLE 미러 ────────────────────────────────────────────────

export type BlePeer = { id: string; mtu: number };
export type BleStatus = {
  enabled: boolean;
  advertising: boolean;
  peers: BlePeer[];
  /// 마지막 BLE 오류. 이 앱에는 tracing subscriber 가 없어 tracing::error! 출력이 전부 유실되므로,
  /// 블루투스 권한 거부 같은 실패는 이 필드로만 사용자에게 도달한다.
  last_error: string | null;
};

export async function bleStatus(): Promise<BleStatus> {
  return invoke<BleStatus>("ble_status");
}

export async function bleSetEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("ble_set_enabled", { enabled });
}

export async function listenBleStatus(cb: () => void): Promise<UnlistenFn> {
  return listen("ble_status", () => cb());
}
```

- [ ] **Step 2: store 에 BLE 상태를 붙인다**

`src/lib/store.svelte.ts` 의 import 목록에 `bleStatus, bleSetEnabled, listenBleStatus, type BleStatus` 를 추가하고, 클래스 안에 추가:
```ts
  ble = $state<BleStatus | null>(null);
  #bleUnlisten: (() => void) | null = null;

  async initBle() {
    if (this.#bleUnlisten) return;
    this.ble = await bleStatus();
    this.#bleUnlisten = await listenBleStatus(async () => {
      this.ble = await bleStatus();
    });
  }

  async setBleEnabled(on: boolean) {
    await bleSetEnabled(on);
    this.ble = await bleStatus();
  }
```

`dispose()` 안에 추가:
```ts
    this.#bleUnlisten?.();
    this.#bleUnlisten = null;
```

- [ ] **Step 3: DevicePanel 컴포넌트를 만든다**

`src/components/DevicePanel.svelte`:
```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "../lib/store.svelte";

  onMount(() => {
    store.initBle();
  });

  let enabled = $derived(store.ble?.enabled ?? false);
  let peers = $derived(store.ble?.peers ?? []);
  let lastError = $derived(store.ble?.last_error ?? null);
</script>

<div class="panel">
  <div class="row">
    <div class="text">
      <strong>BLE 공유</strong>
      <span class="subtle">iPhone 등 클라이언트에 모니터링 화면을 전송합니다</span>
    </div>
    <button
      class="toggle"
      class:on={enabled}
      onclick={() => store.setBleEnabled(!enabled)}
    >
      {enabled ? "켜짐" : "꺼짐"}
    </button>
  </div>

  {#if lastError}
    <p class="error">{lastError}</p>
  {/if}

  {#if enabled}
    <p class="warn">
      1단계에는 기기 인증이 없습니다. 주변의 누구나 연결할 수 있으니 필요할 때만 켜세요.
    </p>
    <p class="subtle status">
      {store.ble?.advertising ? "광고 중 · AIM-*" : "광고 시작 대기 중…"}
    </p>
  {/if}

  <p class="label">연결된 기기</p>
  {#each peers as peer (peer.id)}
    <div class="peer">
      <span class="dot"></span>
      <span class="pid">{peer.id}</span>
      <span class="subtle">MTU {peer.mtu}</span>
    </div>
  {:else}
    <p class="subtle">연결된 기기가 없습니다.</p>
  {/each}
</div>

<style>
  .panel { background: #2c2c2e; border-radius: 8px; padding: 10px 12px; }
  .row { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
  .text { display: flex; flex-direction: column; gap: 2px; }
  .text strong { font-size: 12px; color: #f2f2f7; }
  .toggle {
    background: #3a3a3c; border: none; border-radius: 12px; color: #8e8e93;
    cursor: pointer; font-size: 11px; font-weight: 600; padding: 4px 14px;
  }
  .toggle.on { background: #0a84ff; color: #fff; }
  .warn { color: #ff9f0a; font-size: 10px; margin: 8px 0 0; }
  .error {
    color: #ff453a; font-size: 11px; line-height: 1.4; margin: 8px 0 0;
    background: #3a2a2a; border-radius: 6px; padding: 6px 8px;
  }
  .status { margin: 4px 0 0; }
  .label {
    font-size: 9px; color: #8e8e93; text-transform: uppercase;
    letter-spacing: 0.4px; margin: 12px 0 6px;
  }
  .peer { display: flex; align-items: center; gap: 6px; font-size: 11px; padding: 4px 0; }
  .peer + .peer { border-top: 1px solid #3a3a3c; }
  .dot { width: 6px; height: 6px; border-radius: 50%; background: #30d158; }
  .pid { font-family: ui-monospace, monospace; font-size: 10px; color: #f2f2f7; }
  .subtle { color: #8e8e93; font-size: 10px; }
</style>
```

- [ ] **Step 4: Detail 에 탭을 추가한다**

`src/routes/Detail.svelte` 에서:

import 추가:
```ts
  import DevicePanel from "../components/DevicePanel.svelte";
```

탭 상태 타입 확장:
```ts
  let activeTab = $state<"sessions" | "triggers" | "devices">("sessions");
```

탭 바의 Triggers 버튼 뒤에 추가:
```svelte
    <button class="tab" class:active={activeTab === "devices"} onclick={() => (activeTab = "devices")}>
      Devices
    </button>
```

탭 내용 분기의 `{:else}` 를 다음으로 교체:
```svelte
  {:else if activeTab === "triggers"}
    <div class="triggers">
      <TriggerList />
      <AddTriggerForm />
    </div>
  {:else}
    <DevicePanel />
  {/if}
```

- [ ] **Step 5: 앱을 띄워 확인한다**

Run: `pnpm tauri dev`

확인 항목:
1. Detail 창에 `Devices` 탭이 보인다
2. 토글이 기본 **꺼짐**이다
3. 켜면 "광고 중" 으로 바뀌고, macOS 블루투스 권한 프롬프트가 뜬다면 허용한다
4. 끄면 다시 꺼짐으로 돌아온다
5. **권한을 거부하면** 빨간 오류 문구가 패널에 표시된다. 이 앱에는 tracing subscriber 가 없어
   `tracing::error!` 출력이 전부 유실되므로, 이 화면이 사용자가 실패 원인을 알 수 있는 **유일한 경로**다.
   (권한을 이미 허용했다면 시스템 설정 > 개인정보 보호 및 보안 > Bluetooth 에서 잠시 껐다가 확인)

- [ ] **Step 6: 커밋한다**

```bash
git add src/components/DevicePanel.svelte src/lib/tauri.ts src/lib/store.svelte.ts src/routes/Detail.svelte
git commit -m "feat(ui): Detail 창에 Devices 탭과 BLE 공유 토글 추가"
```

---

## Task 9: iOS Tuist 스캐폴딩과 Wire 모듈

**Files:**
- Create: `ios/Project.swift`, `ios/Tuist/Package.swift`, `ios/.gitignore`
- Create: `ios/Sources/Wire/MirrorSnapshot.swift`
- Create: `ios/Tests/WireTests/MirrorSnapshotTests.swift`
- Create: `docs/ble-protocol/golden/snapshot-sample.json`

**Interfaces:**
- Consumes: `docs/ble-protocol/golden/snapshot-sample.json`
- Produces: Swift `MirrorSnapshot`, `MirrorAgent`, `MirrorProject`, `AgentKindCode`, `ActivityStatusCode`

- [ ] **Step 1: Rust 쪽에서 스냅샷 골든 벡터를 생성한다**

`src-tauri/src/ble/wire.rs` 의 테스트 모듈에 추가:
```rust
    /// Swift Wire 모듈과 공유하는 골든 벡터.
    /// 갱신: UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::wire::tests::golden
    #[test]
    fn golden_snapshot_matches() {
        use std::path::PathBuf;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/ble-protocol/golden/snapshot-sample.json");
        let actual = serde_json::to_value(MirrorSnapshot::from(&sample_snapshot())).unwrap();

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
            return;
        }
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect(
                "골든 벡터가 없다. UPDATE_GOLDEN=1 로 생성하고 커밋하라",
            ))
            .unwrap();
        assert_eq!(actual, expected);
    }
```

Run: `UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::wire::tests::golden_snapshot_matches`
Expected: PASS, `docs/ble-protocol/golden/snapshot-sample.json` 생성

Run: `cargo test --manifest-path src-tauri/Cargo.toml ble::wire`
Expected: PASS — 이제 6 tests

- [ ] **Step 2: Tuist 프로젝트를 만든다**

`ios/.gitignore`:
```
.build/
Derived/
*.xcodeproj
*.xcworkspace
```

`ios/Tuist/Package.swift`:
```swift
// swift-tools-version: 6.0
import PackageDescription

#if TUIST
import ProjectDescription
let packageSettings = PackageSettings(productTypes: ["SnapKit": .framework])
#endif

let package = Package(
    name: "AIAgentMonitorMirrorDeps",
    dependencies: [
        .package(url: "https://github.com/SnapKit/SnapKit", from: "5.7.1")
    ]
)
```

`ios/Project.swift`:
```swift
import ProjectDescription

let bundlePrefix = "com.dgitx.aiagentmonitor.mirror"
let iOS: DeploymentTargets = .iOS("17.0")

func framework(_ name: String, deps: [TargetDependency] = []) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .framework,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: iOS,
        sources: ["Sources/\(name)/**"],
        dependencies: deps
    )
}

func unitTests(_ name: String, for target: String) -> Target {
    .target(
        name: name,
        destinations: .iOS,
        product: .unitTests,
        bundleId: "\(bundlePrefix).\(name.lowercased())",
        deploymentTargets: iOS,
        sources: ["Tests/\(name)/**"],
        resources: ["../docs/ble-protocol/golden/**"],
        dependencies: [.target(name: target)]
    )
}

let project = Project(
    name: "AIAgentMonitorMirror",
    packages: [],
    targets: [
        framework("Wire"),
        unitTests("WireTests", for: "Wire"),
    ]
)
```

- [ ] **Step 3: 실패하는 테스트를 쓴다**

`ios/Tests/WireTests/MirrorSnapshotTests.swift`:
```swift
import XCTest
@testable import Wire

final class MirrorSnapshotTests: XCTestCase {

    /// Rust 가 만든 골든 벡터를 Swift 가 그대로 디코딩할 수 있어야 한다.
    /// 이 테스트가 깨지면 두 언어의 DTO 가 어긋난 것이다.
    func testDecodesGoldenSnapshot() throws {
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "snapshot-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다. Project.swift 의 resources 설정을 확인하라"
        )
        let data = try Data(contentsOf: url)
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: data)

        XCTAssertEqual(snap.v, 1)
        XCTAssertEqual(snap.t, 1_755_500_000)
        XCTAssertEqual(snap.a.count, 1)

        let agent = try XCTUnwrap(snap.a.first)
        XCTAssertEqual(agent.kind, .claude)
        XCTAssertEqual(agent.r, 123.5)
        XCTAssertEqual(agent.t5, 3_000)
        XCTAssertEqual(agent.p5, 62.0)
        XCTAssertEqual(agent.r5, 1_755_512_400)
        XCTAssertNil(agent.pw, "Rust 가 None 이면 키 자체를 생략한다")
        XCTAssertNil(agent.rw)

        let project = try XCTUnwrap(agent.pj.first)
        XCTAssertEqual(project.n, "foo")
        XCTAssertEqual(project.m, "claude-opus-5")
        XCTAssertEqual(project.status, .active)
    }

    func testDecodesUnknownStatusAsDormant() throws {
        let json = #"{"v":1,"t":1,"a":[{"k":9,"r":0,"t5":0,"pj":[{"id":1,"n":"x","m":"m","r":0,"t":0,"s":99}]}]}"#
        let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: Data(json.utf8))
        XCTAssertEqual(snap.a[0].kind, .unknown, "모르는 코드는 크래시가 아니라 unknown 이어야 한다")
        XCTAssertEqual(snap.a[0].pj[0].status, .dormant)
    }
}
```

- [ ] **Step 4: 프로젝트를 생성하고 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist install && tuist generate --no-open
```
Expected: `AIAgentMonitorMirror.xcworkspace` 생성

Run:
```bash
cd ios && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WireTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find type 'MirrorSnapshot' in scope`

> 시뮬레이터 이름이 다르면 `xcrun simctl list devices available` 로 확인해 바꾼다. Wire 는 BLE를 쓰지 않으므로 시뮬레이터로 충분하다.

- [ ] **Step 5: 최소 구현을 쓴다**

`ios/Sources/Wire/MirrorSnapshot.swift`:
```swift
import Foundation

/// Rust `src-tauri/src/ble/wire.rs` 의 DTO 를 그대로 옮긴 것.
/// 짧은 키는 BLE 대역 절약을 위한 것이며, 골든 벡터로 양쪽을 묶어둔다.
public let protocolVersion: UInt8 = 1

public enum AgentKindCode: Equatable, Sendable {
    case claude
    case codex
    case unknown

    init(code: UInt8) {
        switch code {
        case 0: self = .claude
        case 1: self = .codex
        default: self = .unknown
        }
    }
}

public enum ActivityStatusCode: Equatable, Sendable {
    case active
    case idle
    case dormant

    init(code: UInt8) {
        switch code {
        case 0: self = .active
        case 1: self = .idle
        default: self = .dormant
        }
    }
}

public struct MirrorProject: Decodable, Equatable, Sendable {
    public let id: UInt32
    public let n: String
    public let m: String
    public let r: Float
    public let t: UInt64
    public let s: UInt8

    public var name: String { n }
    public var model: String { m }
    public var ratePerSec: Float { r }
    public var lastEventAt: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    public var status: ActivityStatusCode { ActivityStatusCode(code: s) }
}

public struct MirrorAgent: Decodable, Equatable, Sendable {
    public let k: UInt8
    public let r: Float
    public let t5: UInt32
    public let p5: Float?
    public let r5: UInt64?
    public let pw: Float?
    public let rw: UInt64?
    public let pj: [MirrorProject]

    public var kind: AgentKindCode { AgentKindCode(code: k) }
    public var ratePerSec: Float { r }
    public var tokens5h: UInt32 { t5 }
    public var usedPct5h: Float? { p5 }
    public var usedPctWeekly: Float? { pw }
    public var resetAt5h: Date? { r5.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var resetAtWeekly: Date? { rw.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    public var projects: [MirrorProject] { pj }
}

public struct MirrorSnapshot: Decodable, Equatable, Sendable {
    public let v: UInt8
    public let t: UInt64
    public let a: [MirrorAgent]

    public var emittedAt: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    public var agents: [MirrorAgent] { a }
    public var isSupportedVersion: Bool { v == protocolVersion }
}
```

- [ ] **Step 6: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme WireTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: `** TEST SUCCEEDED **` — 2 tests

- [ ] **Step 7: 커밋한다**

```bash
git add ios/ docs/ble-protocol/golden/snapshot-sample.json src-tauri/src/ble/wire.rs
git commit -m "feat(ios): Tuist 스캐폴딩과 Wire 모듈 추가"
```

---

## Task 10: FrameReassembler (`BLETransport`)

Rust `Reassembler` 와 **바이트 단위로 동일**해야 한다. 골든 벡터가 이를 강제한다.

**Files:**
- Create: `ios/Sources/BLETransport/FrameReassembler.swift`
- Create: `ios/Tests/BLETransportTests/FrameReassemblerTests.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: `docs/ble-protocol/golden/frames-sample.json`
- Produces: `FrameReassembler.headerLength`, `FrameReassembler.push(_ packet: Data) -> Data?`

- [ ] **Step 1: Project.swift 에 타깃을 추가한다**

> **Tuist 4.158.2 실측 사항 (Task 9 에서 확인)**: 이 버전의 기본 스킴 자동생성은 테스트 타깃을 본
> 타깃의 스킴에 묶어버려 `WireTests` 같은 **독립 스킴이 생기지 않는다**. 따라서 새 테스트 타깃을
> 추가할 때마다 `Project.swift` 의 `schemes:` 배열에도 항목을 추가해야 `xcodebuild -scheme` 이
> 그 이름을 찾을 수 있다. Task 9 가 `WireTests` 용으로 넣어둔 블록과 같은 형태로 쓴다:
>
> ```swift
> .scheme(
>     name: "BLETransportTests",
>     buildAction: .buildAction(targets: [.target("BLETransportTests")]),
>     testAction: .targets([.testableTarget(target: .target("BLETransportTests"))])
> )
> ```

`ios/Project.swift` 의 `targets:` 배열을 교체:
```swift
    targets: [
        framework("Wire"),
        unitTests("WireTests", for: "Wire"),
        framework("BLETransport", deps: [.target(name: "Wire")]),
        unitTests("BLETransportTests", for: "BLETransport"),
    ]
```

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`ios/Tests/BLETransportTests/FrameReassemblerTests.swift`:
```swift
import XCTest
@testable import BLETransport

final class FrameReassemblerTests: XCTestCase {

    private func packet(_ bytes: [UInt8]) -> Data { Data(bytes) }

    func testSingleChunkFrame() {
        var r = FrameReassembler()
        XCTAssertEqual(r.push(packet([0, 0, 1, 0x41, 0x42])), Data([0x41, 0x42]))
    }

    func testMultiChunkFrame() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([7, 0, 2, 0x41])))
        XCTAssertEqual(r.push(packet([7, 1, 2, 0x42])), Data([0x41, 0x42]))
    }

    func testDiscardsFrameWhenSubscribedMidStream() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([7, 1, 3, 0x42])), "0번을 못 봤으면 완성되면 안 된다")
        XCTAssertNil(r.push(packet([7, 2, 3, 0x43])))
    }

    func testNewFrameIdDiscardsIncompletePrevious() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([1, 0, 3, 0xAA])))
        XCTAssertNil(r.push(packet([1, 1, 3, 0xAA])))
        XCTAssertEqual(r.push(packet([2, 0, 1, 0xBB])), Data([0xBB]))
    }

    func testOutOfOrderChunkDiscardsFrame() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([5, 0, 3, 0xC1])))
        XCTAssertNil(r.push(packet([5, 2, 3, 0xC3])), "순서 이탈이면 폐기")
        XCTAssertNil(r.push(packet([5, 3, 3, 0xC4])), "폐기 후에는 계속 무시")
    }

    func testTooShortPacketIsIgnored() {
        var r = FrameReassembler()
        XCTAssertNil(r.push(packet([1, 2])))
    }

    /// Rust framing.rs 가 생성한 프레임을 그대로 재조립할 수 있어야 한다.
    /// 이 테스트가 언어 간 프레이밍 불일치를 잡는 유일한 안전장치다.
    func testGoldenVectorsRoundTrip() throws {
        struct Golden: Decodable {
            let chunk_size: Int
            let frame_id: UInt8
            let message: String
            let frames: [String]
        }
        let url = try XCTUnwrap(
            Bundle(for: Self.self).url(forResource: "frames-sample", withExtension: "json"),
            "골든 벡터가 테스트 번들에 없다"
        )
        let golden = try JSONDecoder().decode(Golden.self, from: Data(contentsOf: url))

        var r = FrameReassembler()
        var out: Data?
        for hex in golden.frames {
            var bytes = [UInt8]()
            var idx = hex.startIndex
            while idx < hex.endIndex {
                let next = hex.index(idx, offsetBy: 2)
                bytes.append(UInt8(hex[idx..<next], radix: 16)!)
                idx = next
            }
            XCTAssertLessThanOrEqual(bytes.count, golden.chunk_size, "청크가 한계를 넘었다")
            XCTAssertEqual(bytes[0], golden.frame_id)
            if let msg = r.push(Data(bytes)) { out = msg }
        }
        let decoded = try XCTUnwrap(out.flatMap { String(data: $0, encoding: .utf8) })
        XCTAssertEqual(decoded, golden.message, "Rust 가 만든 프레임을 Swift 가 복원하지 못했다")
    }
}
```

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find type 'FrameReassembler' in scope`

- [ ] **Step 4: 최소 구현을 쓴다**

`ios/Sources/BLETransport/FrameReassembler.swift`:
```swift
import Foundation

/// BLE notify 청크 재조립기.
///
/// Rust `src-tauri/src/ble/framing.rs` 의 `Reassembler` 와 **동작이 완전히 같아야 한다**.
/// 두 구현의 어긋남은 실기기에서만 드러나는 난해한 버그가 되므로
/// 골든 벡터(docs/ble-protocol/golden/frames-sample.json)로 양쪽을 묶어둔다.
///
/// 패킷 = [frame_id][chunk_idx][chunk_count][payload…]
public struct FrameReassembler {
    public static let headerLength = 3

    private var frameID: UInt8?
    private var expectedIndex: UInt8 = 0
    private var count: UInt8 = 0
    private var buffer = Data()
    /// 이 프레임을 이미 버렸는지(중간 구독·순서 이탈)
    private var aborted = false

    public init() {}

    /// 완성된 메시지를 만들면 반환한다.
    public mutating func push(_ packet: Data) -> Data? {
        guard packet.count >= Self.headerLength else { return nil }
        let bytes = [UInt8](packet)
        let (id, idx, total) = (bytes[0], bytes[1], bytes[2])
        guard total > 0 else { return nil }

        // 새 frame_id 이거나 0번 청크면 새로 시작한다.
        if frameID != id || idx == 0 {
            guard idx == 0 else {
                // 0번을 못 본 프레임은 통째로 버린다.
                frameID = id
                aborted = true
                return nil
            }
            frameID = id
            expectedIndex = 0
            count = total
            buffer.removeAll(keepingCapacity: true)
            aborted = false
        }

        guard !aborted, idx == expectedIndex, total == count else {
            aborted = true
            return nil
        }

        buffer.append(contentsOf: bytes[Self.headerLength...])
        expectedIndex &+= 1

        if expectedIndex == count {
            let done = buffer
            buffer = Data()
            frameID = nil
            return done
        }
        return nil
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: `** TEST SUCCEEDED **` — 7 tests

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 프레임 재조립기와 골든 벡터 교차 테스트 추가"
```

---

## Task 11: BLEClient와 연결 상태 기계

**Files:**
- Create: `ios/Sources/BLETransport/MirrorUUIDs.swift`
- Create: `ios/Sources/BLETransport/ConnectionState.swift`
- Create: `ios/Sources/BLETransport/BLEClient.swift`
- Create: `ios/Tests/BLETransportTests/ConnectionStateTests.swift`

**Interfaces:**
- Consumes: `Wire.MirrorSnapshot`, `FrameReassembler`
- Produces: `MirrorUUIDs.service/info/auth/snapshot/triggers`, `ConnectionState`, `BLEClient.init()`, `BLEClient.start()`, `BLEClient.stop()`, `BLEClient.state: AnyPublisher<ConnectionState, Never>`, `BLEClient.snapshots: AnyPublisher<MirrorSnapshot, Never>`, `BLEClient.rawMessages: AnyPublisher<String, Never>`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`ios/Tests/BLETransportTests/ConnectionStateTests.swift`:
```swift
import XCTest
import CoreBluetooth
@testable import BLETransport

final class ConnectionStateTests: XCTestCase {

    func testUUIDsMatchSpec() {
        XCTAssertEqual(MirrorUUIDs.service, CBUUID(string: "07A98A35-16C7-4BBA-A296-E28B78B7E683"))
        XCTAssertEqual(MirrorUUIDs.info, CBUUID(string: "F494FC3B-ED50-4561-AADE-1A310C5732E6"))
        XCTAssertEqual(MirrorUUIDs.auth, CBUUID(string: "1403603A-4C78-4899-A2B8-FDA198101900"))
        XCTAssertEqual(MirrorUUIDs.snapshot, CBUUID(string: "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5"))
        XCTAssertEqual(MirrorUUIDs.triggers, CBUUID(string: "4F60A8C2-F181-4717-AEE3-07C4D7846597"))
    }

    func testStateDescriptionsAreUserFacing() {
        XCTAssertEqual(ConnectionState.idle.label, "대기 중")
        XCTAssertEqual(ConnectionState.scanning.label, "Mac 찾는 중…")
        XCTAssertEqual(ConnectionState.streaming.label, "연결됨")
        XCTAssertEqual(
            ConnectionState.disconnected(reason: "범위 이탈").label,
            "연결 끊김 · 범위 이탈",
            "사유가 화면에 그대로 드러나야 원인이 미궁이 되지 않는다"
        )
    }

    func testBluetoothOffIsDistinctFromDisconnected() {
        XCTAssertEqual(ConnectionState.bluetoothOff.label, "블루투스가 꺼져 있습니다")
    }
}
```

- [ ] **Step 2: 테스트가 실패하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: FAIL — `cannot find 'MirrorUUIDs' in scope`

- [ ] **Step 3: UUID와 상태 타입을 구현한다**

`ios/Sources/BLETransport/MirrorUUIDs.swift`:
```swift
import CoreBluetooth

/// 스펙 4.1 의 값과 반드시 일치해야 한다. Rust `ble/peripheral.rs` 의 상수와 같은 값이다.
public enum MirrorUUIDs {
    public static let service  = CBUUID(string: "07A98A35-16C7-4BBA-A296-E28B78B7E683")
    public static let info     = CBUUID(string: "F494FC3B-ED50-4561-AADE-1A310C5732E6")
    public static let auth     = CBUUID(string: "1403603A-4C78-4899-A2B8-FDA198101900")
    public static let snapshot = CBUUID(string: "0AE789AA-EF38-4A35-9E72-A7CD7AD995D5")
    public static let triggers = CBUUID(string: "4F60A8C2-F181-4717-AEE3-07C4D7846597")
}
```

`ios/Sources/BLETransport/ConnectionState.swift`:
```swift
import Foundation

/// 스펙 7.3. 화면 상단에 항상 노출해 "왜 안 뜨는지" 가 미궁이 되지 않게 한다.
public enum ConnectionState: Equatable, Sendable {
    case idle
    case bluetoothOff
    case scanning
    case connecting
    case streaming
    case disconnected(reason: String)

    public var label: String {
        switch self {
        case .idle: return "대기 중"
        case .bluetoothOff: return "블루투스가 꺼져 있습니다"
        case .scanning: return "Mac 찾는 중…"
        case .connecting: return "연결 중…"
        case .streaming: return "연결됨"
        case .disconnected(let reason): return "연결 끊김 · \(reason)"
        }
    }
}
```

- [ ] **Step 4: BLEClient를 구현한다**

`ios/Sources/BLETransport/BLEClient.swift`:
```swift
import Combine
import CoreBluetooth
import Foundation
import Wire

/// CBCentralManager 래퍼. 서비스 UUID 로 스캔 → 연결 → Snapshot 특성 구독 → 청크 재조립.
///
/// CoreBluetooth 콜백이 메인 큐로 오도록 만들고 클래스 전체를 @MainActor 로 고정한다.
/// Swift 6 엄격 동시성에서 델리게이트 상태 접근을 안전하게 만드는 가장 단순한 방법이다.
@MainActor
public final class BLEClient: NSObject {
    private var central: CBCentralManager?
    private var peripheral: CBPeripheral?
    private var reassembler = FrameReassembler()

    private let stateSubject = CurrentValueSubject<ConnectionState, Never>(.idle)
    private let snapshotSubject = PassthroughSubject<MirrorSnapshot, Never>()
    private let rawSubject = PassthroughSubject<String, Never>()

    public var state: AnyPublisher<ConnectionState, Never> { stateSubject.eraseToAnyPublisher() }
    public var snapshots: AnyPublisher<MirrorSnapshot, Never> { snapshotSubject.eraseToAnyPublisher() }
    /// 1단계 확인용 원본 JSON 스트림
    public var rawMessages: AnyPublisher<String, Never> { rawSubject.eraseToAnyPublisher() }

    public override init() {
        super.init()
    }

    public func start() {
        if central == nil {
            central = CBCentralManager(delegate: self, queue: .main)
        } else {
            beginScan()
        }
    }

    public func stop() {
        central?.stopScan()
        if let p = peripheral {
            central?.cancelPeripheralConnection(p)
        }
        peripheral = nil
        stateSubject.send(.idle)
    }

    private func beginScan() {
        guard let central, central.state == .poweredOn else { return }
        reassembler = FrameReassembler()
        stateSubject.send(.scanning)
        central.scanForPeripherals(withServices: [MirrorUUIDs.service])
    }
}

extension BLEClient: CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn: beginScan()
        case .poweredOff: stateSubject.send(.bluetoothOff)
        case .unauthorized: stateSubject.send(.disconnected(reason: "블루투스 권한 거부됨"))
        default: stateSubject.send(.disconnected(reason: "블루투스 사용 불가"))
        }
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self
        stateSubject.send(.connecting)
        central.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.discoverServices([MirrorUUIDs.service])
    }

    public func centralManager(
        _ central: CBCentralManager,
        didDisconnectPeripheral peripheral: CBPeripheral,
        error: Error?
    ) {
        self.peripheral = nil
        stateSubject.send(.disconnected(reason: error?.localizedDescription ?? "Mac 연결 종료"))
        beginScan()
    }

    public func centralManager(
        _ central: CBCentralManager,
        didFailToConnect peripheral: CBPeripheral,
        error: Error?
    ) {
        stateSubject.send(.disconnected(reason: error?.localizedDescription ?? "연결 실패"))
        beginScan()
    }
}

extension BLEClient: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let service = peripheral.services?.first(where: { $0.uuid == MirrorUUIDs.service }) else {
            stateSubject.send(.disconnected(reason: "미러 서비스를 찾지 못함"))
            return
        }
        peripheral.discoverCharacteristics([MirrorUUIDs.snapshot], for: service)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        guard let ch = service.characteristics?.first(where: { $0.uuid == MirrorUUIDs.snapshot }) else {
            stateSubject.send(.disconnected(reason: "Snapshot 특성을 찾지 못함"))
            return
        }
        peripheral.setNotifyValue(true, for: ch)
        stateSubject.send(.streaming)
    }

    public func peripheral(
        _ peripheral: CBPeripheral,
        didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard characteristic.uuid == MirrorUUIDs.snapshot, let data = characteristic.value else { return }
        guard let message = reassembler.push(data) else { return }

        if let text = String(data: message, encoding: .utf8) {
            rawSubject.send(text)
        }
        do {
            let snap = try JSONDecoder().decode(MirrorSnapshot.self, from: message)
            guard snap.isSupportedVersion else {
                stateSubject.send(.disconnected(reason: "프로토콜 버전 불일치 · 앱 업데이트 필요"))
                return
            }
            snapshotSubject.send(snap)
        } catch {
            // 디코딩 실패는 연결을 끊을 사유가 아니다. 다음 프레임에서 회복될 수 있다.
            NSLog("스냅샷 디코딩 실패: \(error)")
        }
    }
}
```

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run:
```bash
cd ios && tuist generate --no-open && xcodebuild test -workspace AIAgentMonitorMirror.xcworkspace -scheme BLETransportTests -destination 'platform=iOS Simulator,name=iPhone 16,OS=18.5' 2>&1 | tail -20
```
Expected: `** TEST SUCCEEDED **` — 10 tests

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): BLE 클라이언트와 연결 상태 기계 추가"
```

---

## Task 12: 앱 타깃과 원본 JSON 확인 화면 · 실기기 검증

1단계의 완료 판정: **실기기에서 스냅샷 JSON이 1Hz로 흐른다.**

**Files:**
- Create: `ios/Sources/App/AppDelegate.swift`, `ios/Sources/App/SceneDelegate.swift`, `ios/Sources/App/RawDumpViewController.swift`
- Modify: `ios/Project.swift`

**Interfaces:**
- Consumes: `BLETransport.BLEClient`, `BLETransport.ConnectionState`
- Produces: 실행 가능한 `App` 타깃

- [ ] **Step 1: Project.swift 에 앱 타깃을 추가한다**

`ios/Project.swift` 의 `targets:` 배열 끝에 추가:
```swift
        .target(
            name: "App",
            destinations: .iOS,
            product: .app,
            bundleId: bundlePrefix,
            deploymentTargets: iOS,
            infoPlist: .extendingDefault(with: [
                "UILaunchScreen": [:],
                "NSBluetoothAlwaysUsageDescription":
                    "Mac 의 AI Agent Monitor 와 연결해 모니터링 화면을 표시합니다.",
                "UIApplicationSceneManifest": [
                    "UIApplicationSupportsMultipleScenes": false,
                    "UISceneConfigurations": [
                        "UIWindowSceneSessionRoleApplication": [[
                            "UISceneConfigurationName": "Default",
                            "UISceneDelegateClassName": "$(PRODUCT_MODULE_NAME).SceneDelegate",
                        ]]
                    ],
                ],
            ]),
            sources: ["Sources/App/**"],
            dependencies: [
                .target(name: "BLETransport"),
                .external(name: "SnapKit"),
            ]
        ),
```

`NSBluetoothAlwaysUsageDescription` 이 없으면 **iOS 가 첫 스캔에서 앱을 종료시킨다**.

- [ ] **Step 2: 앱 진입점을 만든다**

`ios/Sources/App/AppDelegate.swift`:
```swift
import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        configurationForConnecting session: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        UISceneConfiguration(name: "Default", sessionRole: session.role)
    }
}
```

`ios/Sources/App/SceneDelegate.swift`:
```swift
import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }
        let w = UIWindow(windowScene: windowScene)
        w.rootViewController = UINavigationController(rootViewController: RawDumpViewController())
        w.makeKeyAndVisible()
        window = w
    }
}
```

- [ ] **Step 3: 원본 JSON 확인 화면을 만든다**

`ios/Sources/App/RawDumpViewController.swift`:
```swift
import BLETransport
import Combine
import SnapKit
import UIKit

/// 1단계 확인용 화면. 2단계에서 실제 미러 UI 로 교체된다.
/// 목적은 단 하나 — 실기기에서 스냅샷 JSON 이 실제로 흐르는지 눈으로 보는 것.
final class RawDumpViewController: UIViewController {
    private let client = BLEClient()
    private var cancellables = Set<AnyCancellable>()
    private var received = 0

    private let statusLabel = UILabel()
    private let counterLabel = UILabel()
    private let textView = UITextView()

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "BLE 미러 (raw)"
        view.backgroundColor = .systemBackground

        statusLabel.font = .preferredFont(forTextStyle: .headline)
        statusLabel.text = ConnectionState.idle.label
        counterLabel.font = .preferredFont(forTextStyle: .caption1)
        counterLabel.textColor = .secondaryLabel
        counterLabel.text = "수신 0건"

        textView.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        textView.isEditable = false
        textView.backgroundColor = .secondarySystemBackground
        textView.layer.cornerRadius = 8

        [statusLabel, counterLabel, textView].forEach(view.addSubview)

        statusLabel.snp.makeConstraints { make in
            make.top.equalTo(view.safeAreaLayoutGuide).offset(16)
            make.leading.trailing.equalToSuperview().inset(16)
        }
        counterLabel.snp.makeConstraints { make in
            make.top.equalTo(statusLabel.snp.bottom).offset(4)
            make.leading.trailing.equalTo(statusLabel)
        }
        textView.snp.makeConstraints { make in
            make.top.equalTo(counterLabel.snp.bottom).offset(12)
            make.leading.trailing.equalToSuperview().inset(16)
            make.bottom.equalTo(view.safeAreaLayoutGuide).offset(-16)
        }

        client.state
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.statusLabel.text = $0.label }
            .store(in: &cancellables)

        client.rawMessages
            .receive(on: DispatchQueue.main)
            .sink { [weak self] json in
                guard let self else { return }
                self.received += 1
                self.counterLabel.text = "수신 \(self.received)건 · \(json.utf8.count) bytes"
                self.textView.text = json
            }
            .store(in: &cancellables)

        client.start()
    }
}
```

- [ ] **Step 4: 빌드한다**

Run:
```bash
cd ios && tuist install && tuist generate --no-open
cd ios && xcodebuild build -workspace AIAgentMonitorMirror.xcworkspace -scheme App -destination 'generic/platform=iOS' 2>&1 | tail -20
```
Expected: `** BUILD SUCCEEDED **`

- [ ] **Step 5: 실기기에서 검증한다 (시뮬레이터 불가)**

**BLE는 시뮬레이터에서 동작하지 않는다.** iPhone을 Mac에 연결한 뒤:

1. Mac에서 `pnpm tauri dev` 로 앱을 실행하고 Detail → **Devices 탭에서 BLE 공유를 켠다**
2. Xcode 에서 `ios/AIAgentMonitorMirror.xcworkspace` 를 열고 `App` 스킴 · 연결된 iPhone 을 선택해 실행
   - 무료 개발자 계정이면 서명 팀을 지정해야 하며 7일마다 재설치가 필요하다
3. iPhone 에서 블루투스 권한을 허용한다

확인 항목:
- [ ] 상태 라벨이 `Mac 찾는 중…` → `연결 중…` → `연결됨` 순으로 바뀐다
- [ ] Mac의 Devices 탭에 연결된 기기가 하나 나타나고 MTU가 표시된다 (**보통 180~185**)
- [ ] 화면에 `{"v":1,"t":…}` 로 시작하는 JSON이 표시된다
- [ ] 수신 카운터가 **초당 약 1회** 증가한다 (Mac에서 Claude Code를 돌려 토큰이 변할 때)
- [ ] Mac에서 토글을 끄면 상태가 `연결 끊김` 으로 바뀐다
- [ ] iPhone을 들고 멀어졌다가 돌아오면 자동으로 재연결된다

- [ ] **Step 6: 커밋한다**

```bash
git add ios/
git commit -m "feat(ios): 앱 타깃과 원본 JSON 확인 화면 추가"
```

---

## Self-Review

**스펙 커버리지 (1단계 범위)**

| 스펙 항목 | 태스크 |
|---|---|
| 4.1 UUID 확정 | Task 4 (Rust 상수 + 테스트), Task 11 (Swift 상수 + 테스트) |
| 4.2 프레이밍 | Task 2 (Rust), Task 10 (Swift) |
| 4.3 페이로드 DTO | Task 1 (Rust), Task 9 (Swift) |
| 4.4 전송률 제어 | Task 5 (`EmitGate` 1Hz 재사용, 구독자 0이면 직렬화 생략) |
| 4.5 백프레셔 | Task 3 (`SendQueue`), Task 6 (`isReadyToUpdateSubscribers` 연결) |
| 5.3 macOS 권한 | Task 6 (`Info.plist`) |
| 6. Mac 구성 · 명령 | Task 6, Task 7 |
| 6. Devices 탭 (토글만) | Task 8 |
| 7.1 Tuist 모듈 그래프 | Task 9 (`Wire`), Task 10~11 (`BLETransport`), Task 12 (`App`) |
| 7.2 Combine 상태 전파 | Task 11 |
| 7.3 연결 상태 기계 | Task 11 |
| 8. 교차 검증 골든 벡터 | Task 2·9 (생성), Task 9·10 (Swift 검증) |
| 2. watch 채널 분리 | **Task 7 에서 직접 호출로 단순화** — 아래 참조 |

**의도한 스펙 이탈 1건**: 스펙 2장은 `tokio::sync::watch` 로 분기하도록 그렸으나, Task 7 은 틱 루프에서 `BleBridge::on_snapshot` 을 직접 호출한다. `on_snapshot` 은 꺼져 있거나 구독자가 없으면 즉시 반환하고, 실제 전송은 `offer_frame` 이 `run_on_main_thread` 로 fire-and-forget 하므로 **틱 루프를 블로킹하지 않는다**. watch 채널을 추가하면 태스크와 채널이 하나 더 늘 뿐 얻는 게 없다(YAGNI). 3단계에서 BLE 처리가 무거워지면 그때 도입한다.

**DesignSystem·MirrorFeature·Triggers 특성·페어링**은 2·3단계 범위이며 이 계획에 없다 — 의도한 것이다.

**타입 일관성 확인**
- `MirrorSnapshot{v,t,a}` · `MirrorAgent{k,r,t5,p5,r5,pw,rw,pj}` · `MirrorProject{id,n,m,r,t,s}` — Task 1(Rust)과 Task 9(Swift)에서 동일
- `chunk(frame_id, payload, max_chunk)` — Task 2 정의, Task 5 사용 시그니처 일치
- `SendQueue::{offer, pump, on_ready, is_idle}` — Task 3 정의, Task 6 사용 일치
- `CharId`·`Subscriber`·`CentralId` — Task 4 정의, Task 5·6·7 사용 일치
- `HEADER_LEN = 3` (Rust) ≡ `FrameReassembler.headerLength = 3` (Swift)
- `BleBridge::{new, on_snapshot, set_enabled, is_enabled}` — Task 5 정의, Task 7 사용 일치

---

## Execution Handoff

계획 완료. 실행 방식 두 가지 중 선택한다.

1. **Subagent-Driven (권장)** — 태스크마다 새 서브에이전트를 붙이고 사이사이 리뷰. 컨텍스트가 깨끗하게 유지되고 반복이 빠르다.
2. **Inline Execution** — 이 세션에서 체크포인트를 두고 배치 실행.
