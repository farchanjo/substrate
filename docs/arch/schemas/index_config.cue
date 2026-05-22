// DDD role: ValueObject
package schemas

// #IndexConfig is the runtime configuration for the optional in-process filesystem index
// per ADR-0041. It is embedded in the main RuntimeConfig under the [index] TOML section.
// The index is OFF by default; the Cargo feature fs-index must also be compiled in.
// Closed struct: all fields that have defaults must appear explicitly in TOML when changed.
#IndexConfig: {
	// enabled activates the in-process filesystem index.
	// Requires the fs-index Cargo feature to be compiled in.
	// Default OFF: the non-indexed ignore-crate walk path from ADR-0003 is used when false.
	enabled: bool | *false

	// watch_enabled activates the filesystem watcher layer (Layer 2) per ADR-0041.
	// Requires both fs-index and fs-index-watch Cargo features to be compiled in.
	// Has no effect when enabled is false.
	watch_enabled: bool | *false

	// ttl_secs is the snapshot freshness TTL per ADR-0041 Layer 3.
	// On expiry, an incremental Zone B rebuild is triggered on the next lookup.
	// The stale snapshot continues to serve reads filtered by Layer 0 lstat while the
	// rebuild is in progress.
	ttl_secs: int & >=1 | *60

	// max_entries is the maximum number of path entries retained in the snapshot.
	// Exceeding this limit triggers LRU eviction of least-recently-accessed entries.
	// 0 means unbounded (not recommended; may exhaust process RSS).
	max_entries: int & >=0 | *1000000

	// max_bytes is the approximate memory ceiling for the snapshot in bytes (256 MiB default).
	// Exceeding this limit triggers LRU eviction alongside max_entries.
	// 0 means unbounded (not recommended; may exhaust process RSS).
	max_bytes: int & >=0 | *268435456

	// poll_secs is the polling interval used by the PollingWatcher Null Object per ADR-0042.
	// Only active when watch_enabled is true but no kernel watcher tier is available.
	// Configuring a low value increases CPU overhead; values below 5 are not recommended.
	poll_secs: int & >=1 | *30

	// rebuild_concurrency is the per-root parallel rebuild cap during Zone B snapshot refresh.
	// Higher values reduce rebuild latency for trees with many allowlist roots at the cost
	// of additional spawn_blocking worker threads per ADR-0003.
	rebuild_concurrency: int & >=1 | *2
}
