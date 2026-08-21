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
                AgentKind::Antigravity => 2,
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
            }],
        }
    }

    /// 주간 쿼터가 채워진 스냅샷. `pw`/`rw` 는 값이 있을 때만 직렬화되므로,
    /// 값이 실린 골든 벡터가 없으면 키 이름 변경·삭제를 아무도 잡지 못한다.
    fn sample_snapshot_with_weekly() -> Snapshot {
        let mut s = sample_snapshot();
        s.agents[0].quota_used_pct_weekly = Some(41.5);
        s.agents[0].quota_reset_at_weekly = Some(UNIX_EPOCH + Duration::from_secs(1_755_900_000));
        s
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

    /// 주간 쿼터가 실린 두 번째 골든 벡터. Swift 쪽은 값이 없는 벡터만으로는
    /// `pw`/`rw` 의 이름 변경을 nil 과 구분할 수 없으므로 이 벡터가 필요하다.
    /// 갱신: UPDATE_GOLDEN=1 cargo test --manifest-path src-tauri/Cargo.toml ble::wire::tests::golden
    #[test]
    fn golden_snapshot_with_weekly_matches() {
        use std::path::PathBuf;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/ble-protocol/golden/snapshot-weekly-sample.json");
        let dto = MirrorSnapshot::from(&sample_snapshot_with_weekly());
        assert_eq!(dto.a[0].pw, Some(41.5));
        assert_eq!(dto.a[0].rw, Some(1_755_900_000));
        let actual = serde_json::to_value(&dto).unwrap();

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
        assert_eq!(actual, expected, "주간 쿼터 필드가 골든 벡터와 어긋났다");
    }
}
