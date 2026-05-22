//! Capability snapshot and tier enumerations per ADR-0042 and ADR-0043.
//!
//! Mirrors `#Capabilities`, `#SimdTier`, `#WalkerTier`, `#WatcherTier`,
//! `#JailTier`, `#HashTier`, `#StatTier`, and `#CapabilityOverride` from
//! `docs/arch/schemas/simd_capability.cue`.
//!
//! `probe_capabilities()` runs once at startup (in `substrate-mcp-server`)
//! and stores the result in `std::sync::OnceLock<Capabilities>`. Domain code
//! receives a shared reference; it never re-probes.

use serde::{Deserialize, Serialize};

// ---- Tier enumerations -------------------------------------------------------

/// SIMD instruction set tiers detected at startup per ADR-0042 and ADR-0043.
///
/// Ordered weakest-to-strongest: `Portable` < `Sse2` < `Sse42` < `Avx2` < `Avx512`.
/// `Neon` is the ARM equivalent of `Avx2`.
/// `Avx512` is opt-in only due to CPU frequency throttling on older Intel steppings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SimdTier {
    /// AVX-512 — opt-in; highest throughput but may cause frequency throttling.
    Avx512,
    /// AVX2 — common x86-64 baseline (Haswell+).
    Avx2,
    /// SSE4.2 — penryn+ / nehalem+.
    Sse42,
    /// SSE2 — x86-64 minimum guarantee.
    Sse2,
    /// NEON — ARM64 equivalent of AVX2.
    Neon,
    /// Portable scalar fallback; no SIMD intrinsics.
    Portable,
}

/// Directory-walk implementation tiers per ADR-0041 and ADR-0042.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WalkerTier {
    /// Linux io_uring-based walker — requires kernel ≥ 5.1 and `linux-iouring` feature.
    LinuxIouring,
    /// Linux `statx(2)` walker — requires kernel ≥ 4.11.
    LinuxStatx,
    /// Linux legacy walker using `readdir` / `getdents64`.
    LinuxLegacy,
    /// macOS `getattrlistbulk(2)` — requires macOS 10.10+.
    MacosBulk,
    /// macOS legacy walker using `readdir`.
    MacosLegacy,
    /// Cross-platform `std::fs::read_dir` fallback.
    PortableStdfs,
}

/// Filesystem-watch implementation tiers per ADR-0041 and ADR-0042.
///
/// `Polling` is the Null Object (`PollingWatcher`) used when no kernel watcher
/// is available. A `tracing::warn!` is emitted at construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WatcherTier {
    /// Linux `inotify(7)` — always available on Linux ≥ 2.6.13.
    LinuxInotify,
    /// Linux `fanotify(7)` — requires `CAP_SYS_ADMIN`.
    LinuxFanotify,
    /// macOS `FSEvents` API — always available on macOS.
    MacosFsevents,
    /// macOS `kqueue(2)`.
    MacosKqueue,
    /// Polling fallback (Null Object); emits `tracing::warn!` at construction.
    Polling,
}

/// Path-jail implementation tiers per ADR-0035 and ADR-0042.
///
/// `UserspaceDegraded` does not atomically close the TOCTOU window. When selected
/// and `security.refuse_degraded_jail = true` (default), startup aborts with
/// `SUBSTRATE_JAIL_DEGRADED_REFUSED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JailTier {
    /// Linux `openat2(2)` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` — requires kernel ≥ 5.6.
    LinuxOpenat2,
    /// macOS `O_NOFOLLOW_ANY` — requires macOS 12+ (Monterey).
    MacosONoFollowAny,
    /// Userspace-only fallback; TOCTOU window is not atomically closed.
    UserspaceDegraded,
}

/// BLAKE3 hashing implementation tiers per ADR-0042 and ADR-0043.
///
/// Selection is driven by `caps.simd_tier` at `HashFactory` build time.
/// Note: the `blake3` mmap feature is DISABLED per signal-safety contract in ADR-0032.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HashTier {
    /// BLAKE3 with AVX-512 acceleration.
    Blake3Avx512,
    /// BLAKE3 with AVX2 acceleration.
    Blake3Avx2,
    /// BLAKE3 with NEON acceleration.
    Blake3Neon,
    /// BLAKE3 with SSE2 acceleration.
    Blake3Sse2,
    /// BLAKE3 portable scalar implementation.
    Blake3Portable,
}

/// File-stat implementation tiers per ADR-0042.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatTier {
    /// Linux `statx(2)` — requires kernel ≥ 4.11.
    LinuxStatx,
    /// Linux `fstatat(2)` fallback.
    LinuxFstatat,
    /// macOS `getattrlist(2)` — requires macOS 10.10+.
    MacosGetattrlist,
    /// macOS `fstatat(2)` fallback.
    MacosFstatat,
    /// Cross-platform `std::fs::metadata` fallback.
    PortableMetadata,
}

// ---- Capabilities snapshot --------------------------------------------------

/// Startup snapshot of detected runtime capabilities per ADR-0042.
///
/// Produced once by `probe_capabilities()` and stored in
/// `std::sync::OnceLock<Capabilities>`. Subsequent reads use `.get()`.
///
/// A `SUBSTRATE_CAPABILITY_TIERS_SELECTED` audit event is emitted before any
/// MCP session is accepted, recording all selected tier strings.
///
/// All fields are mandatory; absent fields are a schema violation per the
/// CUE `#Capabilities` closed struct.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the CUE #Capabilities closed struct verbatim; each bool is a \
              distinct kernel-feature presence flag — a state machine would add \
              indirection without semantic benefit"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    /// Highest SIMD instruction set available on this CPU.
    pub simd_tier: SimdTier,

    /// Selected directory-walk implementation for this platform.
    pub walker_tier: WalkerTier,

    /// Selected filesystem-watch implementation for this platform.
    pub watcher_tier: WatcherTier,

    /// Selected path-jail implementation for this platform.
    ///
    /// `UserspaceDegraded` triggers a `SUBSTRATE_JAIL_DEGRADED` audit event.
    pub jail_tier: JailTier,

    /// Selected BLAKE3 hashing implementation driven by `simd_tier`.
    pub hash_tier: HashTier,

    /// Selected file-stat implementation for this platform.
    pub stat_tier: StatTier,

    /// `true` when the Linux kernel supports `openat2(2)` (kernel ≥ 5.6).
    pub has_openat2: bool,

    /// `true` when the Linux kernel supports `statx(2)` (kernel ≥ 4.11).
    pub has_statx: bool,

    /// `true` when `io_uring` is available (kernel ≥ 5.1) and the
    /// `linux-iouring` Cargo feature is compiled in.
    pub has_io_uring: bool,

    /// Always `true` on Linux (kernel ≥ 2.6.13).
    pub has_inotify: bool,

    /// `true` when `fanotify` is available and `CAP_SYS_ADMIN` is held.
    pub has_fanotify: bool,

    /// `true` on macOS 10.10+ where `getattrlistbulk(2)` is available.
    pub has_getattrlistbulk: bool,

    /// Always `true` on macOS.
    pub has_fsevents: bool,

    /// Always `true` on macOS.
    pub has_kqueue: bool,

    /// `true` on macOS 12+ (Monterey) where `O_NOFOLLOW_ANY` is available.
    pub has_o_nofollow_any: bool,
}

impl Default for Capabilities {
    /// Returns a safe baseline using the portable tier for every capability.
    ///
    /// Used in tests and as the fallback when probing has not completed.
    /// Production code MUST replace this with the result of `probe_capabilities()`.
    fn default() -> Self {
        Self {
            simd_tier: SimdTier::Portable,
            walker_tier: WalkerTier::PortableStdfs,
            watcher_tier: WatcherTier::Polling,
            jail_tier: JailTier::UserspaceDegraded,
            hash_tier: HashTier::Blake3Portable,
            stat_tier: StatTier::PortableMetadata,
            has_openat2: false,
            has_statx: false,
            has_io_uring: false,
            has_inotify: false,
            has_fanotify: false,
            has_getattrlistbulk: false,
            has_fsevents: false,
            has_kqueue: false,
            has_o_nofollow_any: false,
        }
    }
}

// ---- Operator override ------------------------------------------------------

/// Operator-supplied tier overrides for integration testing or explicit tier downgrade.
///
/// Mirrors `#CapabilityOverride` in `docs/arch/schemas/simd_capability.cue`.
/// All fields are optional; absent fields use the probe-detected tier.
/// Validated at config load time — an invalid tier name aborts startup with
/// `SUBSTRATE_TIER_OVERRIDE_INVALID`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityOverride {
    /// Overrides the `DirWalkerFactory` tier selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walker: Option<WalkerTier>,

    /// Overrides the `FsWatcherFactory` tier selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watcher: Option<WatcherTier>,

    /// Overrides the `PathJailFactory` tier selection.
    ///
    /// Forcing `UserspaceDegraded` still emits `SUBSTRATE_JAIL_DEGRADED` per ADR-0042.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jail: Option<JailTier>,

    /// Overrides the `HashFactory` tier selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<HashTier>,

    /// Overrides the `StatFactory` tier selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat: Option<StatTier>,
}
