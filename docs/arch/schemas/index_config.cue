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

	// ---- Content index (ADR-0072) -----------------------------------------
	// The fields below activate the content-side inverted index and BM25
	// relevance ranking layered onto this same index per ADR-0072. They have
	// no effect unless `enabled` above is also true, and the content-search
	// specific fields require the fs-index-content Cargo feature to be
	// compiled in on top of fs-index.

	// content_index_enabled activates the content inverted index and BM25
	// relevance ranking for text.search per ADR-0072. Requires both the
	// fs-index and fs-index-content Cargo features to be compiled in.
	// Default OFF: text.search falls back to the unranked line-scan path
	// from ADR-0003 when false.
	content_index_enabled: bool | *false

	// bm25 configures the Okapi BM25 ranking parameters applied when
	// content_index_enabled is true. Has no effect otherwise.
	bm25: #Bm25Params

	// mmr_lambda controls the MMR-style per-file diversity penalty applied to
	// ranked results per ADR-0072: 0.0 disables diversity re-ranking (pure
	// BM25 order); 1.0 maximizes spread across distinct files. The penalty
	// subtracted from a candidate's score is
	// mmr_lambda * bm25_score * (already-selected hits from the same file).
	mmr_lambda: number & >=0.0 & <=1.0 | *0.3

	// max_hits_per_file hard-caps the number of ranked matches returned from
	// a single file regardless of mmr_lambda, so one file can never consume
	// an entire result budget.
	max_hits_per_file: int & >=1 | *5

	// token_budget_default is the default token budget (approximate,
	// ~4 bytes/token heuristic per ADR-0007) applied to a text.search
	// response when the caller supplies no explicit budget. Ranked results
	// are truncated greedily by descending score once the cumulative
	// estimate exceeds this value; hints.truncated_by_budget signals when
	// the budget, rather than page_size, was the limiting factor.
	token_budget_default: int & >=1 | *2000

	// max_snippet_bytes bounds the context-window snippet built around each
	// ranked match's highest-scoring position.
	max_snippet_bytes: int & >=1 | *512

	// command_queue_capacity is the bounded mpsc<IndexCommand> channel
	// capacity feeding the single-writer IndexerActor per ADR-0072.
	// Producers use try_send with coalesce-on-full: a command that cannot be
	// enqueued downgrades to marking its root dirty for the next TTL/watch
	// rebuild rather than blocking the caller.
	command_queue_capacity: int & >=1 | *4096

	// event_broadcast_capacity is the broadcast<IndexEvent> channel capacity.
	// A lagged subscriber (per tokio broadcast lag semantics) MUST treat the
	// lag as equivalent to a missed Rescan per ADR-0072.
	event_broadcast_capacity: int & >=1 | *1024

	// hot_shard_max_entries bounds the mutable hot delta shard before a
	// background compaction merges it into an immutable segment per
	// ADR-0072. Keeps per-mutation publish cost independent of total corpus
	// size (amortized O(1), not O(total entries)).
	hot_shard_max_entries: int & >=1 | *1000

	// ack_wait_ms bounds how long a write-through mutation waits for the
	// IndexerActor's oneshot ack before returning its tool response,
	// consistent with the no-unbounded-wait principle of ADR-0059. A timeout
	// does not fail the mutation; it degrades to relying on the Layer-0
	// lazy-lstat / content-hash revalidation backstop on the next read.
	ack_wait_ms: int & >=1 | *50
}
