// DDD role: ValueObject
package schemas

// #SimdTier enumerates the SIMD instruction set tiers detected at startup per ADR-0042 and ADR-0043.
// Detection is performed once via probe_capabilities() and cached in OnceLock<Capabilities>.
// Avx512 is opt-in only due to CPU frequency throttling on older Intel steppings per ADR-0043.
#SimdTier: "avx512" | "avx2" | "sse42" | "sse2" | "neon" | "portable"

// #WalkerTier enumerates the directory-walk implementation tiers per ADR-0041 and ADR-0042.
// Selection is performed once by DirWalkerFactory at composition root startup.
// linux-iouring requires kernel 5.1+ and the linux-iouring Cargo feature.
// portable-stdfs is the cross-platform fallback using std::fs::read_dir.
#WalkerTier:
	"linux-iouring" |
	"linux-statx" |
	"linux-legacy" |
	"macos-bulk" |
	"macos-legacy" |
	"portable-stdfs"

// #WatcherTier enumerates the filesystem-watch implementation tiers per ADR-0041 and ADR-0042.
// Selection is performed once by FsWatcherFactory at composition root startup.
// polling is the Null Object (PollingWatcher) used when no kernel watcher is available.
// A tracing::warn! is emitted at construction time when polling is selected.
#WatcherTier:
	"linux-inotify" |
	"linux-fanotify" |
	"macos-fsevents" |
	"macos-kqueue" |
	"polling"

// #JailTier enumerates the path-jail implementation tiers per ADR-0035 and ADR-0042.
// userspace-degraded is a security-sensitive fallback; it does not atomically close the
// TOCTOU window. An SUBSTRATE_JAIL_DEGRADED audit event is emitted and, by default,
// startup aborts with exit code 77 when this tier is selected (refuse_degraded_jail = true).
#JailTier: "linux-openat2" | "macos-o-nofollow-any" | "userspace-degraded"

// #HashTier enumerates the BLAKE3 hashing implementation tiers per ADR-0042 and ADR-0043.
// Selection is driven by caps.simd_tier at HashFactory build time.
// Note: the blake3 mmap feature is DISABLED per signal-safety contract in ADR-0032.
#HashTier:
	"blake3-avx512" |
	"blake3-avx2" |
	"blake3-neon" |
	"blake3-sse2" |
	"blake3-portable"

// #StatTier enumerates the file-stat implementation tiers per ADR-0042.
// Selection is performed once by StatFactory at composition root startup.
// linux-statx requires kernel 4.11+. macos-getattrlist requires macOS 10.10+.
#StatTier:
	"linux-statx" |
	"linux-fstatat" |
	"macos-getattrlist" |
	"macos-fstatat" |
	"portable-metadata"

// #Capabilities is the startup snapshot of detected runtime capabilities per ADR-0042.
// It is produced once by probe_capabilities() and stored in OnceLock<Capabilities>.
// A SUBSTRATE_CAPABILITY_TIERS_SELECTED audit event is emitted before any MCP session
// is accepted, recording all selected tier strings.
// Closed struct: all fields are mandatory; omitting any field is a schema violation.
#Capabilities: {
	// simd_tier is the highest SIMD instruction set available on this CPU.
	simd_tier: #SimdTier

	// walker_tier is the selected directory-walk implementation for this platform.
	walker_tier: #WalkerTier

	// watcher_tier is the selected filesystem-watch implementation for this platform.
	watcher_tier: #WatcherTier

	// jail_tier is the selected path-jail implementation for this platform.
	// userspace-degraded triggers a SUBSTRATE_JAIL_DEGRADED audit event per ADR-0042.
	jail_tier: #JailTier

	// hash_tier is the selected BLAKE3 hashing implementation driven by simd_tier.
	hash_tier: #HashTier

	// stat_tier is the selected file-stat implementation for this platform.
	stat_tier: #StatTier

	// has_openat2 is true when the Linux kernel supports openat2(2) (kernel 5.6+).
	has_openat2: bool

	// has_statx is true when the Linux kernel supports statx(2) (kernel 4.11+).
	has_statx: bool

	// has_io_uring is true when io_uring is available (kernel 5.1+) and the
	// linux-iouring Cargo feature is compiled in.
	has_io_uring: bool

	// has_inotify is always true on Linux (kernel 2.6.13+).
	has_inotify: bool

	// has_fanotify is true when fanotify is available and CAP_SYS_ADMIN is held.
	// A runtime capability check is performed; false when privilege is absent.
	has_fanotify: bool

	// has_getattrlistbulk is true on macOS 10.10+ (getattrlistbulk(2) available).
	has_getattrlistbulk: bool

	// has_fsevents is always true on macOS.
	has_fsevents: bool

	// has_kqueue is always true on macOS.
	has_kqueue: bool

	// has_o_nofollow_any is true on macOS 12+ (Monterey) where O_NOFOLLOW_ANY is available.
	has_o_nofollow_any: bool
}

// #CapabilityOverride allows operators to force a specific tier for integration testing
// or explicit tier downgrade per ADR-0042. All fields are optional; absent fields use
// the probe-detected tier. Override values are validated at config load time.
// An invalid tier name aborts startup with SUBSTRATE_CONFIG_INVALID.
#CapabilityOverride: {
	// walker overrides the DirWalkerFactory tier selection.
	walker?: #WalkerTier

	// watcher overrides the FsWatcherFactory tier selection.
	watcher?: #WatcherTier

	// jail overrides the PathJailFactory tier selection.
	// Forcing userspace-degraded still emits SUBSTRATE_JAIL_DEGRADED per ADR-0042.
	jail?: #JailTier

	// hash overrides the HashFactory tier selection.
	hash?: #HashTier

	// stat overrides the StatFactory tier selection.
	stat?: #StatTier
}
