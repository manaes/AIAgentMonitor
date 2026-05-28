pub mod ring;
pub mod rotating;

use crate::clock::Clock;
use crate::types::{
    ActivityStatus, AgentKind, AgentState, ProjectActivity, Snapshot, TokenEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ring::EventRing;
use rotating::RotatingBucket;

#[derive(Default)]
pub struct Aggregator {
    by_agent: HashMap<AgentKind, AgentBucket>,
}

struct AgentBucket {
    ring: EventRing,
    rotating: RotatingBucket,
    projects: HashMap<PathBuf, ProjectState>,
}

struct ProjectState {
    model: String,
    last_event_at: SystemTime,
    rate_ring: EventRing,
}

impl Default for AgentBucket {
    fn default() -> Self {
        Self { ring: EventRing::new(), rotating: RotatingBucket::new(), projects: HashMap::new() }
    }
}

impl Aggregator {
    pub fn new() -> Self { Self::default() }

    pub fn push(&mut self, ev: TokenEvent) {
        let bucket = self.by_agent.entry(ev.agent).or_default();
        bucket.rotating.add(ev.ts, &ev.counts);
        let proj = bucket.projects.entry(ev.project_path.clone()).or_insert_with(|| ProjectState {
            model: ev.model.clone(),
            last_event_at: ev.ts,
            rate_ring: EventRing::new(),
        });
        proj.model = ev.model.clone();
        proj.last_event_at = ev.ts;
        proj.rate_ring.push(ev.clone());
        bucket.ring.push(ev);
    }

    pub fn snapshot<C: Clock>(&mut self, clock: &C) -> Snapshot {
        let now = clock.now();
        let mut agents = Vec::with_capacity(2);
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let bucket = self.by_agent.entry(kind).or_default();
            let rate = bucket.ring.rate_tok_per_sec(clock);
            let tokens_5h = bucket.rotating.sum_5h(clock);
            let quota_reset_at = bucket.rotating.oldest_non_empty(clock)
                .map(|t| t + Duration::from_secs(5 * 3600));

            let mut projects: Vec<ProjectActivity> = bucket.projects.iter_mut().map(|(path, ps)| {
                let elapsed = now.duration_since(ps.last_event_at).unwrap_or_default();
                let status = if elapsed <= Duration::from_secs(60) { ActivityStatus::Active }
                    else if elapsed <= Duration::from_secs(300) { ActivityStatus::Idle }
                    else { ActivityStatus::Dormant };
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                ProjectActivity {
                    path: path.clone(),
                    name,
                    model: ps.model.clone(),
                    rate_tok_per_sec: ps.rate_ring.rate_tok_per_sec(clock),
                    last_event_at: ps.last_event_at,
                    status,
                }
            }).collect();
            projects.sort_by(|a, b| b.last_event_at.cmp(&a.last_event_at));

            agents.push(AgentState {
                kind,
                rate_tok_per_sec: rate,
                tokens_5h,
                quota_limit: None,
                quota_reset_at,
                projects,
                triggered_by: None,
            });
        }
        Snapshot { emitted_at: now, agents }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::mock::MockClock;
    use crate::types::{AgentKind, TokenCounts, TokenEvent};
    use std::path::PathBuf;
    use std::time::Duration;

    fn ev(agent: AgentKind, ts: std::time::SystemTime, proj: &str, model: &str, total_in: u32) -> TokenEvent {
        TokenEvent {
            agent, ts,
            project_path: PathBuf::from(proj),
            session_id: "s1".into(),
            model: model.into(),
            counts: TokenCounts { tokens_in: total_in, ..Default::default() },
        }
    }

    #[test]
    fn empty_snapshot_has_two_agents() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        let snap = agg.snapshot(&clock);
        assert_eq!(snap.agents.len(), 2);
    }

    #[test]
    fn active_project_appears_with_correct_status() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "claude-sonnet-4-6", 500));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects.len(), 1);
        assert_eq!(claude.projects[0].status, ActivityStatus::Active);
        assert_eq!(claude.projects[0].name, "p1");
    }

    #[test]
    fn project_becomes_idle_after_60s() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(61));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects[0].status, ActivityStatus::Idle);
    }

    #[test]
    fn project_becomes_dormant_after_5min() {
        let clock = MockClock::new(1_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(301));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(claude.projects[0].status, ActivityStatus::Dormant);
    }

    #[test]
    fn quota_reset_at_estimated_from_oldest_bucket() {
        let clock = MockClock::new(1_000_000_000);
        let mut agg = Aggregator::new();
        agg.push(ev(AgentKind::Claude, clock.now(), "/tmp/p1", "x", 100));
        clock.advance(Duration::from_secs(60));
        let snap = agg.snapshot(&clock);
        let claude = snap.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        let reset = claude.quota_reset_at.expect("reset must be Some after first event");
        let now = clock.now();
        let diff = reset.duration_since(now).unwrap_or_default();
        assert!(diff <= Duration::from_secs(5 * 3600), "reset should be within 5h, got {:?}", diff);
    }
}
