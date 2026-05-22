//! Progress event emission throttle per ADR-0040.
//!
//! Events are suppressed unless at least 250 ms have elapsed since the last
//! emission OR the progress delta is at least 1 percentage point since the last
//! emission. Either condition alone suffices to allow emission.
//!
//! Internally uses two atomic integers — one for the last emission timestamp in
//! nanoseconds, one for the last emitted percentage — with `Relaxed` ordering.
//! This is intentional: the throttle is a best-effort rate limiter, not a
//! synchronisation barrier. Occasional double-emissions on concurrent writers are
//! acceptable; the sequence counter on each event allows clients to detect them.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Stateful throttle gate for a single job's progress events.
///
/// Constructed once per job slot and consulted before each `try_send` call.
/// Fields are `Relaxed` atomics because we require only approximate monotonicity,
/// not a memory ordering guarantee across threads.
#[derive(Debug)]
pub(crate) struct ProgressThrottler {
    /// Wall-clock nanoseconds of the last emitted event.
    ///
    /// Sourced from [`std::time::SystemTime`] rather than `Instant` to allow
    /// zero-cost reads in the fast path. Overflows after ~584 years — acceptable.
    last_emit_ns: AtomicU64,

    /// Percentage reported in the last emitted event (`0..=100`).
    last_pct: AtomicU8,

    /// Minimum milliseconds between consecutive emissions (default: 250).
    interval_ms: u64,

    /// Minimum percentage-point delta required to force early emission (default: 1).
    pct_threshold: u8,
}

impl ProgressThrottler {
    /// Creates a throttler with the given interval and percentage thresholds.
    ///
    /// # Parameters
    /// - `interval_ms`: minimum wall-clock gap between emissions in milliseconds.
    /// - `pct_threshold`: minimum percentage-point delta required to force emission.
    // Wave G+: wired by MCP server progress event emission
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Wave G+: wired by MCP server progress event emission"
        )
    )]
    pub(crate) const fn new(interval_ms: u64, pct_threshold: u8) -> Self {
        Self {
            last_emit_ns: AtomicU64::new(0),
            last_pct: AtomicU8::new(0),
            interval_ms,
            pct_threshold,
        }
    }

    /// Returns `true` when the event at `pct` percent SHOULD be emitted.
    ///
    /// Advances internal state if the answer is `true`. The caller MUST NOT
    /// emit the event if this returns `false`.
    ///
    /// `now_ns` is the current wall-clock time in nanoseconds, typically obtained
    /// via `SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()`.
    // Wave G+: wired by MCP server progress event emission
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Wave G+: wired by MCP server progress event emission"
        )
    )]
    pub(crate) fn should_emit(&self, now_ns: u64, pct: u8) -> bool {
        let last_ns = self.last_emit_ns.load(Ordering::Relaxed);
        let last_pct = self.last_pct.load(Ordering::Relaxed);

        let elapsed_ms = now_ns.saturating_sub(last_ns) / 1_000_000;
        let pct_delta = pct.saturating_sub(last_pct);

        let should = elapsed_ms >= self.interval_ms || pct_delta >= self.pct_threshold;
        if should {
            self.last_emit_ns.store(now_ns, Ordering::Relaxed);
            self.last_pct.store(pct, Ordering::Relaxed);
        }
        should
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_event_always_emits() {
        let t = ProgressThrottler::new(250, 1);
        assert!(t.should_emit(1_000_000_000, 0));
    }

    #[test]
    fn suppressed_within_interval_and_below_pct_threshold() {
        let t = ProgressThrottler::new(250, 1);
        // First emit at t=1 s (non-zero to avoid zero-elapsed edge case).
        assert!(t.should_emit(1_000_000_000, 0));
        // Second emit 100 ms later, same pct — should be suppressed.
        assert!(!t.should_emit(1_100_000_000, 0));
    }

    #[test]
    fn forced_by_pct_delta() {
        let t = ProgressThrottler::new(250, 1);
        // First emit at t=1 s.
        assert!(t.should_emit(1_000_000_000, 0));
        // Only 10 ms elapsed but pct jumped 5 points — should emit.
        assert!(t.should_emit(1_010_000_000, 5));
    }

    #[test]
    fn forced_by_interval() {
        let t = ProgressThrottler::new(250, 1);
        assert!(t.should_emit(0, 50));
        // 300 ms elapsed, pct unchanged — should emit due to interval.
        assert!(t.should_emit(300_000_000, 50));
    }
}
