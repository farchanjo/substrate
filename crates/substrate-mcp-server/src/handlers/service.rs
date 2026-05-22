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
        CallToolRequestParams, CallToolResult, Content, ErrorData as McpErrorData, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::{NotificationContext, RequestContext, RoleServer},
};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{Capabilities, ClientId, SubstrateError};

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
fn schema_for<T: schemars::JsonSchema + 'static>(
) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    rmcp::handler::server::common::schema_for_type::<T>()
}

/// Returns the empty-object schema for tools that accept no parameters
/// (all `sys_*` tools take no caller-supplied arguments per current spec).
fn schema_empty() -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    rmcp::handler::server::common::schema_for_empty_input()
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
            "fs_read_dir", descriptions::fs_read_dir(),
        ),
        make::<substrate_fs_query::stat::FsStatRequest>(
            "fs_stat", descriptions::fs_stat(),
        ),
        make::<substrate_fs_query::find::FsFindRequest>(
            "fs_find", descriptions::fs_find(),
        ),
        make::<substrate_fs_query::read::FsReadRequest>(
            "fs_read", descriptions::fs_read(),
        ),
        make::<substrate_fs_query::hash::FsHashRequest>(
            "fs_hash", descriptions::fs_hash(),
        ),
        // ---- filesystem-mutation BC (write-side) -----------------------------
        make::<substrate_fs_mutation::mkdir::FsMkdirRequest>(
            "fs_mkdir", descriptions::fs_mkdir(),
        ),
        make::<substrate_fs_mutation::write::FsWriteRequest>(
            "fs_write", descriptions::fs_write(),
        ),
        make::<substrate_fs_mutation::copy::FsCopyRequest>(
            "fs_copy", descriptions::fs_copy(),
        ),
        make::<substrate_fs_mutation::rename::FsRenameRequest>(
            "fs_rename", descriptions::fs_rename(),
        ),
        make::<substrate_fs_mutation::remove::FsRemoveRequest>(
            "fs_remove", descriptions::fs_remove(),
        ),
        make::<substrate_fs_mutation::set_permissions::FsSetPermissionsRequest>(
            "fs_set_permissions", descriptions::fs_set_permissions(),
        ),
        make::<substrate_fs_mutation::symlink::FsSymlinkRequest>(
            "fs_symlink", descriptions::fs_symlink(),
        ),
        make::<substrate_fs_mutation::touch::FsTouchRequest>(
            "fs_touch", descriptions::fs_touch(),
        ),
        // ---- process BC ------------------------------------------------------
        make::<substrate_process::list::ProcListRequest>(
            "proc_list", descriptions::proc_list(),
        ),
        make::<substrate_process::tree::ProcTreeRequest>(
            "proc_tree", descriptions::proc_tree(),
        ),
        make::<substrate_process::signal::ProcSignalRequest>(
            "proc_signal", descriptions::proc_signal(),
        ),
        // ---- system-info BC (no caller-supplied parameters) ------------------
        Tool::new("sys_uname",        descriptions::sys_uname(),        schema_empty()),
        Tool::new("sys_hostname",     descriptions::sys_hostname(),     schema_empty()),
        Tool::new("sys_uptime",       descriptions::sys_uptime(),       schema_empty()),
        Tool::new("sys_df",           descriptions::sys_df(),           schema_empty()),
        Tool::new("sys_load_average", descriptions::sys_load_average(), schema_empty()),
        Tool::new("sys_info",         descriptions::sys_info(),         schema_empty()),
        // ---- text-processing BC ----------------------------------------------
        make::<substrate_text::search::SearchParams>(
            "text_search", descriptions::text_search(),
        ),
        make::<substrate_text::count_lines::CountLinesParams>(
            "text_count_lines", descriptions::text_count_lines(),
        ),
        make::<substrate_text::head::HeadParams>(
            "text_head", descriptions::text_head(),
        ),
        make::<substrate_text::tail::TailParams>(
            "text_tail", descriptions::text_tail(),
        ),
        // ---- archive BC ------------------------------------------------------
        make::<substrate_archive::tar_create::TarCreateRequest>(
            "archive_tar_create", descriptions::archive_tar_create(),
        ),
        make::<substrate_archive::tar_extract::TarExtractRequest>(
            "archive_tar_extract", descriptions::archive_tar_extract(),
        ),
        make::<substrate_archive::zip_create::ZipCreateRequest>(
            "archive_zip_create", descriptions::archive_zip_create(),
        ),
        make::<substrate_archive::zip_extract::ZipExtractRequest>(
            "archive_zip_extract", descriptions::archive_zip_extract(),
        ),
        make::<substrate_archive::gzip_compress::GzipCompressRequest>(
            "archive_gzip_compress", descriptions::archive_gzip_compress(),
        ),
        make::<substrate_archive::gzip_decompress::GzipDecompressRequest>(
            "archive_gzip_decompress", descriptions::archive_gzip_decompress(),
        ),
        make::<substrate_archive::hash::ArchiveHashRequest>(
            "archive_hash", descriptions::archive_hash(),
        ),
        // ---- job control-plane -----------------------------------------------
        make::<JobStatusRequest>("job_status", descriptions::job_status()),
        make::<JobResultRequest>("job_result", descriptions::job_result()),
        make::<JobCancelRequest>("job_cancel", descriptions::job_cancel()),
        make::<JobListRequest>("job_list",   descriptions::job_list()),
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
    fn error_result(err: &SubstrateError) -> CallToolResult {
        let structured = serde_json::json!({
            "code": err.code(),
            "message": err.to_string(),
            "recovery_hint": err.recovery_hint(),
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
        let experimental_value =
            build_experimental_capabilities(&self.caps, self.jobs_wired);

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

        let mut caps = ServerCapabilities::builder()
            .enable_tools()
            .build();
        // Wire the experimental substrate block if non-empty.
        if let Some(exp) = experimental {
            caps.experimental = Some(exp);
        }
        caps
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
                    tracing::warn!(
                        client_version = %request.protocol_version,
                        "client protocol version below minimum — rejecting"
                    );
                    return Err(McpErrorData::invalid_request(
                        format!(
                            "unsupported protocol version: {}. Minimum supported: {}",
                            request.protocol_version,
                            super::initialize::PROTOCOL_VERSION_MINIMUM
                        ),
                        None,
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
                        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
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
                Ok(resp) => Ok(Self::into_call_tool_result(resp, false)),
                Err(err) => {
                    tracing::warn!(
                        tool = %request.name,
                        code = err.code(),
                        "tool dispatch error"
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

            if let Ok(job_id) =
                substrate_domain::value_objects::JobId::parse_crockford(&token_str)
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
        // 7 archive + 4 job = 37.  The dispatch match arms in `dispatcher.rs`
        // define the authoritative count; this test pins parity between the
        // registry and the dispatcher.
        let tools = tool_registry();
        assert_eq!(
            tools.len(),
            37,
            "registry/dispatcher parity check failed: found {} tools, expected 37",
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
