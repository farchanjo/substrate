// DDD role: AggregateRoot
package schemas

// #LogLevel enumerates the structured log verbosity levels.
#LogLevel: "trace" | "debug" | "info" | "warn" | "error"

// #LogTarget identifies where log output is directed.
#LogTarget: "stderr" | "file"

// #Timeouts configures execution time limits across tools.
#Timeouts: {
	// global_default_seconds applies when no per-tool override is present.
	global_default_seconds: uint & >=1 | *30

	// per_tool maps individual tool names to their timeout in seconds.
	// Entries here take precedence over global_default_seconds.
	per_tool: {[string]: uint & >=1}
}

// #SemaphoreCaps limits concurrent execution to prevent resource exhaustion.
#SemaphoreCaps: {
	// cpu_bound_max is the maximum concurrent CPU-bound tool executions.
	cpu_bound_max: uint & >=1

	// per_namespace maps each tool namespace to its own concurrency ceiling.
	per_namespace: {[string]: uint & >=1}

	// max_waiters is the maximum number of callers queued behind a full semaphore.
	// Requests exceeding this limit receive SUBSTRATE_RESOURCE_LIMIT immediately.
	max_waiters: uint & >=1 | *256

	// zone_b_max caps concurrent executions in the zone-B (background) semaphore ring.
	// If absent, the runtime computes: num_cpus * 4 at startup.
	zone_b_max?: uint & >=1
}

// #LoggingConfig controls structured logging behaviour of the substrate runtime.
#LoggingConfig: {
	// level controls the minimum severity emitted.
	level: #LogLevel | *"info"

	// target directs output to stderr or a named file.
	target: #LogTarget | *"stderr"

	// file_path is required when target is "file"; must be an absolute path.
	file_path?: string & =~"^/"

	// redaction_extra_patterns is a list of additional Go-compatible regex patterns
	// whose matches are replaced with [REDACTED] before any log line is written.
	redaction_extra_patterns: [...string] | *[]

	// max_log_file_bytes is the rolling size ceiling for a log file before rotation (default 100 MiB).
	max_log_file_bytes: uint & >=1048576 | *104857600

	// log_rotate_count is the number of rotated log files retained on disk (default 7).
	log_rotate_count: uint & >=1 | *7

	// log_write_error_policy controls runtime behavior when a log write fails.
	// warn_stderr_fallback: emit a warning to stderr and continue.
	// abort: terminate the process to preserve audit integrity.
	log_write_error_policy: "warn_stderr_fallback" | "abort" | *"warn_stderr_fallback"
}

// #ProtocolConfig governs MCP wire-level constraints.
#ProtocolConfig: {
	// max_page_size is the hard ceiling for pagination; clients may not exceed this.
	max_page_size: uint & >=1 | *500

	// default_page_size is used when the client omits a page_size parameter.
	default_page_size: uint & >=1 & <=max_page_size | *50

	// max_in_memory_buffer_bytes caps single in-memory read/write operations (8 MiB default).
	// Hard ceiling: 33554432 (32 MiB) per ADR-0016 revised resource-limits ceiling.
	max_in_memory_buffer_bytes: uint & <=33554432 | *8388608

	// max_archive_input_bytes caps the decompressed size of any archive processed (1 GiB default).
	max_archive_input_bytes: uint | *1073741824

	// max_in_flight_requests is the maximum number of concurrent JSON-RPC requests
	// the server will process before returning SUBSTRATE_RESOURCE_LIMIT (default 32).
	max_in_flight_requests: uint & >=1 | *32

	// max_inbound_message_bytes is the maximum size of a single inbound JSON-RPC
	// message before the connection is rejected (default 1 MiB).
	max_inbound_message_bytes: uint & >=4096 | *1048576

	// elicitation_timeout_secs is the maximum time the server waits for a user
	// to respond to an elicitation prompt before cancelling the request (default 60s).
	elicitation_timeout_secs: uint & >=1 | *60

	// max_outbound_frame_queue is the maximum number of frames queued for a single
	// client connection before backpressure drops the connection (default 1024).
	max_outbound_frame_queue: uint & >=1 | *1024

	// write_timeout_secs is the maximum time allowed to complete a single outbound
	// frame write before the connection is considered stalled and closed (default 30s).
	write_timeout_secs: uint & >=1 | *30
}

// #SecurityRuntime configures runtime-level security hardening knobs.
// These mirror fields in #SecurityPolicy; the runtime config takes effect after
// the policy is loaded and may further restrict behaviour.
#SecurityRuntime: {
	// reject_hardlinks causes the runtime to refuse to open or create hard links
	// to files outside the allowlist, preventing hard-link escape attacks.
	reject_hardlinks: bool | *false

	// archive_allow_symlinks permits symlinks inside extracted archive contents.
	// Disabled by default; enabling may allow symlink escape from the extraction root.
	archive_allow_symlinks: bool | *false

	// max_process_rss_bytes is the RSS ceiling for the substrate process (default 256 MiB).
	// The runtime raises SUBSTRATE_RESOURCE_LIMIT if the limit is exceeded.
	max_process_rss_bytes: uint & >=1048576 | *268435456

	// refuse_degraded_jail aborts startup when PathJail falls back to the userspace tier
	// (per ADR-0035 amendment and ADR-0042). Defaults to true; operators who accept the
	// TOCTOU risk must set this to false explicitly.
	refuse_degraded_jail: bool | *true

	// refuse_polling_watcher aborts startup when FsWatcher falls back to PollingWatcher
	// (per ADR-0042). Defaults to false because polling is functionally equivalent.
	refuse_polling_watcher: bool | *false

	// log_tier_on_startup emits a tracing::info! line listing all chosen adapter tiers
	// and the SIMD tier at startup (per ADR-0042). Defaults to true.
	log_tier_on_startup: bool | *true
}

// #RuntimeConfig is the top-level aggregate root for substrate runtime tuning.
// All sub-sections have safe defaults; omitting a section activates those defaults.
#RuntimeConfig: {
	// timeouts configures per-tool and global execution time limits.
	timeouts: #Timeouts

	// semaphore_caps limits concurrent tool execution.
	semaphore_caps: #SemaphoreCaps

	// logging controls structured log emission.
	logging: #LoggingConfig

	// protocol governs MCP wire-level behaviour.
	protocol: #ProtocolConfig

	// security contains runtime-level security hardening knobs.
	security: #SecurityRuntime

	// shutdown_drain_secs is the maximum time (in seconds) the runtime waits for
	// in-flight requests to complete during graceful shutdown (default 5s, max 120s).
	shutdown_drain_secs: uint & >=1 & <=120 | *5

	// jobs configures the async job control-plane per ADR-0040.
	// Omitting this section disables the job control-plane entirely; only Bucket A
	// and Bucket D tools are then available; Bucket B/C tools return an error.
	jobs?: #JobConfig

	// index configures the optional in-process filesystem index per ADR-0041.
	// Disabled by default; enable via index.enabled = true plus feature flag fs-index.
	index?: #IndexConfig

	// capabilities configures operator overrides for the capability-based adapter
	// factory per ADR-0042. Useful for integration testing specific tier paths.
	capabilities?: {
		// override forces specific adapter tiers regardless of probe results (per ADR-0042).
		// Keys are port names (e.g. "DirWalker", "PathJail"); values are tier strings.
		// Invalid tier names abort startup with SUBSTRATE_CONFIG_INVALID.
		override?: #CapabilityOverride
	}

	// simd configures SIMD tier opt-in behaviour per ADR-0043.
	// AVX-512 is opt-in even when the hardware reports AVX-512 capability, because
	// AVX-512 reduces CPU clock frequency on some microarchitectures.
	simd?: {
		// allow_avx512 enables the AVX-512 SIMD tier when hardware is capable (per ADR-0043).
		// Defaults to false; set true only after confirming no frequency throttling on target hardware.
		allow_avx512?: bool | *false
	}
}
