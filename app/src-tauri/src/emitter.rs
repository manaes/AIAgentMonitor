use crate::types::Snapshot;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

pub struct EmitGate {
    last_hash: Option<u64>,
    last_emit_at: Option<SystemTime>,
    throttle: Duration,
}

impl EmitGate {
    pub fn new(throttle: Duration) -> Self {
        Self { last_hash: None, last_emit_at: None, throttle }
    }

    pub fn should_emit(&mut self, snap: &Snapshot, now: SystemTime) -> bool {
        let h = hash_snapshot(snap);
        let unchanged = self.last_hash == Some(h);
        if let Some(last) = self.last_emit_at {
            let elapsed = now.duration_since(last).unwrap_or_default();
            if elapsed < self.throttle {
                return false;
            }
            if unchanged && elapsed < Duration::from_secs(5) {
                return false;
            }
        }
        self.last_hash = Some(h);
        self.last_emit_at = Some(now);
        true
    }

    /// 송출이 실패했을 때 게이트를 되돌린다.
    /// 내용 해시가 같으면 unchanged 로 영구 억제되므로, 실제로 나가지 못한 프레임은
    /// 반드시 이걸 호출해 다음 틱에서 다시 시도되게 해야 한다.
    pub fn reset(&mut self) {
        self.last_hash = None;
    }
}

fn hash_snapshot(s: &Snapshot) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    s.agents.len().hash(&mut h);
    for a in &s.agents {
        format!("{:?}", a.kind).hash(&mut h);
        a.rate_tok_per_sec.to_bits().hash(&mut h);
        a.tokens_5h.total().hash(&mut h);
        a.quota_limit.hash(&mut h);
        a.quota_used_pct.map(|p| p.to_bits()).hash(&mut h);
        a.quota_used_pct_weekly.map(|p| p.to_bits()).hash(&mut h);
        // 사용량 조회가 막 실패했다는 사실도 "바뀐 것"이다. 여기 빠뜨리면 %가
        // 그대로인 동안 에러 배지가 최대 5초(unchanged 억제 창)까지 늦게 뜬다.
        a.quota_error.hash(&mut h);
        a.projects.len().hash(&mut h);
        for p in &a.projects {
            p.path.hash(&mut h);
            p.rate_tok_per_sec.to_bits().hash(&mut h);
            format!("{:?}", p.status).hash(&mut h);
            p.model.hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, AgentState, Snapshot, TokenCounts};
    use std::time::{Duration, SystemTime};

    fn agent(rate: f32) -> AgentState {
        AgentState {
            kind: AgentKind::Claude, rate_tok_per_sec: rate,
            tokens_5h: TokenCounts::default(), quota_limit: None,
            quota_reset_at: None, quota_used_pct: None,
            quota_reset_at_weekly: None, quota_used_pct_weekly: None,
            quota_error: None,
            projects: vec![],
        }
    }

    #[test]
    fn first_snapshot_is_always_emitted() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let snap = Snapshot { emitted_at: SystemTime::now(), agents: vec![agent(0.0)] };
        assert!(e.should_emit(&snap, snap.emitted_at));
    }

    #[test]
    fn identical_snapshot_is_suppressed() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let snap = Snapshot { emitted_at: now, agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap, now));
        let snap2 = Snapshot { emitted_at: now + Duration::from_millis(100), agents: vec![agent(1.0)] };
        assert!(!e.should_emit(&snap2, now + Duration::from_millis(100)));
    }

    #[test]
    fn changed_snapshot_within_throttle_is_suppressed() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let snap1 = Snapshot { emitted_at: now, agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap1, now));
        let snap2 = Snapshot { emitted_at: now + Duration::from_millis(200), agents: vec![agent(2.0)] };
        assert!(!e.should_emit(&snap2, now + Duration::from_millis(200)));
    }

    #[test]
    fn changed_snapshot_after_throttle_emits() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let snap1 = Snapshot { emitted_at: now, agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap1, now));
        let snap2 = Snapshot { emitted_at: now + Duration::from_millis(600), agents: vec![agent(2.0)] };
        assert!(e.should_emit(&snap2, now + Duration::from_millis(600)));
    }

    /// %는 그대로인데 조회만 실패한 순간 — 배지를 곧바로 띄우려면 이게 변화로
    /// 잡혀야 한다(안 잡히면 unchanged 로 최대 5초 억제된다).
    #[test]
    fn quota_error_appearing_counts_as_a_change() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let snap1 = Snapshot { emitted_at: now, agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap1, now));

        let mut failing = agent(1.0);
        failing.quota_error = Some("Codex 로그인 필요".to_string());
        let later = now + Duration::from_millis(600);
        let snap2 = Snapshot { emitted_at: later, agents: vec![failing] };
        assert!(e.should_emit(&snap2, later));
    }

    #[test]
    fn reset_allows_identical_snapshot_to_emit_again() {
        let mut e = EmitGate::new(Duration::from_millis(500));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let snap = Snapshot { emitted_at: now, agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap, now));

        e.reset();

        // 내용은 동일하지만 리셋했으므로, 스로틀 시간만 지나면 다시 나가야 한다.
        let snap2 = Snapshot { emitted_at: now + Duration::from_millis(600), agents: vec![agent(1.0)] };
        assert!(e.should_emit(&snap2, now + Duration::from_millis(600)));
    }
}
