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
        if unchanged { return false; }
        if let Some(last) = self.last_emit_at {
            let elapsed = now.duration_since(last).unwrap_or_default();
            if elapsed < self.throttle { return false; }
        }
        self.last_hash = Some(h);
        self.last_emit_at = Some(now);
        true
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
            projects: vec![], triggered_by: None,
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
}
