//! Pre-exec hook: async-signal-safe configuration between `fork(2)` and `exec(2)`.
//!
//! The code inside the `pre_exec` closure runs in the child's address space after
//! `fork` but before `exec`. Only async-signal-safe functions are permitted inside.
//! See POSIX.1-2017 §2.4.3 for the complete list of async-signal-safe functions.
//!
//! # Safety contract (per ADR-0053 §"Pre-exec safety contract")
//!
//! Permitted inside `pre_exec`:
//! - `setsid(2)`, `prctl(2)`, `signal(2)`, `close(2)`, `dup2(2)`, `_exit(2)`.
//!
//! Forbidden inside `pre_exec`:
//! - `malloc` or any function that calls `malloc` internally.
//! - Any lock acquisition (parent-held mutexes may be in inconsistent state).
//! - Logging, format!, or any allocation.
//!
//! References: ADR-0053 §"Process Group Leadership", ADR-0053 §"Linux Death Signal".

#[allow(
    unsafe_code,
    reason = "pre_exec carve-out per ADR-0053: only async-signal-safe calls \
              (setsid, prctl, signal). The unsafe block is the POSIX-mandated \
              window between fork(2) and exec(2). No allocation, no locks, no \
              non-async-signal-safe calls are made inside the closure."
)]
/// Installs the pre-exec hook on `cmd` that configures process group leadership
/// and, on Linux only, the parent-death signal.
///
/// # What this does
///
/// 1. Calls `setsid(2)` to make the child the leader of a new session and
///    process group. After this, `child.pid == child.pgid`, which is the
///    invariant required by `killpg(pgid, sig)` in the cascade kill chain
///    (ADR-0053 §"Explicit Cleanup Chain").
///
/// 2. On Linux only: calls `prctl(PR_SET_PDEATHSIG, sig)` so that the kernel
///    automatically delivers `sig` to the child when the parent thread exits —
///    even if the parent is killed with `SIGKILL` (ADR-0053 §"Linux Death
///    Signal"). `parent_death_signal` selects `sig`: `None` preserves the
///    historical default of `SIGTERM` (every ordinary `subprocess.spawn`);
///    `Some(raw)` requests `raw` instead, which the launch BC's detached
///    supervisor uses to bind `SIGKILL` (ADR-0068 §"Cross-platform parent-death
///    binding"). An unrecognised raw signal number falls back to `SIGTERM`
///    rather than silently skipping the parent-death binding.
///
/// The `raw_i32` -> [`nix::sys::signal::Signal`] conversion happens here, before
/// the closure is installed: signal conversion is not on the async-signal-safe
/// allowlist, so the resolved [`nix::sys::signal::Signal`] is captured by value
/// into the closure rather than computed inside it.
///
/// # References
///
/// ADR-0053 §"Process Group Leadership", ADR-0053 §"Pre-exec safety contract",
/// ADR-0068 §"Cross-platform parent-death binding".
#[expect(
    clippy::disallowed_types,
    reason = "substrate-subprocess is the single authorized host of tokio::process::Command \
              per ADR-0052 §\"Supersession of ADR-0044\". The workspace clippy.toml \
              disallows this type globally; this crate is the explicit carve-out."
)]
pub fn configure_pre_exec(cmd: &mut tokio::process::Command, parent_death_signal: Option<i32>) {
    // Resolved OUTSIDE the unsafe pre_exec closure: TryFrom<i32> for Signal is
    // not on the async-signal-safe allowlist, so it must not run post-fork.
    #[cfg(target_os = "linux")]
    let death_signal: nix::sys::signal::Signal = parent_death_signal
        .and_then(|raw| nix::sys::signal::Signal::try_from(raw).ok())
        .unwrap_or(nix::sys::signal::Signal::SIGTERM);
    #[cfg(not(target_os = "linux"))]
    let _ = parent_death_signal;

    // SAFETY: The closure is installed as a `pre_exec` hook and runs in the
    // child address space after fork(2) but before exec(2). The only calls
    // made are setsid(2) and prctl(2), both listed in POSIX as async-signal-safe.
    // No heap allocation, no lock acquisition, no format! macros are used.
    // See ADR-0053 §"Pre-exec safety contract" for the full permitted-call list.
    unsafe {
        cmd.pre_exec(move || {
            // Step 1: become a new session leader + process group leader.
            // Errors here are surfaced as io::Error and abort the spawn.
            nix::unistd::setsid().map_err(std::io::Error::other)?;

            // Step 2 (Linux only): request the resolved death signal on parent
            // death. macOS uses the watchdog pipe pattern instead (see watchdog.rs).
            #[cfg(target_os = "linux")]
            nix::sys::prctl::set_pdeathsig(Some(death_signal)).map_err(std::io::Error::other)?;

            Ok(())
        });
    }
}
