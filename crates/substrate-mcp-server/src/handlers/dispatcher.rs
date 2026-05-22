//! Central tool dispatcher — routes incoming MCP `tools/call` requests to the
//! appropriate adapter handler per ADR-0022.
//!
//! Every tool name is a static, compile-time constant matched here. Bucket B/C
//! tools follow the inline-vs-job decision policy from ADR-0040. Bucket A/D
//! tools execute inline and return immediately.
//!
//! # Bucket routing summary
//!
//! | Bucket | Policy | Tools |
//! |--------|--------|-------|
//! | A      | always inline | `sys_*`, `fs_stat`, `fs_read_dir`, `text_head`, `text_tail`, `proc_*` |
//! | B      | inline if small; job if large | `fs_find`, `fs_read`, `fs_hash`, `fs_copy`, `text_search`, `text_count_lines`, `archive_gzip_*`, `archive_hash` |
//! | C      | always job | `archive_tar_*`, `archive_zip_*` |
//! | D      | inline, fast side-effect | `fs_mkdir`, `fs_write`, `fs_rename`, `fs_touch`, `fs_set_permissions`, `fs_symlink` |
//!
//! # `notifications/cancelled` mapping
//!
//! TODO Wave G: wire `notifications/cancelled` → `JobRegistryPort::cancel(progressToken)`.
//! When rmcp 1.7 exposes a cancellation notification hook, intercept it in
//! `run_stdio_server` and call `Arc<dyn JobRegistryPort>::cancel(&job_id)` where
//! `job_id == progressToken` (triple-equality per ADR-0040 §3.1).

// All items in this module are wired to rmcp in Wave G. Until then the
// scaffolding compiles but is not yet called by the dispatch loop.
#![expect(
    dead_code,
    reason = "Wave G wires ToolDispatcher into the rmcp STDIO dispatch loop; all items used then"
)]

use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_config::RuntimeConfig;
use substrate_domain::{
    ClientId, JailedPath, JobBucket, JobRegistryPort, SubstrateError, SubstrateResult,
    ports::job_registry::JobSubmitRequest,
    value_objects::{IdempotencyKey, JobId},
};

use substrate_archive::{ArchiveDeps, ToolResponse as ArchiveToolResponse};
use substrate_fs_mutation::{FsMutationDeps, ToolResponse as FsMutationToolResponse};
use substrate_fs_query::{FsQueryDeps, ToolResponse as FsQueryToolResponse};
use substrate_process::{
    ProcessDeps, ProcessScannerPort, ToolResponse as ProcessToolResponse,
};
use substrate_system_info::{SystemInfoDeps, ToolResponse as SystemInfoToolResponse};
use substrate_text::{TextDeps, ToolResponse as TextToolResponse};

// ---- Unified response type --------------------------------------------------

/// Unified tool response envelope used by the MCP server dispatch layer.
///
/// Each adapter crate has its own `ToolResponse` type with identical fields;
/// we convert them all to this common form so `dispatcher.dispatch` has a
/// single return type.
#[derive(Debug, Clone)]
pub(crate) struct DispatchedResponse {
    /// Model-oriented text (≤80 tokens per ADR-0007 narrative arc).
    pub(crate) content: String,

    /// Programmatic JSON payload for the `structuredContent` field.
    pub(crate) structured_content: Value,

    /// Structured hints map (ADR-0007 + ADR-0040 extension).
    pub(crate) hints: substrate_domain::Hints,
}

// ---- Conversion helpers -----------------------------------------------------

fn from_fs_query(r: FsQueryToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

fn from_fs_mutation(r: FsMutationToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

fn from_process(r: ProcessToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

fn from_system_info(r: SystemInfoToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

fn from_text(r: TextToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

fn from_archive(r: ArchiveToolResponse) -> DispatchedResponse {
    DispatchedResponse {
        content: r.content,
        structured_content: r.structured_content,
        hints: r.hints,
    }
}

// ---- Pending-job response ---------------------------------------------------

/// Constructs the job-pending `DispatchedResponse` returned when a Bucket B/C
/// tool is promoted to an async job rather than executing inline.
///
/// The `job_id` is surfaced in `structuredContent.job_id` and
/// `hints.job_id` + `hints.next_action_suggested = "job_status"` so the agent
/// knows to poll.
fn job_pending_response(job_id: &JobId) -> DispatchedResponse {
    // Serialize job_id via serde (UUID hyphenated format) so clients can pass
    // it directly back to job_status / job_result / job_cancel, which all
    // deserialize `job_id` via `JobId: Deserialize` (inner Uuid format).
    // Using `Display` (Crockford base32) would mismatch the server's Deserialize.
    let job_id_serialized = serde_json::to_value(job_id)
        .unwrap_or(serde_json::Value::Null);
    let job_id_str = job_id_serialized.as_str().unwrap_or("").to_owned();
    let structured = serde_json::json!({
        "job_id": job_id_serialized,
        "state": "Pending",
    });
    let hints = substrate_domain::Hints {
        job_id: Some(job_id_str),
        next_action_suggested: Some("job_status".to_owned()),
        polling_endpoint: Some("job_status".to_owned()),
        ..substrate_domain::Hints::default()
    };
    DispatchedResponse {
        content: "Tool submitted as an async job. Use job_status to poll state.".to_owned(),
        structured_content: structured,
        hints,
    }
}

// ---- Dispatcher -------------------------------------------------------------

/// All runtime-wired adapter dependency bundles and shared services.
///
/// Constructed once by the composition root and shared (via `Arc`) across
/// all concurrent MCP tool calls. Each bundle contains the `Arc<dyn Port>`
/// types its handlers require.
pub(crate) struct ToolDispatcher {
    /// Dependency bundle for filesystem-query (read-side) handlers.
    pub(crate) fs_query: FsQueryDeps,

    /// Dependency bundle for filesystem-mutation (write-side) handlers.
    pub(crate) fs_mutation: FsMutationDeps,

    /// Dependency bundle for process handlers.
    pub(crate) process: ProcessDeps,

    /// Platform-appropriate process scanner, shared across all process handlers.
    ///
    /// Built once at startup by `substrate_process::default_scanner()` and
    /// reused for all `proc.list` / `proc.tree` calls. `proc.signal` does not
    /// require a scanner (uses `nix::sys::signal::kill` directly).
    pub(crate) scanner: Arc<dyn ProcessScannerPort>,

    /// Dependency bundle for system-info handlers.
    pub(crate) system_info: SystemInfoDeps,

    /// Dependency bundle for text-processing handlers.
    pub(crate) text: TextDeps,

    /// Dependency bundle for archive handlers.
    pub(crate) archive: ArchiveDeps,

    /// Job registry — used to submit Bucket B/C jobs and dispatch job.* tools.
    pub(crate) jobs: Arc<dyn JobRegistryPort>,

    /// Frozen runtime configuration for inline-threshold reads.
    pub(crate) config: Arc<RuntimeConfig>,

    /// Pre-built `JailedPath` values for each configured allowlist root.
    ///
    /// Mutation handlers require a `&JailedPath` allowlist root as the anchor
    /// for kernel-level path confinement. The dispatcher picks the first root
    /// that successfully jails the caller-supplied path (see `jail_for`).
    pub(crate) allowlist_roots: Vec<JailedPath>,
}

impl std::fmt::Debug for ToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDispatcher")
            .field("fs_query", &self.fs_query)
            .field("fs_mutation", &self.fs_mutation)
            .field("process", &self.process)
            .field("scanner", &"<dyn ProcessScannerPort>")
            .field("system_info", &self.system_info)
            .field("text", &self.text)
            .field("archive", &self.archive)
            .field("jobs", &"<dyn JobRegistryPort>")
            .field("config", &self.config)
            .field("allowlist_roots_count", &self.allowlist_roots.len())
            .finish()
    }
}

impl ToolDispatcher {
    /// Dispatches a `tools/call` request to the appropriate adapter handler.
    ///
    /// Bucket A/D tools always execute inline and return immediately.
    /// Bucket B tools execute inline when the input is below the configured
    /// threshold; otherwise they are submitted as jobs and a `Pending` receipt
    /// is returned.
    /// Bucket C tools are always submitted as jobs.
    ///
    /// Job control-plane tools (`job_status`, `job_result`, `job_cancel`,
    /// `job_list`) delegate directly to `Arc<dyn JobRegistryPort>`.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_UNKNOWN_TOOL` when `tool` does not match any registered name.
    /// - Any domain error propagated from the handler or registry.
    #[expect(
        clippy::too_many_lines,
        reason = "central dispatch match arms — each arm is a one-liner; extracting sub-dispatchers would not reduce complexity"
    )]
    #[instrument(skip(self, args, cancel), fields(tool))]
    pub(crate) async fn dispatch(
        &self,
        tool: &str,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        match tool {
            // ---- Bucket A/D: filesystem-query (inline) ----------------------
            "fs_read_dir" => {
                let req = parse(args)?;
                substrate_fs_query::handle_fs_read_dir(req, &self.fs_query, cancel)
                    .await
                    .map(from_fs_query)
            },
            "fs_stat" => {
                let req = parse(args)?;
                substrate_fs_query::handle_fs_stat(req, &self.fs_query, cancel)
                    .await
                    .map(from_fs_query)
            },
            // ---- Bucket B: fs_find ------------------------------------------
            "fs_find" => self.dispatch_fs_find(args, cancel, client_id).await,
            // ---- Bucket B: fs_read ------------------------------------------
            "fs_read" => self.dispatch_fs_read(args, cancel, client_id).await,
            // ---- Bucket B: fs_hash ------------------------------------------
            "fs_hash" => self.dispatch_fs_hash(args, cancel, client_id).await,
            // ---- Bucket D: filesystem-mutation (inline side-effect) ----------
            "fs_mkdir" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_mkdir(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_write" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_write(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_copy" => self.dispatch_fs_copy(args, cancel, client_id).await,
            "fs_rename" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_rename(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_remove" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_remove(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_set_permissions" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_set_permissions(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_symlink" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_symlink(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            "fs_touch" => {
                let root = self.primary_root()?;
                let req = parse(args)?;
                substrate_fs_mutation::handle_fs_touch(req, &self.fs_mutation, root)
                    .await
                    .map(from_fs_mutation)
            },
            // ---- Bucket A: process ------------------------------------------
            "proc_list" => {
                let req = parse(args)?;
                let deps = Arc::new(self.process.clone());
                substrate_process::handle_proc_list(req, deps, Arc::clone(&self.scanner))
                    .await
                    .map(from_process)
            },
            "proc_tree" => {
                let req = parse(args)?;
                let deps = Arc::new(self.process.clone());
                substrate_process::handle_proc_tree(req, deps, Arc::clone(&self.scanner))
                    .await
                    .map(from_process)
            },
            "proc_signal" => {
                let req = parse(args)?;
                let deps = Arc::new(self.process.clone());
                substrate_process::handle_proc_signal(req, deps)
                    .await
                    .map(from_process)
            },
            // ---- Bucket A: system-info --------------------------------------
            // All sys_* handlers take Arc<SystemInfoDeps> only (no request param).
            // The args value is intentionally dropped — sys_* tools have no
            // caller-supplied parameters in the current spec.
            "sys_uname" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_uname(deps)
                    .await
                    .map(from_system_info)
            },
            "sys_hostname" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_hostname(deps)
                    .await
                    .map(from_system_info)
            },
            "sys_uptime" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_uptime(deps)
                    .await
                    .map(from_system_info)
            },
            "sys_df" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_df(deps)
                    .await
                    .map(from_system_info)
            },
            "sys_load_average" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_load_average(deps)
                    .await
                    .map(from_system_info)
            },
            "sys_info" => {
                let deps = Arc::new(self.system_info.clone());
                substrate_system_info::handle_sys_info(deps)
                    .await
                    .map(from_system_info)
            },
            // ---- Bucket B: text-processing ----------------------------------
            "text_search" => self.dispatch_text_search(args, cancel, client_id).await,
            "text_count_lines" => {
                self.dispatch_text_count_lines(args, cancel, client_id)
                    .await
            },
            "text_head" => {
                let req = parse(args)?;
                let deps = Arc::new(self.text.clone());
                substrate_text::handle_text_head(req, deps, cancel)
                    .await
                    .map(from_text)
            },
            "text_tail" => {
                let req = parse(args)?;
                let deps = Arc::new(self.text.clone());
                substrate_text::handle_text_tail(req, deps, cancel)
                    .await
                    .map(from_text)
            },
            // ---- Bucket C: archive (always-async job) ------------------------
            //
            // Each arm clones `args` for the idempotency dedup payload and moves
            // the parsed request into a cancel-safe `BoxFuture`.  The registry
            // wraps the future in a `tokio::select! biased` block so the job's
            // child `CancellationToken` can interrupt it cooperatively per ADR-0037.
            "archive_tar_create" => {
                let req: substrate_archive::tar_create::TarCreateRequest = parse(args.clone())?;
                let deps = Arc::new(self.archive.clone());
                // Job-scoped cancel: use a standalone token so the request-level
                // cancel (which fires when the MCP response is sent) does not
                // prematurely cancel the background worker. The registry's slot
                // cancel token (via select! biased) handles cooperative cancellation
                // from job_cancel / SIGTERM per ADR-0037.
                let job_cancel = CancellationToken::new();
                let handler_call: futures::future::BoxFuture<
                    'static,
                    SubstrateResult<serde_json::Value>,
                > = Box::pin(async move {
                    substrate_archive::handle_archive_tar_create(req, &deps, job_cancel)
                        .await
                        .map(|r| {
                            serde_json::to_value(&r.structured_content)
                                .unwrap_or(serde_json::Value::Null)
                        })
                });
                self.dispatch_as_job(
                    args,
                    "archive_tar_create",
                    JobBucket::CAlwaysAsync,
                    client_id,
                    handler_call,
                )
                .await
            },
            "archive_tar_extract" => {
                let req: substrate_archive::tar_extract::TarExtractRequest = parse(args.clone())?;
                let deps = Arc::new(self.archive.clone());
                // Job-scoped cancel: see archive_tar_create comment above.
                let job_cancel = CancellationToken::new();
                let handler_call: futures::future::BoxFuture<
                    'static,
                    SubstrateResult<serde_json::Value>,
                > = Box::pin(async move {
                    substrate_archive::handle_archive_tar_extract(req, &deps, job_cancel)
                        .await
                        .map(|r| {
                            serde_json::to_value(&r.structured_content)
                                .unwrap_or(serde_json::Value::Null)
                        })
                });
                self.dispatch_as_job(
                    args,
                    "archive_tar_extract",
                    JobBucket::CAlwaysAsync,
                    client_id,
                    handler_call,
                )
                .await
            },
            "archive_zip_create" => {
                let req: substrate_archive::zip_create::ZipCreateRequest = parse(args.clone())?;
                let deps = Arc::new(self.archive.clone());
                // Job-scoped cancel: see archive_tar_create comment above.
                let job_cancel = CancellationToken::new();
                let handler_call: futures::future::BoxFuture<
                    'static,
                    SubstrateResult<serde_json::Value>,
                > = Box::pin(async move {
                    substrate_archive::handle_archive_zip_create(req, &deps, job_cancel)
                        .await
                        .map(|r| {
                            serde_json::to_value(&r.structured_content)
                                .unwrap_or(serde_json::Value::Null)
                        })
                });
                self.dispatch_as_job(
                    args,
                    "archive_zip_create",
                    JobBucket::CAlwaysAsync,
                    client_id,
                    handler_call,
                )
                .await
            },
            "archive_zip_extract" => {
                let req: substrate_archive::zip_extract::ZipExtractRequest = parse(args.clone())?;
                let deps = Arc::new(self.archive.clone());
                // Job-scoped cancel: see archive_tar_create comment above.
                let job_cancel = CancellationToken::new();
                let handler_call: futures::future::BoxFuture<
                    'static,
                    SubstrateResult<serde_json::Value>,
                > = Box::pin(async move {
                    substrate_archive::handle_archive_zip_extract(req, &deps, job_cancel)
                        .await
                        .map(|r| {
                            serde_json::to_value(&r.structured_content)
                                .unwrap_or(serde_json::Value::Null)
                        })
                });
                self.dispatch_as_job(
                    args,
                    "archive_zip_extract",
                    JobBucket::CAlwaysAsync,
                    client_id,
                    handler_call,
                )
                .await
            },
            // ---- Bucket B: archive gzip + hash ------------------------------
            "archive_gzip_compress" => {
                self.dispatch_archive_gzip_compress(args, cancel, client_id)
                    .await
            },
            "archive_gzip_decompress" => {
                self.dispatch_archive_gzip_decompress(args, cancel, client_id)
                    .await
            },
            "archive_hash" => self.dispatch_archive_hash(args, cancel, client_id).await,
            // ---- Job control-plane ------------------------------------------
            "job_status" => self.handle_job_status(args).await,
            "job_result" => self.handle_job_result(args).await,
            "job_cancel" => self.handle_job_cancel(args).await,
            "job_list" => self.handle_job_list(args, client_id).await,
            // ---- Unknown tool -----------------------------------------------
            unknown => Err(SubstrateError::InvalidArgument {
                offending_field: "tool_name".to_owned(),
                reason: format!("unknown tool: {unknown}"),
                correlation_id: None,
            }),
        }
    }

    // ---- Allowlist root helpers ---------------------------------------------

    /// Returns a reference to the first configured allowlist root.
    ///
    /// Mutation handlers require `&JailedPath` as the kernel-jail anchor.
    /// When multiple roots are configured, Wave G will add per-request root
    /// selection based on the target path; for now the primary root is used
    /// as the anchor and the jail enforces the containment check.
    ///
    /// # Errors
    ///
    /// Returns `SUBSTRATE_INTERNAL_ERROR` when no allowlist roots are
    /// configured (an empty `policy.roots` that slipped past startup validation).
    fn primary_root(&self) -> SubstrateResult<&JailedPath> {
        self.allowlist_roots.first().ok_or_else(|| SubstrateError::InternalError {
            reason: "no allowlist roots configured — composition root should have rejected empty policy.roots".to_owned(),
            correlation_id: None,
        })
    }

    // ---- Bucket B: fs_find --------------------------------------------------

    /// Dispatches `fs_find`: inline if the request does not force async,
    /// otherwise submits as a Bucket B job.
    ///
    /// TODO Wave G: inspect the `FsFindRequest` to derive the candidate count
    /// from `fs_index` and compare against `inline_thresholds.fs_find_inline_entries`.
    /// For now the request always runs inline; job path is a placeholder.
    async fn dispatch_fs_find(
        &self,
        args: Value,
        cancel: CancellationToken,
        // Wave G: used for job routing once async job path is wired
        _client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        // Threshold guard (Wave G will inspect actual request fields).
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(1_000, |c| c.inline_thresholds.fs_find_inline_entries);

        // TODO Wave G: derive actual candidate count from fs_index or lstat.
        // Currently always below threshold — executes inline.
        let _ = threshold;

        let req = parse(args)?;
        substrate_fs_query::handle_fs_find(req, &self.fs_query, cancel)
            .await
            .map(from_fs_query)
    }

    // ---- Bucket B: fs_read --------------------------------------------------

    /// Dispatches `fs_read`: inline when the file is below `fs_read_inline_bytes`;
    /// promotes to a Bucket B job otherwise.
    ///
    /// Path is extracted from the raw `args` JSON before parsing so we can stat
    /// it without consuming the `Value`.
    async fn dispatch_fs_read(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(1_048_576, |c| c.inline_thresholds.fs_read_inline_bytes);

        let path = args.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_fs_query::read::FsReadRequest = parse(args.clone())?;
            let fs_query = self.fs_query.clone();
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_fs_query::handle_fs_read(req, &fs_query, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(args, "fs_read", JobBucket::BAutoMode, client_id, handler_call)
                .await;
        }

        let req = parse(args)?;
        substrate_fs_query::handle_fs_read(req, &self.fs_query, cancel)
            .await
            .map(from_fs_query)
    }

    // ---- Bucket B: fs_hash --------------------------------------------------

    /// Dispatches `fs_hash`: inline when the file is below `fs_hash_inline_bytes`;
    /// promotes to a Bucket B job otherwise.
    async fn dispatch_fs_hash(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(4_194_304, |c| c.inline_thresholds.fs_hash_inline_bytes);

        let path = args.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_fs_query::hash::FsHashRequest = parse(args.clone())?;
            let fs_query = self.fs_query.clone();
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_fs_query::handle_fs_hash(req, &fs_query, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(args, "fs_hash", JobBucket::BAutoMode, client_id, handler_call)
                .await;
        }

        let req = parse(args)?;
        substrate_fs_query::handle_fs_hash(req, &self.fs_query, cancel)
            .await
            .map(from_fs_query)
    }

    // ---- Bucket B: fs_copy --------------------------------------------------

    /// Dispatches `fs_copy`: inline when the source file is below `fs_copy_inline_bytes`;
    /// promotes to a Bucket B job otherwise.
    async fn dispatch_fs_copy(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(1_048_576, |c| c.inline_thresholds.fs_copy_inline_bytes);

        // `FsCopyRequest` uses `src` as the source field name.
        let src_path = args.get("src").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&src_path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_fs_mutation::copy::FsCopyRequest = parse(args.clone())?;
            let fs_mutation = self.fs_mutation.clone();
            // Clone the primary root JailedPath for the async closure.
            // `primary_root` returns `&JailedPath`; we need an owned copy.
            let root_owned = self.primary_root()?.clone();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_fs_mutation::handle_fs_copy(req, &fs_mutation, &root_owned)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(args, "fs_copy", JobBucket::BAutoMode, client_id, handler_call)
                .await;
        }

        let root = self.primary_root()?;
        let req = parse(args)?;
        // `_cancel` is unused in the inline path per existing dispatcher design.
        let _ = cancel;
        substrate_fs_mutation::handle_fs_copy(req, &self.fs_mutation, root)
            .await
            .map(from_fs_mutation)
    }

    // ---- Bucket B: text_search ----------------------------------------------

    /// Dispatches `text_search`: inline when the file is below `text_search_inline_bytes`;
    /// promotes to a Bucket B job otherwise.
    async fn dispatch_text_search(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(524_288, |c| c.inline_thresholds.text_search_inline_bytes);

        let path = args.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_text::search::SearchParams = parse(args.clone())?;
            let deps = Arc::new(self.text.clone());
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_text::handle_text_search(req, deps, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(
                    args,
                    "text_search",
                    JobBucket::BAutoMode,
                    client_id,
                    handler_call,
                )
                .await;
        }

        let req = parse(args)?;
        let deps = Arc::new(self.text.clone());
        substrate_text::handle_text_search(req, deps, cancel)
            .await
            .map(from_text)
    }

    // ---- Bucket B: text_count_lines -----------------------------------------

    /// Dispatches `text_count_lines`: inline when the file is below
    /// `text_count_lines_inline_bytes`; promotes to a Bucket B job otherwise.
    async fn dispatch_text_count_lines(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self.config.jobs.as_ref().map_or(524_288, |c| {
            c.inline_thresholds.text_count_lines_inline_bytes
        });

        let path = args.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_text::count_lines::CountLinesParams = parse(args.clone())?;
            let deps = Arc::new(self.text.clone());
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_text::handle_text_count_lines(req, deps, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(
                    args,
                    "text_count_lines",
                    JobBucket::BAutoMode,
                    client_id,
                    handler_call,
                )
                .await;
        }

        let req = parse(args)?;
        let deps = Arc::new(self.text.clone());
        substrate_text::handle_text_count_lines(req, deps, cancel)
            .await
            .map(from_text)
    }

    // ---- Bucket B: archive gzip compress ------------------------------------

    /// Dispatches `archive_gzip_compress`: inline when the source file is below
    /// `archive_gzip_inline_bytes`; promotes to a Bucket B job otherwise.
    async fn dispatch_archive_gzip_compress(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(131_072, |c| c.inline_thresholds.archive_gzip_inline_bytes);

        // `GzipCompressRequest` uses `source` as the input path field.
        let source_path = args.get("source").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&source_path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_archive::gzip_compress::GzipCompressRequest = parse(args.clone())?;
            let archive = self.archive.clone();
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_archive::handle_archive_gzip_compress(req, &archive, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(
                    args,
                    "archive_gzip_compress",
                    JobBucket::BAutoMode,
                    client_id,
                    handler_call,
                )
                .await;
        }

        let req = parse(args)?;
        substrate_archive::handle_archive_gzip_compress(req, &self.archive, cancel)
            .await
            .map(from_archive)
    }

    // ---- Bucket B: archive gzip decompress ----------------------------------

    /// Dispatches `archive_gzip_decompress`: inline when the source file is below
    /// `archive_gzip_inline_bytes`; promotes to a Bucket B job otherwise.
    async fn dispatch_archive_gzip_decompress(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(131_072, |c| c.inline_thresholds.archive_gzip_inline_bytes);

        // `GzipDecompressRequest` uses `source` as the input path field.
        let source_path = args.get("source").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&source_path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_archive::gzip_decompress::GzipDecompressRequest =
                parse(args.clone())?;
            let archive = self.archive.clone();
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_archive::handle_archive_gzip_decompress(req, &archive, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(
                    args,
                    "archive_gzip_decompress",
                    JobBucket::BAutoMode,
                    client_id,
                    handler_call,
                )
                .await;
        }

        let req = parse(args)?;
        substrate_archive::handle_archive_gzip_decompress(req, &self.archive, cancel)
            .await
            .map(from_archive)
    }

    // ---- Bucket B: archive hash ---------------------------------------------

    /// Dispatches `archive_hash`: inline when the archive is below
    /// `archive_hash_inline_bytes`; promotes to a Bucket B job otherwise.
    async fn dispatch_archive_hash(
        &self,
        args: Value,
        cancel: CancellationToken,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        let threshold = self
            .config
            .jobs
            .as_ref()
            .map_or(4_194_304, |c| c.inline_thresholds.archive_hash_inline_bytes);

        let path = args.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
        let size = Self::file_size_bytes(&path).await.unwrap_or(0);

        if size >= threshold {
            let req: substrate_archive::hash::ArchiveHashRequest = parse(args.clone())?;
            let archive = self.archive.clone();
            // Job-scoped cancel: standalone token so request-level cancel (fires
            // when MCP response is sent) does not interrupt the background worker.
            // The registry select! biased uses slot_cancel for cooperative cancel.
            let cancel_child = CancellationToken::new();
            let handler_call: futures::future::BoxFuture<
                'static,
                SubstrateResult<serde_json::Value>,
            > = Box::pin(async move {
                substrate_archive::handle_archive_hash(req, &archive, cancel_child)
                    .await
                    .map(|r| {
                        serde_json::to_value(&r.structured_content)
                            .unwrap_or(serde_json::Value::Null)
                    })
            });
            return self
                .dispatch_as_job(
                    args,
                    "archive_hash",
                    JobBucket::BAutoMode,
                    client_id,
                    handler_call,
                )
                .await;
        }

        let req = parse(args)?;
        substrate_archive::handle_archive_hash(req, &self.archive, cancel)
            .await
            .map(from_archive)
    }

    // ---- Bucket B: inline-vs-job threshold helper ---------------------------

    /// Stats `path` and returns the file size in bytes.
    ///
    /// Returns `None` if the path cannot be stat'd (missing, permission denied,
    /// or not a regular file). Callers treat `None` as "size unknown; run inline"
    /// so that a stat failure never silently promotes a fast tool to an async job.
    async fn file_size_bytes(path: &str) -> Option<u64> {
        tokio::fs::metadata(path)
            .await
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
    }

    // ---- Bucket C: always-async job submission ------------------------------

    /// Submits a Bucket C (always-async) tool as an async job via the registry.
    ///
    /// `handler_call` is the `BoxFuture` that invokes the adapter handler.  It is
    /// moved into `JobSubmitRequest.execute` so the registry can spawn it as a
    /// `tokio` task, wrapped in a `CancellationToken`-biased `select!` per ADR-0037.
    ///
    /// Returns a `Pending` receipt immediately. The caller polls via `job_status`
    /// and retrieves the terminal result via `job_result`.
    async fn dispatch_as_job(
        &self,
        args: Value,
        tool_name: &str,
        bucket: JobBucket,
        client_id: ClientId,
        handler_call: futures::future::BoxFuture<'static, SubstrateResult<serde_json::Value>>,
    ) -> SubstrateResult<DispatchedResponse> {
        let job_id = self
            .jobs
            .submit(JobSubmitRequest {
                client_id,
                tool: tool_name.to_owned(),
                bucket,
                idempotency_key: extract_idempotency_key(&args),
                args_json: args,
                execute: handler_call,
            })
            .await?;

        Ok(job_pending_response(&job_id))
    }

    // ---- Job control-plane handlers -----------------------------------------

    async fn handle_job_status(&self, args: Value) -> SubstrateResult<DispatchedResponse> {
        #[derive(serde::Deserialize)]
        struct Req {
            job_id: JobId,
        }
        let req: Req = parse(args)?;
        let entry = self.jobs.status(&req.job_id).await?;
        let content = format!("Job {} state: {:?}", req.job_id, entry.state);
        Ok(DispatchedResponse {
            content,
            structured_content: serde_json::to_value(&entry).unwrap_or(Value::Null),
            hints: substrate_domain::Hints::default(),
        })
    }

    async fn handle_job_result(&self, args: Value) -> SubstrateResult<DispatchedResponse> {
        #[derive(serde::Deserialize)]
        struct Req {
            job_id: JobId,
            #[serde(default)]
            wait_ms: Option<u64>,
        }
        let req: Req = parse(args)?;
        let wait = req.wait_ms.map(std::time::Duration::from_millis);
        let result = self.jobs.result(&req.job_id, wait).await?;
        let structured = match &result {
            substrate_domain::ports::job_registry::JobResult::Succeeded(v) => v.clone(),
            substrate_domain::ports::job_registry::JobResult::Failed(e) => {
                serde_json::json!({ "error": e.to_string() })
            },
            substrate_domain::ports::job_registry::JobResult::Cancelled => {
                serde_json::json!({ "state": "Cancelled" })
            },
            substrate_domain::ports::job_registry::JobResult::TimedOut => {
                serde_json::json!({ "state": "TimedOut" })
            },
        };
        Ok(DispatchedResponse {
            content: "Job result retrieved.".to_owned(),
            structured_content: structured,
            hints: substrate_domain::Hints::default(),
        })
    }

    async fn handle_job_cancel(&self, args: Value) -> SubstrateResult<DispatchedResponse> {
        #[derive(serde::Deserialize)]
        struct Req {
            job_id: JobId,
        }
        let req: Req = parse(args)?;
        let state = self.jobs.cancel(&req.job_id).await?;
        Ok(DispatchedResponse {
            content: format!(
                "Job {} cancellation triggered; current state: {state:?}",
                req.job_id
            ),
            structured_content: serde_json::json!({ "state": format!("{state:?}") }),
            hints: substrate_domain::Hints::default(),
        })
    }

    async fn handle_job_list(
        &self,
        args: Value,
        client_id: ClientId,
    ) -> SubstrateResult<DispatchedResponse> {
        #[derive(serde::Deserialize, Default)]
        struct Req {
            #[serde(default)]
            cursor: Option<substrate_domain::PageCursor>,
        }
        let req: Req = if args.is_null() || args == Value::Object(serde_json::Map::default()) {
            Req::default()
        } else {
            parse(args)?
        };
        let page = self.jobs.list(&client_id, req.cursor).await?;
        Ok(DispatchedResponse {
            content: format!("Listed {} job(s).", page.jobs.len()),
            structured_content: serde_json::json!({
                "jobs": page.jobs.iter().map(|e| serde_json::to_value(e).unwrap_or(Value::Null)).collect::<Vec<_>>(),
                "next_cursor": page.next_cursor.as_ref().map(|c| {
                    c.as_bytes().iter().fold(String::new(), |mut s, b| {
                        use std::fmt::Write as _;
                        let _ = write!(s, "{b:02x}");
                        s
                    })
                }),
            }),
            hints: substrate_domain::Hints::default(),
        })
    }
}

// ---- Helpers ----------------------------------------------------------------

/// Deserializes `Value` into `T`, mapping JSON errors to
/// `SUBSTRATE_INVALID_ARGUMENT`.
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> SubstrateResult<T> {
    serde_json::from_value(value).map_err(|e| SubstrateError::InvalidArgument {
        offending_field: "arguments".to_owned(),
        reason: e.to_string(),
        correlation_id: None,
    })
}

/// Attempts to extract a client-supplied idempotency key from the raw args
/// JSON.  Looks for a top-level `"idempotency_key"` string field encoded as
/// Crockford base32 (26 characters) per ADR-0040.
fn extract_idempotency_key(args: &Value) -> Option<IdempotencyKey> {
    args.get("idempotency_key")
        .and_then(Value::as_str)
        .and_then(|s| IdempotencyKey::parse_crockford(s).ok())
}
