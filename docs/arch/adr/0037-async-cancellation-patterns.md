---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0037 — Async Cancellation and Permit Lifetime Patterns

## Context and Problem Statement

`substrate` uses `tokio::select!`, `tokio::sync::Semaphore`, `tokio::task::JoinSet`, and `#[async_trait]` extensively. Several subtle correctness issues emerge at the intersection of these primitives with the `panic = "abort"` build profile (see [ADR-0014](0014-build-system-and-toolchain.md)) and the `CancellationToken`-based cancellation model (see [ADR-0006](0006-tokio-runtime-timeout-cancellation.md)):

- Non-biased `select!` can silently discard a completed work result in favor of a same-instant cancellation.
- Semaphore permits moved into `spawn_blocking` closures are permanently leaked on panic because `panic = "abort"` suppresses stack unwinding.
- Bare `JoinHandle`s dropped without `abort` create detached tasks that outlive their parent cancellation scope.
- Rust has no async `Drop`; cancel-path cleanup (e.g., temp file deletion) written as `Drop` impls silently fails to await the cleanup future.
- Wrong `Mutex` type causes deadlock or compile error when `.await` is held across a lock.
- `#[async_trait]` without `Send` bounds produces non-`Send` futures that cannot be spawned onto the multi-thread runtime.

This ADR codifies the correct pattern for each concern so that code review and automated linting can enforce them uniformly across all tool implementations.

## Decision Drivers

- `panic = "abort"` is non-negotiable for release binaries (see [ADR-0014](0014-build-system-and-toolchain.md)); patterns that rely on unwind-based RAII are unsound under abort.
- Every tool spawn may be cancelled at any `.await` point; all patterns must be cancel-safe by construction.
- The tokio multi-thread runtime requires all spawned futures to be `Send`; non-`Send` futures cause runtime panics, not compile errors, when spawned.
- Code patterns must be teachable and enforceable by Clippy lints where possible; patterns that require bespoke tooling are deprioritized.

## Considered Options

For each concern, the accepted option is documented in the Decision Outcome. Rejected alternatives are noted inline.

## Decision Outcome

### Biased Select: Work Arm First

Every `tokio::select!` that races a work future against a cancellation future MUST use `biased;` with the work arm listed first:

```rust
// Correct: biased, work checked before cancellation.
tokio::select! {
    biased;
    result = do_work() => result?,
    _ = ctx.token.cancelled() => return Err(ToolError::Cancelled),
}
```

Without `biased;`, tokio randomly polls all arms on each `select!` invocation. If the work future and the cancellation future both resolve in the same executor tick, tokio may choose the cancellation arm first, silently discarding a successfully completed result. With `biased;` and the work arm first, a completed result is always returned before cancellation is observed.

Rejected alternative: use `tokio::select!` without `biased;` and accept occasional result loss — rejected; non-deterministic data loss is unacceptable in a tool server.

### Permit Lifetime Rule: Permits in Async Scope

Semaphore permits MUST be acquired in the async function scope and MUST NOT be moved into `spawn_blocking` closures:

```rust
// Correct: permit lives in async scope; spawn_blocking receives only owned data.
async fn hash_file(ctx: &ToolCtx, path: PathBuf) -> Result<String> {
    let permit = Arc::clone(&ctx.cpu_semaphore)
        .acquire_owned()
        .await
        .map_err(|_| ToolError::semaphore_closed())?;

    let digest = tokio::task::spawn_blocking(move || {
        // permit is NOT moved here; only path is moved.
        blake3_hash_streaming(&path)
    })
    .await??;

    drop(permit); // explicit drop; permit released after blocking work completes.
    Ok(digest)
}
```

Reason: under `panic = "abort"`, a panic inside `spawn_blocking` does not unwind; it aborts the process. Under `panic = "unwind"` (dev profile), a panic inside the closure triggers `catch_unwind` at the tokio task boundary, which drops the closure. If the permit were inside the closure, it would be dropped and released on panic. Under abort, no drop runs at all — the permit is permanently leaked. By keeping the permit in the async scope, the async executor's drop path (which does run on task cancellation and future drop) releases the permit regardless of the closure's outcome.

Rejected alternative: move permit into closure and document as known leak — rejected; a leaked permit reduces effective concurrency permanently and is invisible to operators.

### Owned Semaphore Permits: `acquire_owned()` Mandatory

All `tokio::sync::Semaphore` permit acquisitions MUST use `Arc<Semaphore>::acquire_owned()` to obtain an `OwnedSemaphorePermit`:

```rust
// Correct
let semaphore: Arc<Semaphore> = Arc::clone(&ctx.cpu_semaphore);
let permit: OwnedSemaphorePermit = semaphore.acquire_owned().await?;
```

`Semaphore::acquire()` returns a `SemaphorePermit<'_>` that borrows the `Semaphore` and cannot be held across `.await` points that may outlive the borrow. `OwnedSemaphorePermit` is `'static` and survives across awaits, enabling the permit-in-async-scope pattern above.

### JoinSet Rule: No Bare JoinHandles

Tools that spawn internal `tokio::spawn` tasks MUST store all `JoinHandle`s in a `tokio::task::JoinSet`. On `CancellationToken` cancellation, `joinset.abort_all()` must be called before returning:

```rust
// Correct
let mut set: JoinSet<Result<()>> = JoinSet::new();
set.spawn(subtask_a(ctx.token.child_token()));
set.spawn(subtask_b(ctx.token.child_token()));

tokio::select! {
    biased;
    _ = collect_all(&mut set) => {},
    _ = ctx.token.cancelled() => {
        set.abort_all();
        return Err(ToolError::Cancelled);
    }
}
```

Dropping a bare `JoinHandle` without calling `abort()` detaches the task: it continues running on the executor, consuming resources, and ignoring the parent's `CancellationToken`. This is the primary cause of resource leaks after cancellation.

Rejected alternative: drop `JoinHandle` and document as "self-limiting" — rejected; detached tasks have no cancellation path and accumulate under load.

### Async Drop Limitation: Cleanup in select! Arms

Rust does not have async `Drop`. Cleanup futures (deleting temp files, flushing write buffers, sending cancel acknowledgments) that must run on cancellation CANNOT be written as `Drop` impls. They MUST be written as `select!` arms executed before returning `ToolError::Cancelled`:

```rust
// Correct: cleanup in select! arm, not in Drop.
tokio::select! {
    biased;
    result = write_to_target(&mut file, &data) => result?,
    _ = ctx.token.cancelled() => {
        // Cleanup runs here, in async context.
        tokio::fs::remove_file(&tmp_path).await.ok();
        return Err(ToolError::Cancelled);
    }
}
```

A `Drop` impl that calls `tokio::runtime::Handle::block_on` is forbidden: it either panics (if called from within an async context) or deadlocks (if the runtime has shut down). A `Drop` impl that spawns a new task is also forbidden: the spawned task may outlive the runtime shutdown sequence.

See also [ADR-0031](0031-file-handle-lifecycle.md) for the temp file lifecycle pattern that builds on this rule.

### Mutex Policy: Sync vs Async vs parking_lot

| Context | Correct Mutex type |
|---|---|
| Inside `spawn_blocking` closure (no `.await` in scope) | `std::sync::Mutex` or `parking_lot::Mutex` |
| Async context with `.await` inside the critical section | `tokio::sync::Mutex` |
| Async context with no `.await` in critical section, low contention | `parking_lot::Mutex` (faster than `std`, integrates with async) |
| Shared across async and sync contexts | `parking_lot::Mutex` with no `.await` inside lock, or redesign |

`std::sync::Mutex` MUST NOT be held across `.await` points. The Clippy lint `clippy::await_holding_lock` is enabled workspace-wide and enforces this at compile time.

`tokio::sync::Mutex` MUST NOT be used inside `spawn_blocking` closures (the tokio runtime context is not available to blocking threads; `lock().await` would require a `block_on` call, which panics inside a blocking thread that is itself running on a tokio executor).

### async_trait Send Bound: Mandatory

All `#[async_trait]` impls on tool traits MUST satisfy the default `Send` bound:

```rust
// Correct
#[async_trait]
impl Tool for MyTool {
    async fn execute(&self, ctx: ToolCtx) -> Result<ToolOutput> { ... }
}
```

`#[async_trait(?Send)]` is only permitted for explicitly non-`Send` contexts (none currently exist in `substrate`). It is forbidden in tool trait impls.

The following types MUST NOT appear inside `#[async_trait]` method bodies because they make the generated future non-`Send`:

- `Rc<T>` (use `Arc<T>` instead)
- `RefCell<T>` (use `Mutex<T>` or restructure)
- Raw pointers `*const T` / `*mut T` that cross `.await` points

Rejected alternative: `#[async_trait(?Send)]` as default — rejected; the multi-thread tokio runtime requires `Send` futures for all spawned tasks; a non-`Send` tool future cannot be dispatched by the scheduler.

### Consequences

#### Positive

- Biased select eliminates the silent-result-discard class of bugs.
- Permit-in-async-scope eliminates permanent semaphore leaks under both abort and unwind panic profiles.
- `JoinSet` enforces structured concurrency; no detached tasks survive cancellation.
- Explicit cancel-path cleanup in `select!` arms makes cancellation behavior auditable via code review without inspecting `Drop` impls.
- Mutex policy prevents the two most common async mutex misuse deadlocks.
- Mandatory `Send` on async traits prevents runtime panics from non-`Send` futures.

#### Negative

- `biased;` disables tokio's fairness guarantee; if the work arm is always ready, the cancellation arm is never polled. In practice this is correct behavior: we want to return the result, not cancel it. However, in tests that need to observe cancellation, the cancellation future must fire before the work future completes.
- Keeping permits in the async scope requires callers to hold the permit variable across the `spawn_blocking` call, which is slightly more verbose than moving the permit into the closure.
- Cleanup in `select!` arms requires duplicating cleanup logic if the same cleanup must also run on the happy path. The recommended pattern is a cleanup function called from both arms.

## Validation

- Unit test: construct a `select!` without `biased;` where both arms resolve simultaneously; confirm that `biased;` version always picks the work arm.
- Unit test: panic inside a `spawn_blocking` closure with `panic = "unwind"`; assert the permit (held in async scope) is released and the semaphore permit count returns to its initial value.
- Unit test: drop a `JoinHandle` without abort; confirm via `JoinSet` refactor that abort is called on cancellation.
- Code review checklist: every `tokio::select!` that races work vs cancellation uses `biased;`; every `Semaphore` acquire uses `acquire_owned()`; no `spawn` result is ignored without `JoinSet` registration.
- Clippy lint `clippy::await_holding_lock` must emit zero warnings.

## Cross-References

- [ADR-0006](0006-tokio-runtime-timeout-cancellation.md): `CancellationToken` propagation; timeout strategy; `MutexGuard` across `.await` lint.
- [ADR-0014](0014-build-system-and-toolchain.md): `panic = "abort"` profile; `catch_unwind` semantics; unwind RAII.
- [ADR-0017](0017-concurrency-limits.md): Semaphore sizing and `OwnedSemaphorePermit` usage.
- [ADR-0031](0031-file-handle-lifecycle.md): temp file lifecycle; explicit flush; NFS close hazard.

## Amendments

### 2026-05-21 — Extended by ADR-0040 async-job-control-plane

ADR-0040 introduces a JobRegistry and per-job CancellationToken hierarchy. The cancellation patterns established by this ADR are extended to cover client-initiated job cancellation via the MCP protocol and server-shutdown propagation through the job tree.

**Additions:**

- A client-initiated `notifications/cancelled` MCP protocol message for an in-flight tool call MUST be mapped by the substrate-mcp-server dispatch layer to `job.cancel(progressToken)`. The mapping key is `progressToken == job_id`. The dispatch layer is responsible for this translation; individual tool implementations are not aware of the MCP-level cancellation message.
- Each job owns a dedicated CancellationToken that is a child of the workspace-rooted server shutdown token. Server shutdown cancels the parent token, which propagates automatically to all child job tokens without additional dispatch-layer intervention.
- Worker tasks implementing jobs MUST use `tokio::select! biased` with the CancellationToken arm first and the work arm second, consistent with the existing biased-select pattern defined in this ADR.
- The cancellation chain order for a job is: token.cancel() is called; the biased select in the worker task acknowledges; the worker drops any held async resources; transactional cleanup runs (cross-ref ADR-0033 for `.tmp.<uuid7>` removal); result_tx.send(Err(Cancelled)) is called on the result channel; the final audit state transition is emitted to the job audit log before the task future completes.

### 2026-05-21 — Extended by ADR-0041 filesystem-index-native-tiers

ADR-0041 introduces index rebuild operations executed in Zone B (spawn_blocking) and a hot-path lookup executed in Zone A. The cancellation patterns differ between these two zones and are specified here.

**Additions:**

- Index rebuild operations run inside `spawn_blocking` closures (Zone B). Because blocking closures cannot await, cancellation awareness is implemented as a periodic check: the rebuild loop MUST check the CancellationToken's `is_cancelled()` flag at every directory-iteration boundary (that is, once per allowlist root processed). On cancellation, the partial index snapshot is discarded in full; no atomic swap to the live index occurs, and no partial state is persisted.
- On cancellation during rebuild, any temporary snapshot files created by the rebuild MUST be deleted synchronously within the blocking closure before it returns (consistent with the async Drop limitation rule in this ADR applied to the sync context).
- Index lookup operations on the hot path run in Zone A and do NOT check cancellation per individual entry. Lookups are expected to complete in sub-millisecond time; a stale cancellation observed after a lookup completes is acceptable. The calling tool's biased select will observe the cancellation at the next await boundary after the lookup returns.
