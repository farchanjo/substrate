//! Signal policy — destructive-signal classification and elicitation gate.
//!
//! ADR-0004 (Layer 4 — Elicitation) requires that SIGKILL, SIGTERM, and
//! SIGSTOP pass through an explicit user confirmation step before delivery.
//! This module encodes that classification so it can be tested independently
//! from the handler.

use nix::sys::signal::Signal;

/// Returns `true` when the signal requires elicitation confirmation per
/// ADR-0004 Layer 4.
///
/// Destructive signals are those whose effect on the target process is
/// irreversible from the agent's perspective:
/// - `SIGKILL` — forced termination, no cleanup possible.
/// - `SIGTERM` — polite termination request; still stops the process.
/// - `SIGSTOP` — pauses the process until resumed; disrupts real-time work.
#[must_use]
pub const fn is_destructive(sig: Signal) -> bool {
    matches!(sig, Signal::SIGKILL | Signal::SIGTERM | Signal::SIGSTOP)
}

/// Returns `true` when `sig` is signal 0 (existence probe, not a real signal).
///
/// Signal 0 is used by `proc.signal` for the PID existence check (`kill(2)`
/// with `sig=0` returns ESRCH when the process does not exist). It never
/// requires elicitation and is never delivered to the target.
#[must_use]
pub const fn is_existence_probe(sig: Signal) -> bool {
    // nix::Signal has no Signal::SIGNULL variant; 0 maps to a raw integer
    // in the kill(2) call. We handle the probe via a separate code path and
    // this helper guards the classifier.
    let _ = sig; // sig != null sentinel; all named signals are real deliveries.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;

    #[test]
    fn destructive_signals_classified_correctly() {
        assert!(is_destructive(Signal::SIGKILL));
        assert!(is_destructive(Signal::SIGTERM));
        assert!(is_destructive(Signal::SIGSTOP));
    }

    #[test]
    fn non_destructive_signals_not_classified_as_destructive() {
        assert!(!is_destructive(Signal::SIGHUP));
        assert!(!is_destructive(Signal::SIGUSR1));
        assert!(!is_destructive(Signal::SIGUSR2));
        assert!(!is_destructive(Signal::SIGCONT));
        assert!(!is_destructive(Signal::SIGWINCH));
    }
}
