//! `SubstrateService` — rmcp 1.7 `ServerHandler` implementation.
//!
//! This module wires the `ToolDispatcher` and substrate `Capabilities` to the
//! rmcp `ServerHandler` trait so that `tools/list`, `tools/call`, and
//! `initialize` requests are fully served over the STDIO transport.
//!
//! Per ADR-0005: `stdout` is sacred — no `println!` or `print!` may appear
//! in this module.  All diagnostic output goes to `stderr` via `tracing`.
//!
//! Per ADR-0013: capability advertisement is computed from the detected
//! `substrate_domain::Capabilities` snapshot built during the composition-root
//! wire phase.
//!
//! Per ADR-0040: `notifications/cancelled` from the client is intercepted in
//! `on_cancelled` and mapped to `ToolDispatcher.jobs.cancel(job_id)`.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::{collections::BTreeMap, sync::Arc};

use rmcp::{
    ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, CancelTaskParams, CancelTaskResult, Content,
        CreateTaskResult, ErrorData as McpErrorData, GetTaskInfoParams, GetTaskPayloadResult,
        GetTaskResult, GetTaskResultParams, Implementation, InitializeRequestParams,
        InitializeResult, ListTasksResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Task, TaskStatus, TasksCapability, Tool,
    },
    service::{NotificationContext, RequestContext, RoleServer},
};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{
    Capabilities, ClientId, JobState, SubstrateError, jobs::entry::JobEntry,
    ports::job_registry::JobResult, value_objects::JobId,
};

use super::{
    dispatcher::ToolDispatcher,
    initialize::{
        PROTOCOL_VERSION_PREFERRED, SERVER_NAME, SERVER_VERSION, build_experimental_capabilities,
        negotiate_version,
    },
    rmcp_progress_notifier::RmcpPeerNotifier,
};

// ---- Tool registry ----------------------------------------------------------

/// Returns a helper function that generates JSON Schema for a type via rmcp's
/// built-in `schema_for_type` (JSON Schema 2020-12, draft2020-12 settings).
///
/// Used inside `tool_registry` to generate real per-tool input schemas (Task A).
fn schema_for<T: schemars::JsonSchema + 'static>()
-> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    rmcp::handler::server::common::schema_for_type::<T>()
}

/// Returns the empty-object schema for tools that accept no parameters
/// (all `sys_*` tools take no caller-supplied arguments per current spec).
fn schema_empty() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    rmcp::handler::server::common::schema_for_empty_input()
}

/// Converts a `serde_json::Value::Object` into the `Arc<Map>` form rmcp expects
/// for `Tool::new`'s input schema parameter.
#[cfg(feature = "subprocess")]
fn schema_from_json(
    value: serde_json::Value,
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    let map = value
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    std::sync::Arc::new(map)
}

#[cfg(feature = "subprocess")]
fn schema_subprocess_spawn() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    schema_from_json(serde_json::json!({
        "type": "object",
        "required": ["binary_path", "cwd", "stdin_kind", "capture_kind", "elicitation_confirmed"],
        "properties": {
            "binary_path": { "type": "string", "description": "Absolute path to allowlisted binary." },
            "args": { "type": "array", "items": { "type": "string" }, "default": [] },
            "env_allowlist": { "type": "array", "items": { "type": "string" }, "default": [] },
            "env_override": {
                "type": "object",
                "additionalProperties": { "type": "string" },
                "default": {}
            },
            "cwd": { "type": "string", "description": "Absolute working directory inside PathJail." },
            "stdin_kind": {
                "oneOf": [
                    { "type": "string", "enum": ["none", "piped"] },
                    {
                        "type": "object",
                        "required": ["file_path"],
                        "properties": { "file_path": { "type": "string" } }
                    }
                ]
            },
            "capture_kind": { "type": "string", "enum": ["stream", "in_memory", "tmp_file"] },
            "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 86400 },
            "idempotency_key": { "type": "string", "description": "Optional UUIDv7 in Crockford base32 (26 chars) or hyphenated UUID." },
            "elicitation_confirmed": { "type": "boolean", "description": "Must be true. ADR-0052 requires explicit confirmation for every spawn." },
            "name": {
                "type": "string",
                "pattern": "^[a-z0-9-]{1,64}$",
                "description": "ADR-0056 supervisor alias. Idempotent re-spawn: spawn with existing non-terminal name returns the existing handle."
            },
            "restart_policy": {
                "oneOf": [
                    { "type": "object", "required": ["kind"], "properties": { "kind": { "const": "Never" } } },
                    {
                        "type": "object",
                        "required": ["kind", "max_retries", "backoff_ms"],
                        "properties": {
                            "kind": { "const": "OnFailure" },
                            "max_retries": { "type": "integer", "minimum": 1, "maximum": 100 },
                            "backoff_ms": { "type": "integer", "minimum": 100, "maximum": 300000 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "backoff_ms"],
                        "properties": {
                            "kind": { "const": "Always" },
                            "backoff_ms": { "type": "integer", "minimum": 100, "maximum": 300000 }
                        }
                    }
                ],
                "description": "ADR-0056 restart policy controlling supervisor re-spawn on child exit."
            },
            "health_probe": {
                "oneOf": [
                    { "type": "object", "required": ["kind"], "properties": { "kind": { "const": "None" } } },
                    {
                        "type": "object",
                        "required": ["kind", "url", "expected_status", "interval_ms", "startup_grace_ms"],
                        "properties": {
                            "kind": { "const": "HttpGet" },
                            "url": { "type": "string", "pattern": "^https?://" },
                            "expected_status": { "type": "integer", "minimum": 100, "maximum": 599 },
                            "interval_ms": { "type": "integer", "minimum": 100, "maximum": 60000 },
                            "startup_grace_ms": { "type": "integer", "minimum": 0, "maximum": 600000 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "host", "port", "interval_ms", "startup_grace_ms"],
                        "properties": {
                            "kind": { "const": "PortOpen" },
                            "host": { "type": "string" },
                            "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                            "interval_ms": { "type": "integer", "minimum": 100, "maximum": 60000 },
                            "startup_grace_ms": { "type": "integer", "minimum": 0, "maximum": 600000 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "regex", "timeout_ms"],
                        "properties": {
                            "kind": { "const": "LogPattern" },
                            "regex": { "type": "string", "minLength": 1 },
                            "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 600000 }
                        }
                    }
                ],
                "description": "ADR-0056 health probe gating Starting -> Ready transition. None = Ready immediately."
            },
            "log_rotation": {
                "oneOf": [
                    { "type": "object", "required": ["kind"], "properties": { "kind": { "const": "None" } } },
                    {
                        "type": "object",
                        "required": ["kind", "max_bytes_per_file", "keep_files"],
                        "properties": {
                            "kind": { "const": "BySize" },
                            "max_bytes_per_file": { "type": "integer", "minimum": 1048576, "maximum": 1073741824 },
                            "keep_files": { "type": "integer", "minimum": 1, "maximum": 20 }
                        }
                    }
                ],
                "description": "ADR-0056 log rotation for capture_kind=tmp_file. Cumulative cap = max_bytes_per_file * keep_files."
            }
        },
        "additionalProperties": false
    }))
}

#[cfg(feature = "subprocess")]
fn schema_subprocess_list() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    schema_from_json(serde_json::json!({
        "type": "object",
        "properties": {
            "state_filter": {
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": ["pending", "starting", "running", "ready", "restarting", "succeeded", "failed", "cancelled", "timed_out", "killed"]
                }
            },
            "page_cursor": { "type": "string" },
            "page_size": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
            "client_id": { "type": "string" }
        },
        "additionalProperties": false
    }))
}

#[cfg(feature = "subprocess")]
fn schema_subprocess_cancel() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    schema_from_json(serde_json::json!({
        "type": "object",
        "required": ["job_id"],
        "properties": {
            "job_id": { "type": "string", "description": "UUIDv7 in Crockford base32 (26 chars) or hyphenated UUID." },
            "force": { "type": "boolean", "default": false, "description": "Skip SIGTERM drain and send SIGKILL immediately." }
        },
        "additionalProperties": false
    }))
}

#[cfg(feature = "subprocess")]
fn schema_subprocess_result() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    schema_from_json(serde_json::json!({
        "type": "object",
        "required": ["job_id"],
        "properties": {
            "job_id": { "type": "string" },
            "wait_ms": { "type": "integer", "minimum": 0, "default": 0, "description": "Long-poll timeout in milliseconds." },
            "include_aggregates": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    }))
}

#[cfg(feature = "subprocess")]
fn schema_subprocess_signal() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    schema_from_json(serde_json::json!({
        "type": "object",
        "required": ["job_id", "signal"],
        "properties": {
            "job_id": { "type": "string" },
            "signal": {
                "type": "string",
                "enum": ["SIGHUP", "SIGINT", "SIGTERM", "SIGKILL", "SIGSTOP", "SIGCONT", "SIGUSR1", "SIGUSR2"]
            },
            "target": { "type": "string", "enum": ["process", "process_group"], "default": "process_group" },
            "elicitation_confirmed": { "type": "boolean", "default": false, "description": "Required true for SIGKILL/SIGTERM/SIGSTOP." }
        },
        "additionalProperties": false
    }))
}

/// Thin MCP tool descriptions per ADR-0007 amendment 2026-05-22 (MCP + skill synergy).
///
/// Each description is <= 100 chars. Full lookup reference (buckets, errors,
/// config, rules, skip routing) lives in the companion `substrate` skill at
/// `~/.claude/skills/substrate/SKILL.md`, auto-primed via the `mcp__substrate__`
/// trigger family. JSON-Schema for args ships via `inputSchema` (schemars-derived).
mod descriptions {
    // ---- filesystem-query ---------------------------------------------------

    pub(super) const fn fs_read_dir() -> &'static str {
        "List immediate children of a directory, paginated. See substrate skill."
    }

    pub(super) const fn fs_stat() -> &'static str {
        "Return metadata for a path (lstat — does not follow symlinks). See substrate skill."
    }

    pub(super) const fn fs_find() -> &'static str {
        "Recursive file walk by glob/mtime/kind, paginated. See substrate skill."
    }

    pub(super) const fn fs_read() -> &'static str {
        "Read file content as text or base64; large files promote to async job. See substrate skill."
    }

    pub(super) const fn fs_hash() -> &'static str {
        "Hash a file (blake3/sha256); large files return job_id. See substrate skill."
    }

    // ---- filesystem-mutation ------------------------------------------------

    pub(super) const fn fs_mkdir() -> &'static str {
        "Create a directory tree. Dry-run by default. See substrate skill."
    }

    pub(super) const fn fs_write() -> &'static str {
        "Write text or base64 bytes to a file atomically. Dry-run by default. See substrate skill."
    }

    pub(super) const fn fs_copy() -> &'static str {
        "Copy a file; large files return job_id. Dry-run by default. See substrate skill."
    }

    pub(super) const fn fs_rename() -> &'static str {
        "Rename or move a file/directory. Dry-run by default. See substrate skill."
    }

    pub(super) const fn fs_remove() -> &'static str {
        "Delete file or directory tree. Destructive — needs elicitation_confirmed. See substrate skill."
    }

    pub(super) const fn fs_set_permissions() -> &'static str {
        "Change POSIX permissions. Destructive — needs elicitation_confirmed. See substrate skill."
    }

    pub(super) const fn fs_symlink() -> &'static str {
        "Create a symbolic link. Dry-run by default. See substrate skill."
    }

    pub(super) const fn fs_touch() -> &'static str {
        "Create an empty file or update its timestamps. See substrate skill."
    }

    // ---- process ------------------------------------------------------------

    pub(super) const fn proc_list() -> &'static str {
        "List running processes with optional filters, paginated. See substrate skill."
    }

    pub(super) const fn proc_tree() -> &'static str {
        "Return process hierarchy rooted at a PID. See substrate skill."
    }

    pub(super) const fn proc_signal() -> &'static str {
        "Deliver POSIX signal to PID. KILL/TERM/STOP need elicitation_confirmed. See substrate skill."
    }

    // ---- system-info --------------------------------------------------------

    pub(super) const fn sys_uname() -> &'static str {
        "Return OS name, kernel version, and architecture. See substrate skill."
    }

    pub(super) const fn sys_hostname() -> &'static str {
        "Return system hostname. See substrate skill."
    }

    pub(super) const fn sys_uptime() -> &'static str {
        "Return system uptime and boot timestamp. See substrate skill."
    }

    pub(super) const fn sys_df() -> &'static str {
        "Return disk usage for all mounted volumes. See substrate skill."
    }

    pub(super) const fn sys_load_average() -> &'static str {
        "Return CPU load averages (1m, 5m, 15m). See substrate skill."
    }

    pub(super) const fn sys_info() -> &'static str {
        "Return one-shot snapshot of OS, memory, CPU, and disk. See substrate skill."
    }

    // ---- text-processing ----------------------------------------------------

    pub(super) const fn text_search() -> &'static str {
        "Search regex in a file, paginated; large files promote to async job. See substrate skill."
    }

    pub(super) const fn text_count_lines() -> &'static str {
        "Count lines and bytes in a file; large files return job_id. See substrate skill."
    }

    pub(super) const fn text_head() -> &'static str {
        "Read the first N lines of a text file. See substrate skill."
    }

    pub(super) const fn text_tail() -> &'static str {
        "Read the last N lines of a text file. See substrate skill."
    }

    // ---- archive ------------------------------------------------------------

    pub(super) const fn archive_tar_create() -> &'static str {
        "Create TAR archive (optional gzip). Always async — returns job_id. See substrate skill."
    }

    pub(super) const fn archive_tar_extract() -> &'static str {
        "Extract TAR/TAR.GZ archive. Always async — returns job_id. See substrate skill."
    }

    pub(super) const fn archive_zip_create() -> &'static str {
        "Create ZIP archive. Always async — returns job_id. See substrate skill."
    }

    pub(super) const fn archive_zip_extract() -> &'static str {
        "Extract ZIP archive. Always async — returns job_id. See substrate skill."
    }

    pub(super) const fn archive_gzip_compress() -> &'static str {
        "Gzip-compress a file; large files return job_id. See substrate skill."
    }

    pub(super) const fn archive_gzip_decompress() -> &'static str {
        "Gzip-decompress a .gz file; large files return job_id. See substrate skill."
    }

    pub(super) const fn archive_hash() -> &'static str {
        "Hash an archive file (blake3/sha256); large files return job_id. See substrate skill."
    }

    // ---- job control-plane --------------------------------------------------

    pub(super) const fn job_status() -> &'static str {
        "Snapshot job state + progress by job_id. See substrate skill."
    }

    pub(super) const fn job_result() -> &'static str {
        "Retrieve terminal result of a completed job (optional long-poll). See substrate skill."
    }

    pub(super) const fn job_cancel() -> &'static str {
        "Cancel an in-flight async job; idempotent on terminal jobs. See substrate skill."
    }

    pub(super) const fn job_list() -> &'static str {
        "List async jobs for the current session, paginated. See substrate skill."
    }

    // ---- subprocess (feature-gated) -----------------------------------------

    #[cfg(feature = "subprocess")]
    pub(super) const fn subprocess_spawn() -> &'static str {
        "Spawn supervised child process from allowlisted binary. Destructive — needs elicitation_confirmed. See substrate skill."
    }

    #[cfg(feature = "subprocess")]
    pub(super) const fn subprocess_list() -> &'static str {
        "List live subprocess handles for the current client, paginated. See substrate skill."
    }

    #[cfg(feature = "subprocess")]
    pub(super) const fn subprocess_cancel() -> &'static str {
        "Cancel a running subprocess (SIGTERM drain → SIGKILL). See substrate skill."
    }

    #[cfg(feature = "subprocess")]
    pub(super) const fn subprocess_result() -> &'static str {
        "Retrieve terminal result, exit code, and captured output of a subprocess. See substrate skill."
    }

    #[cfg(feature = "subprocess")]
    pub(super) const fn subprocess_signal() -> &'static str {
        "Send POSIX signal to subprocess or its process group. KILL/TERM/STOP need elicitation_confirmed. See substrate skill."
    }
}

// Job control-plane request types used for schemars schema generation (Task A).
// Defined here because they are used only in the server layer, not in substrate-jobs.

/// Input parameters for `job_status`.
///
/// Uses plain `String` for `job_id` so `schemars::JsonSchema` can be derived
/// without pulling `schemars` into `substrate-domain` (which must stay infra-free).
/// Fields are read only by the schemars derive macro, not by application code.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "fields exist for schemars schema generation only")]
struct JobStatusRequest {
    /// `UUIDv7` job identifier — Crockford base32, 26 ASCII chars (e.g. `01HN8XKZR4…`).
    job_id: String,
}

/// Input parameters for `job_result`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "fields exist for schemars schema generation only")]
struct JobResultRequest {
    /// `UUIDv7` job identifier — Crockford base32, 26 ASCII chars.
    job_id: String,
    /// Optional long-poll timeout in milliseconds (capped by server config).
    #[serde(default)]
    wait_ms: Option<u64>,
}

/// Input parameters for `job_cancel`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "fields exist for schemars schema generation only")]
struct JobCancelRequest {
    /// `UUIDv7` job identifier — Crockford base32, 26 ASCII chars.
    job_id: String,
}

/// Input parameters for `job_list`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "fields exist for schemars schema generation only")]
struct JobListRequest {
    /// Opaque pagination cursor from a previous `job_list` response.
    #[serde(default)]
    cursor: Option<String>,
}

/// Returns the static list of all 37 substrate tools.
///
/// Each entry carries a thin description (<= 100 chars) plus a schemars-derived
/// `inputSchema`. The companion `substrate` skill body at
/// `~/.claude/skills/substrate/SKILL.md` holds the full reference (buckets,
/// errors, config, rules, skip routing), auto-primed via the `mcp__substrate__`
/// trigger family.
#[must_use]
pub(crate) fn tool_registry() -> Vec<Tool> {
    /// Builds a tool entry with a real JSON Schema from type `T` and a narrative-arc description.
    fn make<T: schemars::JsonSchema + 'static>(name: &'static str, desc: &'static str) -> Tool {
        Tool::new(name, desc, schema_for::<T>())
    }

    vec![
        // ---- filesystem-query BC (read-side) ---------------------------------
        make::<substrate_fs_query::read_dir::FsReadDirRequest>(
            "fs_read_dir",
            descriptions::fs_read_dir(),
        ),
        make::<substrate_fs_query::stat::FsStatRequest>("fs_stat", descriptions::fs_stat()),
        make::<substrate_fs_query::find::FsFindRequest>("fs_find", descriptions::fs_find()),
        make::<substrate_fs_query::read::FsReadRequest>("fs_read", descriptions::fs_read()),
        make::<substrate_fs_query::hash::FsHashRequest>("fs_hash", descriptions::fs_hash()),
        // ---- filesystem-mutation BC (write-side) -----------------------------
        make::<substrate_fs_mutation::mkdir::FsMkdirRequest>("fs_mkdir", descriptions::fs_mkdir()),
        make::<substrate_fs_mutation::write::FsWriteRequest>("fs_write", descriptions::fs_write()),
        make::<substrate_fs_mutation::copy::FsCopyRequest>("fs_copy", descriptions::fs_copy()),
        make::<substrate_fs_mutation::rename::FsRenameRequest>(
            "fs_rename",
            descriptions::fs_rename(),
        ),
        make::<substrate_fs_mutation::remove::FsRemoveRequest>(
            "fs_remove",
            descriptions::fs_remove(),
        ),
        make::<substrate_fs_mutation::set_permissions::FsSetPermissionsRequest>(
            "fs_set_permissions",
            descriptions::fs_set_permissions(),
        ),
        make::<substrate_fs_mutation::symlink::FsSymlinkRequest>(
            "fs_symlink",
            descriptions::fs_symlink(),
        ),
        make::<substrate_fs_mutation::touch::FsTouchRequest>("fs_touch", descriptions::fs_touch()),
        // ---- process BC ------------------------------------------------------
        make::<substrate_process::list::ProcListRequest>("proc_list", descriptions::proc_list()),
        make::<substrate_process::tree::ProcTreeRequest>("proc_tree", descriptions::proc_tree()),
        make::<substrate_process::signal::ProcSignalRequest>(
            "proc_signal",
            descriptions::proc_signal(),
        ),
        // ---- system-info BC (no caller-supplied parameters) ------------------
        Tool::new("sys_uname", descriptions::sys_uname(), schema_empty()),
        Tool::new("sys_hostname", descriptions::sys_hostname(), schema_empty()),
        Tool::new("sys_uptime", descriptions::sys_uptime(), schema_empty()),
        Tool::new("sys_df", descriptions::sys_df(), schema_empty()),
        Tool::new(
            "sys_load_average",
            descriptions::sys_load_average(),
            schema_empty(),
        ),
        Tool::new("sys_info", descriptions::sys_info(), schema_empty()),
        // ---- text-processing BC ----------------------------------------------
        make::<substrate_text::search::SearchParams>("text_search", descriptions::text_search()),
        make::<substrate_text::count_lines::CountLinesParams>(
            "text_count_lines",
            descriptions::text_count_lines(),
        ),
        make::<substrate_text::head::HeadParams>("text_head", descriptions::text_head()),
        make::<substrate_text::tail::TailParams>("text_tail", descriptions::text_tail()),
        // ---- archive BC ------------------------------------------------------
        make::<substrate_archive::tar_create::TarCreateRequest>(
            "archive_tar_create",
            descriptions::archive_tar_create(),
        ),
        make::<substrate_archive::tar_extract::TarExtractRequest>(
            "archive_tar_extract",
            descriptions::archive_tar_extract(),
        ),
        make::<substrate_archive::zip_create::ZipCreateRequest>(
            "archive_zip_create",
            descriptions::archive_zip_create(),
        ),
        make::<substrate_archive::zip_extract::ZipExtractRequest>(
            "archive_zip_extract",
            descriptions::archive_zip_extract(),
        ),
        make::<substrate_archive::gzip_compress::GzipCompressRequest>(
            "archive_gzip_compress",
            descriptions::archive_gzip_compress(),
        ),
        make::<substrate_archive::gzip_decompress::GzipDecompressRequest>(
            "archive_gzip_decompress",
            descriptions::archive_gzip_decompress(),
        ),
        make::<substrate_archive::hash::ArchiveHashRequest>(
            "archive_hash",
            descriptions::archive_hash(),
        ),
        // ---- job control-plane -----------------------------------------------
        make::<JobStatusRequest>("job_status", descriptions::job_status()),
        make::<JobResultRequest>("job_result", descriptions::job_result()),
        make::<JobCancelRequest>("job_cancel", descriptions::job_cancel()),
        make::<JobListRequest>("job_list", descriptions::job_list()),
        // ---- subprocess BC (feature-gated) -----------------------------------
        #[cfg(feature = "subprocess")]
        Tool::new(
            "subprocess_spawn",
            descriptions::subprocess_spawn(),
            schema_subprocess_spawn(),
        ),
        #[cfg(feature = "subprocess")]
        Tool::new(
            "subprocess_list",
            descriptions::subprocess_list(),
            schema_subprocess_list(),
        ),
        #[cfg(feature = "subprocess")]
        Tool::new(
            "subprocess_cancel",
            descriptions::subprocess_cancel(),
            schema_subprocess_cancel(),
        ),
        #[cfg(feature = "subprocess")]
        Tool::new(
            "subprocess_result",
            descriptions::subprocess_result(),
            schema_subprocess_result(),
        ),
        #[cfg(feature = "subprocess")]
        Tool::new(
            "subprocess_signal",
            descriptions::subprocess_signal(),
            schema_subprocess_signal(),
        ),
    ]
}

// ---- SubstrateService -------------------------------------------------------

/// rmcp `ServerHandler` that delegates requests to the [`ToolDispatcher`].
///
/// `SubstrateService` is `Clone` because rmcp 1.7 may clone the handler for
/// concurrent request processing on some transport configurations. Each clone
/// shares the same `Arc<ToolDispatcher>` and `Arc<Capabilities>`, so no state
/// is duplicated.
///
/// # Thread safety
///
/// `ToolDispatcher` is `Send + Sync` (all inner `Arc<dyn Port>` are `Send + Sync`).
/// `Capabilities` is `Clone + Send + Sync`.  `SubstrateService` therefore satisfies
/// `ServerHandler: Send + Sync + 'static`.
#[derive(Clone)]
pub(crate) struct SubstrateService {
    /// Central tool dispatcher — routes `tools/call` to the correct adapter.
    pub(crate) dispatcher: Arc<ToolDispatcher>,

    /// Detected runtime capabilities used to build the `initialize` response.
    caps: Arc<Capabilities>,

    /// Root shutdown token; cancelled by SIGTERM/SIGINT.
    shutdown_token: CancellationToken,

    /// Whether the async job control-plane is wired (exposed in experimental caps).
    jobs_wired: bool,

    /// Late-bound progress notifier shared with `InMemoryJobRegistry`.
    ///
    /// `initialize` calls `set_peer` so that subsequent job progress events
    /// are forwarded to the connected client via `notifications/progress`.
    notifier: Arc<RmcpPeerNotifier>,
}

impl std::fmt::Debug for SubstrateService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubstrateService")
            .field("jobs_wired", &self.jobs_wired)
            .field("notifier", &self.notifier)
            .finish_non_exhaustive()
    }
}

impl SubstrateService {
    /// Creates a new `SubstrateService`.
    pub(crate) const fn new(
        dispatcher: Arc<ToolDispatcher>,
        caps: Arc<Capabilities>,
        shutdown_token: CancellationToken,
        jobs_wired: bool,
        notifier: Arc<RmcpPeerNotifier>,
    ) -> Self {
        Self {
            dispatcher,
            caps,
            shutdown_token,
            jobs_wired,
            notifier,
        }
    }

    /// Converts a `DispatchedResponse` into the rmcp `CallToolResult` envelope.
    ///
    /// Uses `CallToolResult::structured` / `CallToolResult::structured_error` so
    /// both `content` (model text) and `structured_content` (JSON) are present per
    /// ADR-0007.  `is_error` is set according to the caller-supplied flag.
    fn into_call_tool_result(
        dispatched: super::dispatcher::DispatchedResponse,
        is_error: bool,
    ) -> CallToolResult {
        // Build the combined structured value: the dispatched structured_content
        // plus a `_text` field for the model-oriented summary.
        let mut combined = dispatched.structured_content.clone();
        if let Value::Object(ref mut map) = combined {
            map.insert(
                "_text".to_owned(),
                Value::String(dispatched.content.clone()),
            );
        }

        let base = if is_error {
            CallToolResult::structured_error(combined)
        } else {
            CallToolResult::structured(combined)
        };

        // Override the auto-generated content with our ADR-0007 model text.
        let mut result = base;
        result.content = vec![Content::text(dispatched.content)];
        result
    }

    /// Converts a `SubstrateError` into an error `CallToolResult` per ADR-0010.
    ///
    /// Structured content shape (asserted by cucumber error-envelope steps):
    /// ```json
    /// {
    ///   "error": {
    ///     "code": "SUBSTRATE_*",
    ///     "message": "...",
    ///     "recovery_hint": "...",
    ///     "correlation_id": "<uuidv7>"
    ///   }
    /// }
    /// ```
    ///
    /// Flat root fields (`code`, `message`, `recovery_hint`) are retained for
    /// backward-compat with step paths that inspect root-level keys.
    /// The `data` sub-object mirrors the JSON-RPC `error.data` shape (ADR-0010).
    fn error_result(err: &SubstrateError) -> CallToolResult {
        // Always emit a non-empty correlation_id so Gherkin steps that assert
        // the UUIDv7 pattern pass even when the domain error was constructed
        // without one (e.g. adapters that set correlation_id: None).
        let correlation_id_str = err.correlation_id().map_or_else(
            || uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext)).to_string(),
            |u| u.to_string(),
        );

        // `offending_field` is present only for `SUBSTRATE_INVALID_ARGUMENT` so
        // tool-unknown-argument feature steps can assert `error.offending_field`.
        let offending_field_val = if let SubstrateError::InvalidArgument {
            offending_field, ..
        } = err
        {
            serde_json::Value::String(offending_field.clone())
        } else {
            serde_json::Value::Null
        };

        let structured = serde_json::json!({
            // Flat root fields (backward-compat with root-level assertions)
            "code": err.code(),
            "message": err.to_string(),
            "recovery_hint": err.recovery_hint(),
            // Nested `error` object — primary path for cucumber assertions:
            // result.structuredContent.error.{code,recovery_hint,correlation_id}
            "error": {
                "code": err.code(),
                "message": err.to_string(),
                "recovery_hint": err.recovery_hint(),
                "correlation_id": correlation_id_str,
                "offending_field": offending_field_val,
            },
            // `data` sub-object mirrors JSON-RPC error.data shape (ADR-0010)
            "data": {
                "code": err.code(),
                "message": err.to_string(),
                "recovery_hint": err.recovery_hint(),
                "correlation_id": err.correlation_id().map(|u| u.to_string()),
            },
        });
        let mut result = CallToolResult::structured_error(structured);
        result.content = vec![Content::text(format!(
            "Error {}: {}. Hint: {}",
            err.code(),
            err,
            err.recovery_hint(),
        ))];
        result
    }

    /// Builds the `ServerCapabilities` struct including the experimental substrate block.
    fn build_server_capabilities(&self) -> ServerCapabilities {
        let experimental_value = build_experimental_capabilities(&self.caps, self.jobs_wired);

        let experimental: Option<BTreeMap<String, Map<String, Value>>> =
            if let Value::Object(obj) = experimental_value {
                let btree: BTreeMap<String, Map<String, Value>> = obj
                    .into_iter()
                    .filter_map(|(k, v)| {
                        if let Value::Object(inner) = v {
                            Some((k, inner))
                        } else {
                            None
                        }
                    })
                    .collect();
                if btree.is_empty() { None } else { Some(btree) }
            } else {
                None
            };

        // `TasksCapability::server_default()` advertises `list`, `cancel`, and
        // `requests.tools.call` per SEP-1686 / ADR-0048.
        let mut caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_tasks_with(TasksCapability::server_default())
            .build();

        // rmcp 1.7 does not expose a capability builder method for `elicitation`
        // on `ServerCapabilities` — elicitation is a CLIENT-side capability
        // (the server requests elicitation; clients advertise the ability to
        // respond). The server sends `elicitation/create` requests; it does not
        // need to declare a corresponding `ServerCapabilities.elicitation` field.
        //
        // rmcp 1.7 does not expose capability builder methods for
        // `structured_content` or `output_schema` on `ServerCapabilities`.
        // These are result-level fields in `CallToolResult`, not capability
        // advertised in `initialize`. No builder method exists.

        // Wire the experimental substrate block if non-empty.
        if let Some(exp) = experimental {
            caps.experimental = Some(exp);
        }
        caps
    }

    // ---- Tasks primitive helpers -------------------------------------------

    /// Maps a `JobState` (domain) to the closest `TaskStatus` (MCP SEP-1686).
    const fn job_state_to_task_status(state: JobState) -> TaskStatus {
        match state {
            JobState::Pending | JobState::Running => TaskStatus::Working,
            JobState::Succeeded => TaskStatus::Completed,
            // `TimedOut` is a terminal failure in substrate's domain model; both
            // map to SEP-1686 `Failed`.
            JobState::Failed | JobState::TimedOut => TaskStatus::Failed,
            JobState::Cancelled => TaskStatus::Cancelled,
        }
    }

    /// Converts a `JobEntry` snapshot into an rmcp `Task` value object.
    fn job_entry_to_task(entry: &JobEntry) -> Task {
        use time::format_description::well_known::Rfc3339;

        let created_iso = entry
            .started_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| entry.started_at.to_string());
        let updated_iso = entry
            .updated_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| entry.updated_at.to_string());

        let mut task = Task::new(
            entry.id.to_crockford(),
            Self::job_state_to_task_status(entry.state),
            created_iso,
            updated_iso,
        );
        if let Some(msg) = &entry.message {
            task = task.with_status_message(msg.clone());
        }
        task
    }

    /// Extracts the `ClientId` from the request context, falling back to "anonymous".
    fn client_id_from_context(context: &RequestContext<RoleServer>) -> ClientId {
        context
            .peer
            .peer_info()
            .and_then(|info| {
                let raw = format!("{}-{}", info.client_info.name, info.client_info.version);
                let sanitised: String = raw
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .take(64)
                    .collect();
                ClientId::parse(sanitised).ok()
            })
            .unwrap_or_else(|| {
                #[expect(
                    clippy::expect_used,
                    reason = "'anonymous' satisfies ClientId invariants; this is infallible"
                )]
                ClientId::parse("anonymous").expect("'anonymous' is a valid ClientId")
            })
    }
}

impl ServerHandler for SubstrateService {
    /// Handles `initialize` — returns negotiated protocol version + capability advertisement.
    ///
    /// Protocol version negotiation follows ADR-0013 semantics via `negotiate_version`.
    /// After negotiation succeeds, the `Peer<RoleServer>` from the request context is
    /// injected into `RmcpPeerNotifier` so that subsequent job progress events are
    /// forwarded to the client via `notifications/progress` per ADR-0040.
    #[instrument(skip(self, context), fields(client_version = %request.protocol_version))]
    fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<InitializeResult, McpErrorData>> + Send + '_ {
        async move {
            let negotiated_str = match negotiate_version(request.protocol_version.as_str()) {
                super::initialize::NegotiatedVersion::BelowMinimum => {
                    let correlation_id =
                        uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext)).to_string();
                    tracing::warn!(
                        client_version = %request.protocol_version,
                        %correlation_id,
                        "client protocol version below minimum — rejecting"
                    );
                    // Embed structured `data` so step assertions on
                    // `error.data.code` / `error.data.recovery_hint` pass
                    // (ADR-0010 + error-response-shape.feature).
                    let data = serde_json::json!({
                        "code": "SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED",
                        "recovery_hint": substrate_domain::SubstrateError::ProtocolVersionUnsupported {
                            version: request.protocol_version.to_string(),
                            correlation_id: None,
                        }.recovery_hint(),
                        "correlation_id": correlation_id,
                    });
                    return Err(McpErrorData::invalid_request(
                        format!(
                            "unsupported protocol version: {}. Minimum supported: {}",
                            request.protocol_version,
                            super::initialize::PROTOCOL_VERSION_MINIMUM
                        ),
                        Some(data),
                    ));
                },
                super::initialize::NegotiatedVersion::Accepted(v) => v,
            };

            let protocol_version = if negotiated_str == PROTOCOL_VERSION_PREFERRED {
                ProtocolVersion::V_2025_11_25
            } else {
                ProtocolVersion::V_2025_06_18
            };

            // Bind the live peer so progress notifications flow to this client.
            self.notifier.set_peer(context.peer.clone());
            tracing::debug!("progress notifier peer bound after initialize");

            let capabilities = self.build_server_capabilities();
            let server_info = Implementation::new(SERVER_NAME, SERVER_VERSION);

            let result = InitializeResult::new(capabilities)
                .with_protocol_version(protocol_version)
                .with_server_info(server_info);

            tracing::info!(
                negotiated = %result.protocol_version,
                "initialize accepted"
            );
            Ok(result)
        }
    }

    /// Handles `tools/list` — returns the static tool registry (38 tools, single page).
    ///
    /// Pagination cursor is accepted but ignored; all tools fit in one response.
    #[instrument(skip(self, _request, _context))]
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpErrorData>> + Send + '_ {
        async move {
            let tools = tool_registry();
            tracing::debug!(count = tools.len(), "tools/list served");
            Ok(ListToolsResult::with_all_items(tools))
        }
    }

    /// Handles `tools/call` — dispatches to `ToolDispatcher`.
    ///
    /// The `RequestContext.ct` (per-request `CancellationToken`) is forwarded to
    /// `dispatcher.dispatch` so inline handlers respect request-level cancellation.
    /// The global `shutdown_token` is also wired so that a SIGTERM propagates to
    /// all in-flight tool calls per ADR-0037.
    #[instrument(skip(self, context), fields(tool = %request.name))]
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpErrorData>> + Send + '_ {
        async move {
            // Build a per-request cancel token that fires on either the rmcp
            // per-request ct OR the global shutdown token (ADR-0037).
            let per_request_cancel = context.ct.child_token();

            // Forward global shutdown → per-request cancel in a background task.
            // The task is detached; it exits as soon as either token fires.
            let shutdown_child = self.shutdown_token.child_token();
            let cancel_fwd = per_request_cancel.clone();
            tokio::spawn(async move {
                shutdown_child.cancelled().await;
                cancel_fwd.cancel();
            });

            // Build `ClientId` from peer info or fall back to a well-known sentinel.
            //
            // `ClientId::parse` validates the string per ADR-0040 (alphanumeric +
            // hyphens + underscores, 1–64 chars).  When the client-supplied name is
            // invalid we fall back to the "anonymous" sentinel rather than rejecting
            // the call.
            let client_id = context
                .peer
                .peer_info()
                .and_then(|info| {
                    // Sanitise: keep only alphanumeric, hyphens, and underscores.
                    let raw = format!("{}-{}", info.client_info.name, info.client_info.version);
                    let sanitised: String = raw
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '-' || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .take(64)
                        .collect();
                    ClientId::parse(sanitised).ok()
                })
                .unwrap_or_else(|| {
                    // "anonymous" is always valid per ClientId contract (alphanumeric, ≤64 chars).
                    #[expect(
                        clippy::expect_used,
                        reason = "'anonymous' satisfies ClientId invariants; this is infallible"
                    )]
                    ClientId::parse("anonymous").expect("'anonymous' is a valid ClientId")
                });

            // Deserialize arguments to `serde_json::Value`.
            let args = request
                .arguments
                .map_or_else(|| Value::Object(Map::new()), Value::Object);

            match self
                .dispatcher
                .dispatch(&request.name, args, per_request_cancel, client_id)
                .await
            {
                Ok(resp) => {
                    // ADR-0019 / ADR-0038: emit one structured audit_event line per
                    // tool invocation so log processors and cucumber tests can grep
                    // "audit_event" in stderr. Format: k=v pairs on a single line.
                    tracing::info!(
                        target: "substrate.audit",
                        audit_event = "tool_call",
                        tool = %request.name,
                        outcome = "ok",
                        "audit_event"
                    );
                    Ok(Self::into_call_tool_result(resp, false))
                },
                Err(err) => {
                    tracing::warn!(
                        tool = %request.name,
                        code = err.code(),
                        "tool dispatch error"
                    );
                    // ADR-0019 / ADR-0038: emit audit_event for error outcome too.
                    tracing::info!(
                        target: "substrate.audit",
                        audit_event = "tool_call",
                        tool = %request.name,
                        outcome = "error",
                        error_code = err.code(),
                        "audit_event"
                    );
                    // Surface as tool-level error (is_error=true) so agents can
                    // inspect the structured content without a JSON-RPC fault.
                    Ok(Self::error_result(&err))
                },
            }
        }
    }

    /// Handles `notifications/cancelled` — cancels the corresponding job per ADR-0040.
    ///
    /// `request_id` from the MCP cancelled notification maps to `progressToken == job_id`
    /// per the ADR-0040 triple-equality invariant.  Non-fatal: if the job is already
    /// terminal the cancel is silently discarded.
    #[instrument(skip(self, _context), fields(request_id = ?notification.request_id))]
    fn on_cancelled(
        &self,
        notification: rmcp::model::CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            let token_str = notification.request_id.to_string();
            tracing::debug!(token = %token_str, "notifications/cancelled received");

            if let Ok(job_id) = substrate_domain::value_objects::JobId::parse_crockford(&token_str)
            {
                match self.dispatcher.jobs.cancel(&job_id).await {
                    Ok(state) => {
                        tracing::info!(
                            job_id = %job_id,
                            state = ?state,
                            "job cancelled via notifications/cancelled"
                        );
                    },
                    Err(e) => {
                        // Non-fatal: job may already be terminal.
                        tracing::debug!(
                            job_id = %job_id,
                            error = %e,
                            "cancel for already-terminal or unknown job — ignored"
                        );
                    },
                }
            } else {
                tracing::debug!(
                    token = %token_str,
                    "notifications/cancelled: request_id is not a substrate job_id — ignored"
                );
            }
        }
    }

    // ---- MCP Tasks primitive (SEP-1686 / ADR-0048) -------------------------

    /// Handles `tasks/call` — enqueues a `tools/call` as an async job.
    ///
    /// Called by rmcp when a `tools/call` request carries a `task` field.
    /// Delegates to `ToolDispatcher.jobs.submit` via the existing job control-plane.
    /// Returns `CreateTaskResult { task }` with the initial `TaskStatus::Working`.
    #[instrument(skip(self, context), fields(tool = %request.name))]
    fn enqueue_task(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CreateTaskResult, McpErrorData>> + Send + '_ {
        async move {
            use substrate_domain::jobs::bucket::JobBucket;
            use substrate_domain::ports::job_registry::JobSubmitRequest;
            use time::OffsetDateTime;
            use time::format_description::well_known::Rfc3339;

            let client_id = Self::client_id_from_context(&context);
            let per_request_cancel = context.ct.child_token();
            let shutdown_child = self.shutdown_token.child_token();
            let cancel_fwd = per_request_cancel.clone();
            tokio::spawn(async move {
                shutdown_child.cancelled().await;
                cancel_fwd.cancel();
            });

            let args = request.arguments.clone().map_or_else(
                || serde_json::Value::Object(Map::new()),
                serde_json::Value::Object,
            );

            let tool_name = request.name.clone();
            let dispatcher = self.dispatcher.clone();
            let execute_cancel = per_request_cancel.clone();
            // Clone `client_id` for the execute closure; ownership goes into
            // the async block while the outer scope keeps the original for
            // `JobSubmitRequest`.
            let client_id_execute = client_id.clone();
            let execute = Box::pin(async move {
                dispatcher
                    .dispatch(&tool_name, args, execute_cancel, client_id_execute)
                    .await
                    .map(|resp| resp.structured_content)
            });

            let submit_req = JobSubmitRequest {
                client_id,
                tool: request.name.to_string(),
                // `enqueue_task` is always async; use C_always_async bucket.
                bucket: JobBucket::CAlwaysAsync,
                idempotency_key: None,
                args_json: request.arguments.map_or_else(
                    || serde_json::Value::Object(Map::new()),
                    serde_json::Value::Object,
                ),
                execute,
            };

            match self.dispatcher.jobs.submit(submit_req).await {
                Ok(job_id) => {
                    tracing::info!(
                        job_id = %job_id,
                        tool = %request.name,
                        "task enqueued"
                    );
                    // Build a minimal Task in `Working` state; the client polls
                    // `tasks/get` or `tasks/result` for terminal state.
                    let now = OffsetDateTime::now_utc()
                        .format(&Rfc3339)
                        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
                    let task =
                        Task::new(job_id.to_crockford(), TaskStatus::Working, now.clone(), now);
                    Ok(CreateTaskResult::new(task))
                },
                Err(err) => {
                    tracing::warn!(
                        tool = %request.name,
                        code = err.code(),
                        "enqueue_task submit error"
                    );
                    Err(McpErrorData::internal_error(
                        format!("job submit failed: {err}"),
                        None,
                    ))
                },
            }
        }
    }

    /// Handles `tasks/list` — delegates to `JobRegistryPort::list`.
    #[instrument(skip(self, _request, context))]
    fn list_tasks(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListTasksResult, McpErrorData>> + Send + '_ {
        async move {
            let client_id = Self::client_id_from_context(&context);
            match self.dispatcher.jobs.list(&client_id, None).await {
                Ok(page) => {
                    let tasks: Vec<Task> = page.jobs.iter().map(Self::job_entry_to_task).collect();
                    // Encode cursor bytes to UTF-8 string for the MCP wire format.
                    // The registry stores cursor payload as UTF-8 JSON bytes (see
                    // `encode_cursor` in `substrate-jobs`); converting to String is
                    // infallible for well-formed cursors.
                    // `ListTasksResult` is #[non_exhaustive]; use the named
                    // constructor then mutate the optional cursor field.
                    let mut result = ListTasksResult::new(tasks);
                    result.next_cursor = page.next_cursor.and_then(|c| {
                        // Cursor bytes are UTF-8 JSON produced by `encode_cursor`
                        // in substrate-jobs; conversion is infallible for valid cursors.
                        String::from_utf8(c.into_bytes()).ok()
                    });
                    Ok(result)
                },
                Err(err) => Err(McpErrorData::internal_error(
                    format!("list_tasks failed: {err}"),
                    None,
                )),
            }
        }
    }

    /// Handles `tasks/get` — delegates to `JobRegistryPort::status`.
    #[instrument(skip(self, _context), fields(task_id = %request.task_id))]
    fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskResult, McpErrorData>> + Send + '_ {
        async move {
            let job_id = JobId::parse_crockford(&request.task_id).map_err(|_| {
                McpErrorData::invalid_params(format!("invalid task_id: {}", request.task_id), None)
            })?;
            match self.dispatcher.jobs.status(&job_id).await {
                Ok(entry) => Ok(GetTaskResult {
                    meta: None,
                    task: Self::job_entry_to_task(&entry),
                }),
                Err(err) => Err(McpErrorData::internal_error(
                    format!("get_task_info failed: {err}"),
                    None,
                )),
            }
        }
    }

    /// Handles `tasks/result` — delegates to `JobRegistryPort::result` (long-poll until terminal).
    #[instrument(skip(self, _context), fields(task_id = %request.task_id))]
    fn get_task_result(
        &self,
        request: GetTaskResultParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskPayloadResult, McpErrorData>> + Send + '_
    {
        async move {
            let job_id = JobId::parse_crockford(&request.task_id).map_err(|_| {
                McpErrorData::invalid_params(format!("invalid task_id: {}", request.task_id), None)
            })?;
            // Long-poll with no timeout override; server-side cap applies.
            match self.dispatcher.jobs.result(&job_id, None).await {
                Ok(JobResult::Succeeded(v)) => Ok(GetTaskPayloadResult::new(v)),
                Ok(JobResult::Failed(e)) => Err(McpErrorData::internal_error(
                    format!("task failed: {e}"),
                    None,
                )),
                Ok(JobResult::Cancelled) => Err(McpErrorData::internal_error(
                    "task was cancelled".to_owned(),
                    None,
                )),
                Ok(JobResult::TimedOut) => Err(McpErrorData::internal_error(
                    "task timed out".to_owned(),
                    None,
                )),
                Err(err) => Err(McpErrorData::internal_error(
                    format!("get_task_result failed: {err}"),
                    None,
                )),
            }
        }
    }

    /// Handles `tasks/cancel` — delegates to `JobRegistryPort::cancel`.
    #[instrument(skip(self, _context), fields(task_id = %request.task_id))]
    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CancelTaskResult, McpErrorData>> + Send + '_ {
        async move {
            let job_id = JobId::parse_crockford(&request.task_id).map_err(|_| {
                McpErrorData::invalid_params(format!("invalid task_id: {}", request.task_id), None)
            })?;
            match self.dispatcher.jobs.status(&job_id).await {
                Ok(entry) => {
                    // Best-effort cancel; idempotent per JobRegistryPort contract.
                    let _ = self.dispatcher.jobs.cancel(&job_id).await;
                    // Re-fetch updated state for the response; fall back to the
                    // pre-cancel snapshot on error (idempotent cancel already fired).
                    let task = self.dispatcher.jobs.status(&job_id).await.map_or_else(
                        |_| Self::job_entry_to_task(&entry),
                        |updated| Self::job_entry_to_task(&updated),
                    );
                    Ok(CancelTaskResult { meta: None, task })
                },
                Err(err) => Err(McpErrorData::internal_error(
                    format!("cancel_task status check failed: {err}"),
                    None,
                )),
            }
        }
    }

    /// Returns the static server info used during rmcp's internal `get_info` calls.
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(self.build_server_capabilities())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION))
    }
}

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_registry_count() {
        // 5 fs-query + 8 fs-mutation + 3 process + 6 sys-info + 4 text +
        // 7 archive + 4 job = 37 base. +5 subprocess when feature enabled = 42.
        // The dispatch match arms in `dispatcher.rs` define the authoritative
        // count; this test pins parity between the registry and the dispatcher.
        let tools = tool_registry();
        let expected = if cfg!(feature = "subprocess") { 42 } else { 37 };
        assert_eq!(
            tools.len(),
            expected,
            "registry/dispatcher parity check failed: found {} tools, expected {expected}",
            tools.len()
        );
    }

    #[test]
    fn tool_names_are_unique() {
        let tools = tool_registry();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tool names detected");
    }

    #[test]
    fn all_tools_have_descriptions() {
        for tool in tool_registry() {
            assert!(
                tool.description.as_ref().is_some_and(|d| !d.is_empty()),
                "tool '{}' has no description",
                tool.name
            );
        }
    }
}
