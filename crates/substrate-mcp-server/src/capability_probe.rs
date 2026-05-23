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
            return SimdTier::Avx512;
        }
        if cfg!(feature = "simd-avx2") && std::is_x86_feature_detected!("avx2") {
            return SimdTier::Avx2;
        }
        if std::is_x86_feature_detected!("sse4.2") {
            return SimdTier::Sse42;
        }
        return SimdTier::Sse2;
    }

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is architecturally mandatory on all AArch64 hardware.
        if std::arch::is_aarch64_feature_detected!("neon") {
            return SimdTier::Neon;
        }
    }

    // Fallback for unsupported architectures (e.g., RISC-V, WASM).
    SimdTier::Portable
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
fn probe_statx_stub() -> bool {
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
const fn detect_macos(caps: &mut Capabilities) {
    // FSEvents and kqueue are always available on macOS.
    caps.has_fsevents = true;
    caps.has_kqueue = true;

    // getattrlistbulk(2) is available since macOS 10.10 (Yosemite).
    // All macOS versions substrate targets (>= 12, see ADR-0042) include it.
    caps.has_getattrlistbulk = true;

    // O_NOFOLLOW_ANY is available since macOS 12.0 (Monterey).
    caps.has_o_nofollow_any = macos_major_version() >= 12;
}

#[cfg(target_os = "macos")]
const fn macos_major_version() -> u64 {
    // Parse the macOS major version from `sw_vers -productVersion` alternative:
    // use sysctl kern.osrelease and map Darwin release to macOS version.
    // Darwin 21.x = macOS 12.x, Darwin 22.x = macOS 13.x, etc.
    // Darwin release = macOS major + 9 (for macOS >= 11).
    // TODO Wave D: replace with a proper sysctl probe via nix::sys::sysctl.
    // Stub: assume macOS 14 (Darwin 23.x) for CI; real probe needed for production.
    let darwin_major: u64 = 23; // macOS 14 = Darwin 23
    darwin_major.saturating_sub(9)
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
