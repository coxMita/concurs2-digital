//! Per-username failed-login limiter. Successful logins are never throttled;
//! the per-IP abuse limit is applied separately by `tower_governor`.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use moka::sync::Cache;

/// Fixed one-minute window counter.
#[derive(Clone, Copy, Debug)]
struct Window {
    started: Instant,
    failures: u32,
}

#[derive(Clone)]
pub struct LoginLimiter {
    windows: Arc<Cache<String, Window>>,
    max_failures: u32,
}

const WINDOW: Duration = Duration::from_secs(60);

impl LoginLimiter {
    pub fn new(max_failures_per_min: u32) -> Self {
        Self {
            windows: Arc::new(
                Cache::builder()
                    .max_capacity(100_000)
                    .time_to_live(WINDOW)
                    .build(),
            ),
            max_failures: max_failures_per_min,
        }
    }

    /// True if this username has exceeded its failure budget in the current window.
    pub fn is_blocked(&self, key: &str) -> bool {
        self.windows
            .get(key)
            .is_some_and(|w| w.started.elapsed() < WINDOW && w.failures >= self.max_failures)
    }

    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let entry = self.windows.get(key);
        let w = match entry {
            Some(w) if w.started.elapsed() < WINDOW => Window {
                started: w.started,
                failures: w.failures.saturating_add(1),
            },
            _ => Window {
                started: now,
                failures: 1,
            },
        };
        self.windows.insert(key.to_string(), w);
    }

    pub fn clear(&self, key: &str) {
        self.windows.invalidate(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_failures() {
        let l = LoginLimiter::new(3);
        for _ in 0..3 {
            assert!(!l.is_blocked("ana"));
            l.record_failure("ana");
        }
        assert!(l.is_blocked("ana"));
        assert!(!l.is_blocked("ion"));
        l.clear("ana");
        assert!(!l.is_blocked("ana"));
    }
}
