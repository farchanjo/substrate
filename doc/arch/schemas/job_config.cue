// DDD role: ValueObject
package schemas

// Configuration surface of the async job control-plane: quotas, inline-promotion
// thresholds, and per-tool execution timeouts. Per ADR-0040.

// #JobWaitBudget is the long-poll budget pair of #JobQuotas: the hard ceiling and
// the substituted default for the wait_ms parameter of job.result. Embedded by
// #JobQuotas.
#JobWaitBudget: {
	// result_max_wait_ms caps the wait_ms parameter of job.result (long-poll ceiling).
	result_max_wait_ms: int & >=0 | *30000

	// result_default_wait_ms is the wait_ms substituted by the handler when the
	// caller omits the field per ADR-0059. Must satisfy 0 < default <= result_max_wait_ms.
	// An explicit wait_ms=0 in the request payload is honored as before; only the
	// "field absent" case is substituted by this default.
	result_default_wait_ms: int & >0 & <=result_max_wait_ms | *5000
}

// DDD role: ValueObject
// #JobQuotas configures the resource limits for the async job control-plane per ADR-0040.
// All fields have safe defaults; operators may override via TOML [jobs] section.
#JobQuotas: {
	// max_concurrent is the global limit on active (pending + running) jobs.
	max_concurrent: int & >=1 | *16

	// max_per_client is the per-client active job limit.
	max_per_client: int & >=1 | *4

	// result_ttl_secs is the retention period after terminal state entry.
	// After eviction, job.result and job.status return SUBSTRATE_JOB_NOT_FOUND.
	result_ttl_secs: int & >=1 | *300

	// Long-poll ceiling and substituted default for job.result.
	#JobWaitBudget

	// progress_interval_ms is the minimum emission interval between progress events.
	// Events are also suppressed unless progress delta >= 1 percentage point.
	progress_interval_ms: int & >=10 | *250

	// progress_channel_size is the bounded mpsc channel capacity per job.
	// Events submitted via try_send when full are dropped and counted.
	progress_channel_size: int & >=1 | *64

	// gc_interval_secs is the background GC wake interval for evicting expired jobs.
	gc_interval_secs: int & >=1 | *60
}

// #JobFsInlineThresholds is the filesystem-tool half of #JobInlineThresholds.
// Embedded by #JobInlineThresholds.
#JobFsInlineThresholds: {
	// fs_find_inline_entries: inline if the candidate count is below this value.
	fs_find_inline_entries: int & >=0 | *1000

	// fs_read_inline_bytes: inline if the file byte size is below this value.
	fs_read_inline_bytes: int & >=0 | *1048576

	// fs_hash_inline_bytes: inline if the input byte size is below this value.
	fs_hash_inline_bytes: int & >=0 | *4194304

	// fs_copy_inline_bytes: inline if the source file size is below this value.
	fs_copy_inline_bytes: int & >=0 | *1048576
}

// DDD role: ValueObject
// #JobInlineThresholds declares per-tool size thresholds for Bucket B auto-mode.
// A tool invocation below its threshold returns an inline result; at or above the
// threshold the tool is promoted to an async job per ADR-0040.
// Open struct: additional tool thresholds may be added without a schema amendment.
#JobInlineThresholds: {
	// Filesystem-tool thresholds.
	#JobFsInlineThresholds

	// text_search_inline_bytes: inline if the file byte size is below this value.
	text_search_inline_bytes: int & >=0 | *524288

	// text_count_lines_inline_bytes: inline if the file byte size is below this value.
	text_count_lines_inline_bytes: int & >=0 | *524288

	// archive_gzip_inline_bytes: inline if the uncompressed byte size is below this value.
	archive_gzip_inline_bytes: int & >=0 | *131072

	// archive_hash_inline_bytes: inline if the archive byte size is below this value.
	archive_hash_inline_bytes: int & >=0 | *4194304

	// Open: additional per-tool thresholds may be declared here without breaking existing configs.
	...
}

// DDD role: ValueObject
// #JobTimeouts configures per-tool execution time limits for async jobs per ADR-0040.
// Per-tool entries override the default. All values are in seconds.
#JobTimeouts: {
	// default_secs applies when no per-tool override is present.
	default_secs: int & >=1 | *600

	// archive_create_secs caps archive.tar.create and archive.zip.create jobs.
	archive_create_secs: int & >=1 | *1800

	// archive_extract_secs caps archive.tar.extract and archive.zip.extract jobs.
	archive_extract_secs: int & >=1 | *1800

	// fs_find_secs caps fs.find jobs promoted to Bucket C.
	fs_find_secs: int & >=1 | *60

	// fs_hash_secs caps fs.hash jobs in Bucket B or C.
	fs_hash_secs: int & >=1 | *600
}

// DDD role: ValueObject
// #JobConfig is the top-level configuration aggregate for the async job control-plane.
// It is embedded in the main RuntimeConfig under the [jobs] TOML section per ADR-0040.
#JobConfig: {
	// quotas configures resource limits (concurrency, TTL, channel sizes).
	quotas: #JobQuotas

	// inline_thresholds declares per-tool size thresholds for Bucket B auto-mode.
	inline_thresholds: #JobInlineThresholds

	// timeouts configures per-tool execution time limits.
	timeouts: #JobTimeouts
}