use crate::clock::Clock;
use crate::types::TokenEvent;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

const RETENTION: Duration = Duration::from_secs(300);
const WINDOW_SECS: f32 = 10.0;
const ALPHA: f32 = 0.3;

pub struct EventRing {
    events: VecDeque<TokenEvent>,
    ema_rate: f32,
}

impl EventRing {
    pub fn new() -> Self {
        Self { events: VecDeque::new(), ema_rate: 0.0 }
    }

    pub fn push(&mut self, ev: TokenEvent) {
        let now = ev.ts;
        self.events.push_back(ev);
        self.prune(now);
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize { self.events.len() }

    #[allow(dead_code)]
    pub fn events(&self) -> impl Iterator<Item = &TokenEvent> { self.events.iter() }

    pub fn rate_tok_per_sec<C: Clock>(&mut self, clock: &C) -> f32 {
        let now = clock.now();
        self.prune(now);
        let window_start = now.checked_sub(Duration::from_secs_f32(WINDOW_SECS))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut sum: u64 = 0;
        for e in &self.events {
            if e.ts >= window_start {
                sum = sum.saturating_add(e.counts.total() as u64);
            }
        }
        let raw = sum as f32 / WINDOW_SECS;
        self.ema_rate = ALPHA * raw + (1.0 - ALPHA) * self.ema_rate;
        self.ema_rate
    }

    fn prune(&mut self, now: SystemTime) {
        let cutoff = now.checked_sub(RETENTION).unwrap_or(SystemTime::UNIX_EPOCH);
        while let Some(front) = self.events.front() {
            if front.ts < cutoff { self.events.pop_front(); } else { break; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::mock::MockClock;
    use crate::types::{AgentKind, TokenCounts, TokenEvent};
    use std::path::PathBuf;
    use std::time::Duration;

    fn ev(clock: &MockClock, total: u32) -> TokenEvent {
        TokenEvent {
            agent: AgentKind::Claude,
            project_path: PathBuf::from("/tmp/p"),
            session_id: "s".into(),
            model: "claude".into(),
            ts: clock.now(),
            counts: TokenCounts { tokens_in: total, tokens_out: 0, tokens_cache_read: 0, tokens_cache_create: 0 },
            prompt_preview: None,
        }
    }

    #[test]
    fn ema_starts_at_zero_with_no_events() {
        let clock = MockClock::new(1_000_000);
        let mut ring = EventRing::new();
        assert_eq!(ring.rate_tok_per_sec(&clock), 0.0);
    }

    #[test]
    fn ema_increases_after_burst() {
        let clock = MockClock::new(1_000_000);
        let mut ring = EventRing::new();
        for _ in 0..10 { ring.push(ev(&clock, 100)); }
        let r = ring.rate_tok_per_sec(&clock);
        assert!(r > 0.0, "rate should be positive after burst, got {r}");
    }

    #[test]
    fn ema_decays_to_zero_after_idle() {
        let clock = MockClock::new(1_000_000);
        let mut ring = EventRing::new();
        ring.push(ev(&clock, 1000));
        let _ = ring.rate_tok_per_sec(&clock);
        for _ in 0..30 {
            clock.advance(Duration::from_secs(1));
            ring.rate_tok_per_sec(&clock);
        }
        let r = ring.rate_tok_per_sec(&clock);
        assert!(r < 1.0, "rate should decay near zero, got {r}");
    }

    #[test]
    fn ring_drops_events_older_than_5min() {
        let clock = MockClock::new(1_000_000);
        let mut ring = EventRing::new();
        ring.push(ev(&clock, 100));
        clock.advance(Duration::from_secs(310));
        ring.push(ev(&clock, 50));
        assert_eq!(ring.len(), 1);
    }
}
