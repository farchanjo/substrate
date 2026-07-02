//! Runtime capability probe per ADR-0042.
//!
//! `probe()` runs once at startup and caches the result in a process-global
//! `OnceLock<Capabilities>`. All subsequent callers receive a shared reference
//! to the same value; no re-probing occurs at runtime.
//!
//! Per ADR-0042 amendment to ADR-0032, the probe MUST complete before SIGPIPE
//! is set to `SIG_IGN` and before the tokio runtime installs SIGTERM/SIGINT handlers.
//! Because this module is invoked from `async_main`, which runs after both
//! signal-handler installs, the actual ordering in main.rs places the SIGPIPE
//! `SIG_IGN` call before the runtime starts and the async handlers inside it —
//! see `main.rs` for the exact call sequence.
//!
//! # SIMD detection
//!
//! Uses `std::is_x86_feature_detected!` and `std::arch::is_aarch64_feature_detected!`.
//! No subprocesses, no `/proc/cpuinfo` parsing (ADR-0044 No-Subprocess Policy).
//!
//! # Syscall probes (stubs)
//!
//! Real probes use the "attempt with safe minimal arguments" strategy per ADR-0042:
//! `ENOSYS` / `EOPNOTSUPP` → capability absent; any other errno → present.
//! Current stubs return `false`; production probes will be added in the adapter wave.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::sync::OnceLock;

use substrate_domain::{
    Capabilities, HashTier, JailTier, SimdTier, StatTier, WalkerTier, WatcherTier,
};

static CAPS: OnceLock<Capabilities> = OnceLock::new();

/// Returns a shared reference to the process-wide capability snapshot.
///
/// On the first call, probes the CPU and kernel for available tiers and stores
/// the result. Subsequent calls return the cached value without re-probing.
pub(crate) fn probe() -> &'static Capabilities {
    CAPS.get_or_init(detect)
}

#[expect(
    clippy::field_reassign_with_default,
    reason = "detect_linux/detect_macos take &mut Capabilities — struct literal initialization is not possible here"
)]
fn detect() -> Capabilities {
    let mut caps = Capabilities::default();
    caps.simd_tier = detect_simd_tier();

    #[cfg(target_os = "linux")]
    detect_linux(&mut caps);

    #[cfg(target_os = "macos")]
    detect_macos(&mut caps);

    caps.walker_tier = pick_walker_tier(&caps);
    caps.watcher_tier = pick_watcher_tier(&caps);
    caps.jail_tier = pick_jail_tier(&caps);
    caps.hash_tier = pick_hash_tier(&caps);
    caps.stat_tier = pick_stat_tier(&caps);
    caps
}

// ---- SIMD detection ----------------------------------------------------------

fn detect_simd_tier() -> SimdTier {
    #[cfg(target_arch = "x86_64")]
    {
        // AVX-512 requires the `simd-avx512` Cargo feature gate per ADR-0043.
        // Even when the CPU reports AVX-512F, we stay at AVX2 unless the feature
        // is explicitly opted in (to avoid frequency throttling on older steppings).
        if cfg!(feature = "simd-avx512") && std::is_x86_feature_detected!("avx512f") {
            SimdTier::Avx512
        } else if cfg!(feature = "simd-avx2") && std::is_x86_feature_detected!("avx2") {
            SimdTier::Avx2
        } else if std::is_x86_feature_detected!("sse4.2") {
            SimdTier::Sse42
        } else {
            SimdTier::Sse2
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is architecturally mandatory on all AArch64 hardware; the
        // check is defensive in case that invariant is ever violated by
        // future/unusual hardware.
        if std::arch::is_aarch64_feature_detected!("neon") {
            SimdTier::Neon
        } else {
            SimdTier::Portable
        }
    }

    // Fallback for unsupported architectures (e.g., RISC-V, WASM).
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        SimdTier::Portable
    }
}

// ---- Linux capability probes -------------------------------------------------

#[cfg(target_os = "linux")]
fn detect_linux(caps: &mut Capabilities) {
    // inotify is always available on Linux >= 2.6.13.
    caps.has_inotify = true;

    // Probe statx(2): requires kernel >= 4.11 (released 2017-07-02).
    // TODO Wave D: replace with real syscall probe (attempt statx on AT_FDCWD with
    //   STATX_TYPE | STATX_MODE | STATX_NLINK, check for ENOSYS).
    caps.has_statx = probe_statx_stub();

    // Probe openat2(2): requires kernel >= 5.6 (released 2020-03-29).
    // Real probe: attempt openat2 with empty path + O_PATH + RESOLVE_BENEATH.
    // ENOSYS → kernel too old; any other errno (e.g. ENOENT) → syscall present.
    caps.has_openat2 = probe_openat2_stub();

    // Probe fanotify: requires kernel >= 2.6.37 and CAP_SYS_ADMIN.
    // TODO Wave D: attempt fanotify_init with EINVAL check vs EPERM/ENOSYS.
    caps.has_fanotify = false;

    // io_uring: requires kernel >= 5.1 AND the linux-iouring Cargo feature.
    // TODO Wave D: probe io_uring_setup(0, ...) for ENOSYS.
    caps.has_io_uring = false;
}

#[cfg(target_os = "linux")]
const fn probe_statx_stub() -> bool {
    // Stub: returns false until Wave D implements the real syscall probe.
    // Real probe: statx(AT_FDCWD, "", AT_EMPTY_PATH, STATX_NLINK, &mut buf)
    // ENOSYS -> false; ENOENT or any other -> true.
    false
}

/// Probe whether `openat2(2)` is available on the running kernel.
///
/// Delegates to `substrate_policy::probe_openat2_available()`, which contains
/// the narrow `unsafe` syscall carve-out (ADR-0042 + ADR-0044).  This function
/// is safe to call without `unsafe_code` permission in this crate.
#[cfg(target_os = "linux")]
fn probe_openat2_stub() -> bool {
    substrate_policy::probe_openat2_available()
}

// ---- macOS capability probes -------------------------------------------------

#[cfg(target_os = "macos")]
fn detect_macos(caps: &mut Capabilities) {
    // FSEvents and kqueue are always available on macOS.
    caps.has_fsevents = true;
    caps.has_kqueue = true;

    // getattrlistbulk(2) is available since macOS 10.10 (Yosemite).
    // All macOS versions substrate targets (>= 12, see ADR-0042) include it.
    caps.has_getattrlistbulk = true;

    // O_NOFOLLOW_ANY is available since macOS 12.0 (Monterey).
    caps.has_o_nofollow_any = macos_major_version() >= 12;
}

/// Maps the running kernel's Darwin major release to a macOS major version.
///
/// Darwin release = macOS major + 9 (for macOS >= 11): Darwin 21.x = macOS
/// 12.x, Darwin 22.x = macOS 13.x, Darwin 23.x = macOS 14.x, etc.
///
/// Gates `has_o_nofollow_any` (and therefore `pick_jail_tier`'s choice between
/// the kernel-enforced `MacosONoFollowAny` tier and the `UserspaceDegraded`
/// fallback per ADR-0035/ADR-0042): a wrong value on a host older than macOS
/// 12 would select a tier whose `O_NOFOLLOW_ANY` open flag does not exist on
/// that kernel, so the underlying release must come from a real probe, not a
/// literal picked for whichever macOS the binary happened to be built on.
///
/// Delegates to [`darwin_release_major`] for the actual probe; see that
/// function's docs for the fail-safe contract on probe failure.
#[cfg(target_os = "macos")]
fn macos_major_version() -> u64 {
    darwin_release_major().unwrap_or(0).saturating_sub(9)
}

/// Reads the running kernel's Darwin major release number via `uname(2)`.
///
/// This is the safe-FFI equivalent of `sysctlbyname("kern.osrelease", ...)` —
/// both read the identical kernel-reported release string on Darwin — chosen
/// because `substrate-mcp-server` sets `#![cfg_attr(not(test),
/// forbid(unsafe_code))]` crate-wide in `main.rs` and, unlike the narrow
/// syscall-shim crates this workspace uses for such carve-outs (e.g.
/// `substrate-signal-sys` for `SIGPIPE`), that `forbid` cannot be downgraded
/// locally to reach for `libc::sysctlbyname` directly in this file.
/// `substrate_system_info::handle_sys_uname` already wraps the unsafe-free
/// `nix::sys::utsname::uname()` for exactly this reason (see that crate's
/// `uname.rs` module doc), so this probe reuses it instead of introducing a
/// second syscall shim for the same class of problem. The handler performs a
/// single synchronous `uname(2)` call and never awaits, so driving it with
/// `futures::executor::block_on` resolves on the very first poll — equivalent
/// to a direct synchronous call, with no thread-parking or reactor
/// involvement.
///
/// Returns `None` if the handler errors or the release string does not begin
/// with a parseable integer. The caller ([`macos_major_version`]) treats
/// `None` as major version `0`, which is always older than the macOS 12
/// baseline `has_o_nofollow_any` requires — i.e. probe failure fails safe
/// toward the degraded userspace jail tier, never toward the stronger
/// kernel-enforced tier.
#[cfg(target_os = "macos")]
fn darwin_release_major() -> Option<u64> {
    let deps = std::sync::Arc::new(substrate_system_info::SystemInfoDeps {
        capabilities: std::sync::Arc::new(Capabilities::default()),
    });
    let Ok(resp) = futures::executor::block_on(substrate_system_info::handle_sys_uname(deps))
    else {
        tracing::warn!("uname(2) probe failed; assuming macOS major version 0 (fail-safe)");
        return None;
    };
    let release = resp.structured_content.get("release").and_then(|v| v.as_str());
    let Some(major) = release.and_then(|r| r.split('.').next()).and_then(|s| s.parse().ok()) else {
        tracing::warn!(
            release = ?release,
            "could not parse Darwin major release from uname(2) output; assuming macOS major version 0 (fail-safe)"
        );
        return None;
    };
    Some(major)
}

// ---- Tier selection ----------------------------------------------------------

const fn pick_walker_tier(caps: &Capabilities) -> WalkerTier {
    #[cfg(target_os = "linux")]
    {
        if caps.has_statx {
            return WalkerTier::LinuxStatx;
        }
        return WalkerTier::LinuxLegacy;
    }

    #[cfg(target_os = "macos")]
    {
        if caps.has_getattrlistbulk {
            return WalkerTier::MacosBulk;
        }
        return WalkerTier::MacosLegacy;
    }

    // Portable fallback for non-Linux, non-macOS targets.
    #[allow(unreachable_code, reason = "compile-time dead on Linux and macOS")]
    WalkerTier::PortableStdfs
}

const fn pick_watcher_tier(caps: &Capabilities) -> WatcherTier {
    #[cfg(target_os = "linux")]
    {
        if caps.has_inotify {
            return WatcherTier::LinuxInotify;
        }
        return WatcherTier::Polling;
    }

    #[cfg(target_os = "macos")]
    {
        if caps.has_fsevents {
            return WatcherTier::MacosFsevents;
        }
        if caps.has_kqueue {
            return WatcherTier::MacosKqueue;
        }
        return WatcherTier::Polling;
    }

    #[allow(unreachable_code, reason = "compile-time dead on Linux and macOS")]
    WatcherTier::Polling
}

const fn pick_jail_tier(caps: &Capabilities) -> JailTier {
    #[cfg(target_os = "linux")]
    {
        if caps.has_openat2 {
            return JailTier::LinuxOpenat2;
        }
        return JailTier::UserspaceDegraded;
    }

    #[cfg(target_os = "macos")]
    {
        if caps.has_o_nofollow_any {
            return JailTier::MacosONoFollowAny;
        }
        return JailTier::UserspaceDegraded;
    }

    #[allow(unreachable_code, reason = "compile-time dead on Linux and macOS")]
    JailTier::UserspaceDegraded
}

const fn pick_hash_tier(caps: &Capabilities) -> HashTier {
    match caps.simd_tier {
        SimdTier::Avx512 => HashTier::Blake3Avx512,
        SimdTier::Avx2 => HashTier::Blake3Avx2,
        SimdTier::Neon => HashTier::Blake3Neon,
        SimdTier::Sse42 | SimdTier::Sse2 => HashTier::Blake3Sse2,
        SimdTier::Portable => HashTier::Blake3Portable,
    }
}

const fn pick_stat_tier(caps: &Capabilities) -> StatTier {
    #[cfg(target_os = "linux")]
    {
        if caps.has_statx {
            return StatTier::LinuxStatx;
        }
        return StatTier::LinuxFstatat;
    }

    #[cfg(target_os = "macos")]
    {
        if caps.has_getattrlistbulk {
            return StatTier::MacosGetattrlist;
        }
        return StatTier::MacosFstatat;
    }

    #[allow(unreachable_code, reason = "compile-time dead on Linux and macOS")]
    StatTier::PortableMetadata
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_consistent_tiers() {
        let caps = detect();
        // hash_tier must be consistent with simd_tier
        match caps.simd_tier {
            SimdTier::Avx512 => assert_eq!(caps.hash_tier, HashTier::Blake3Avx512),
            SimdTier::Avx2 => assert_eq!(caps.hash_tier, HashTier::Blake3Avx2),
            SimdTier::Neon => assert_eq!(caps.hash_tier, HashTier::Blake3Neon),
            SimdTier::Sse42 | SimdTier::Sse2 => {
                assert_eq!(caps.hash_tier, HashTier::Blake3Sse2);
            },
            SimdTier::Portable => assert_eq!(caps.hash_tier, HashTier::Blake3Portable),
        }
    }

    #[test]
    fn once_lock_is_idempotent() {
        let first = probe();
        let second = probe();
        // Both references must point to the same allocation.
        assert!(std::ptr::eq(first, second));
    }
}
