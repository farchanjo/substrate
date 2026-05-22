//! Atomic quota enforcement helpers for the async job control-plane.
//!
//! Provides check-and-increment + rollback utilities for:
//! - Global inflight counter (`jobs.max_concurrent`)
//! - Per-client inflight counter (`jobs.max_per_client`)
//!
//! Both operations are optimistic: the counter is incremented speculatively and
//! rolled back if another quota is subsequently violated. This avoids the need
//! for a combined lock across two separate atomic integers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use substrate_domain::errors::{SubstrateError, SubstrateResult};

/// Attempts to increment `counter` up to `max`.
///
/// Returns `Ok(())` if the counter was below `max` and was successfully
/// incremented. Returns `SubstrateError::QuotaExceeded` otherwise, with no
/// change to the counter.
///
/// Uses `compare_exchange_weak` in a CAS loop for ABA-safety.
pub(crate) fn try_increment(
    counter: &AtomicUsize,
    max: usize,
    detail: &str,
) -> SubstrateResult<()> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        if current >= max {
            return Err(SubstrateError::QuotaExceeded {
                detail: detail.to_owned(),
                correlation_id: None,
            });
        }
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(actual) => current = actual,
        }
    }
}

/// Decrements `counter` by one, saturating at zero.
///
/// Call this whenever a job leaves the active set (terminal state reached or
/// quota rollback after a failed submission).
pub(crate) fn decrement(counter: &AtomicUsize) {
    // `fetch_sub` with saturation — we never want a usize underflow.
    // The wrapping case is pathological (bug elsewhere) but we guard it anyway.
    let prev = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_sub(1))
    });
    // `fetch_update` always returns `Ok` when the closure returns `Some`.
    let _ = prev;
}

/// Scoped quota reservation that rolls back automatically on drop.
///
/// Acquire with [`QuotaGuard::try_acquire`]; call `.commit()` to disarm the
/// rollback when the job is fully registered and will decrement the counter
/// itself at terminal state.
pub(crate) struct QuotaGuard {
    counter: Arc<AtomicUsize>,
    committed: bool,
}

impl QuotaGuard {
    /// Attempts to acquire the quota guard, incrementing `counter` up to `max`.
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::QuotaExceeded`] when the counter is already at `max`.
    pub(crate) fn try_acquire(
        counter: Arc<AtomicUsize>,
        max: usize,
        detail: &str,
    ) -> SubstrateResult<Self> {
        try_increment(&counter, max, detail)?;
        Ok(Self {
            counter,
            committed: false,
        })
    }

    /// Disarms the automatic rollback.
    ///
    /// Call this after the job has been successfully inserted into the registry.
    /// The job itself is then responsible for calling [`decrement`] when it reaches
    /// a terminal state.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for QuotaGuard {
    fn drop(&mut self) {
        if !self.committed {
            decrement(&self.counter);
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test code: panicking assertions are idiomatic in unit tests"
)]
mod tests {
    use super::*;

    #[test]
    fn try_increment_succeeds_below_max() {
        let c = AtomicUsize::new(0);
        try_increment(&c, 2, "test").expect("should succeed");
        assert_eq!(c.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn try_increment_fails_at_max() {
        let c = AtomicUsize::new(2);
        let err = try_increment(&c, 2, "test").expect_err("should fail at cap");
        assert_eq!(err.code(), "SUBSTRATE_QUOTA_EXCEEDED");
    }

    #[test]
    fn quota_guard_rollback_on_drop() {
        let c = Arc::new(AtomicUsize::new(0));
        {
            let _g = QuotaGuard::try_acquire(Arc::clone(&c), 10, "test").expect("should acquire");
            assert_eq!(c.load(Ordering::Relaxed), 1);
            // Drop without commit -> rollback.
        }
        assert_eq!(c.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn quota_guard_no_rollback_after_commit() {
        let c = Arc::new(AtomicUsize::new(0));
        let g = QuotaGuard::try_acquire(Arc::clone(&c), 10, "test").expect("should acquire");
        g.commit();
        assert_eq!(c.load(Ordering::Relaxed), 1);
    }
}
