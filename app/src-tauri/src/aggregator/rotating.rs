use crate::clock::Clock;
use crate::types::TokenCounts;
use std::time::{Duration, SystemTime};

const BUCKET: Duration = Duration::from_secs(300);  // 5분
const NUM_BUCKETS: usize = 60;                       // 60 * 5min = 5h

pub struct RotatingBucket {
    cells: [(SystemTime, TokenCounts); NUM_BUCKETS],
}

impl RotatingBucket {
    pub fn new() -> Self {
        Self {
            cells: std::array::from_fn(|_| (SystemTime::UNIX_EPOCH, TokenCounts::default())),
        }
    }

    fn slot(ts: SystemTime) -> usize {
        let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        ((secs / BUCKET.as_secs()) as usize) % NUM_BUCKETS
    }

    fn bucket_start(ts: SystemTime) -> SystemTime {
        let secs = ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
        SystemTime::UNIX_EPOCH + Duration::from_secs((secs / BUCKET.as_secs()) * BUCKET.as_secs())
    }

    pub fn add(&mut self, ts: SystemTime, counts: &TokenCounts) {
        let bs = Self::bucket_start(ts);
        let idx = Self::slot(ts);
        let cell = &mut self.cells[idx];
        if cell.0 != bs {
            cell.0 = bs;
            cell.1 = TokenCounts::default();
        }
        cell.1.add(counts);
    }

    pub fn sum_5h<C: Clock>(&self, clock: &C) -> TokenCounts {
        let now = clock.now();
        let cutoff = now.checked_sub(Duration::from_secs(5 * 3600)).unwrap_or(SystemTime::UNIX_EPOCH);
        let mut total = TokenCounts::default();
        for (bs, c) in &self.cells {
            if *bs >= cutoff && *bs <= now {
                total.add(c);
            }
        }
        total
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::mock::MockClock;
    use crate::types::TokenCounts;
    use std::time::Duration;

    #[test]
    fn empty_bucket_returns_zero() {
        let clock = MockClock::new(1_000_000);
        let rb = RotatingBucket::new();
        let total = rb.sum_5h(&clock);
        assert_eq!(total.total(), 0);
    }

    #[test]
    fn sums_within_5h() {
        let clock = MockClock::new(1_000_000);
        let mut rb = RotatingBucket::new();
        let c = TokenCounts { tokens_in: 100, tokens_out: 50, tokens_cache_read: 0, tokens_cache_create: 0 };
        rb.add(clock.now(), &c);
        clock.advance(Duration::from_secs(60));
        rb.add(clock.now(), &c);
        let total = rb.sum_5h(&clock);
        assert_eq!(total.tokens_in, 200);
        assert_eq!(total.tokens_out, 100);
    }

    #[test]
    fn drops_after_5h_plus_1s() {
        let clock = MockClock::new(1_000_000);
        let mut rb = RotatingBucket::new();
        let c = TokenCounts { tokens_in: 100, ..Default::default() };
        rb.add(clock.now(), &c);
        clock.advance(Duration::from_secs(5 * 3600 + 1));
        assert_eq!(rb.sum_5h(&clock).tokens_in, 0);
    }

}
