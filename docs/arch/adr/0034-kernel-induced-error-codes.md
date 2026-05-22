---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0034 — Kernel-Induced Error Codes

## Context and Problem Statement

ADR-0010 defines 13 stable error codes covering path safety, permissions, resource limits, and protocol mismatches. Direct OS errno values arising from hardware failures, filesystem states (read-only mounts, storage exhaustion, symlink cycles), and transient kernel conditions are not yet mapped to a distinct stable code. Without an explicit mapping, these errors fall through to `SUBSTRATE_INTERNAL_ERROR`, losing diagnostic fidelity and preventing LLM agents from applying targeted recovery.

## Decision Drivers

- Kernel errno values that represent caller-actionable conditions must map to distinct, stable codes, not a generic internal error.
- Codes that look similar (e.g., ELOOP vs. SYMLINK_ESCAPE) must be distinguishable: one is a security policy violation, the other is an OS resolution limit.
- Recovery hints must remain actionable and ≤ 150 characters (CUE schema enforces this cap; see ADR-0010 addendum).
- The retry contract for transient errors (EINTR, EAGAIN, EBUSY, ESTALE) must be defined server-side so callers never need to re-implement it.

## Considered Options

1. Map all unhandled kernel errors to `SUBSTRATE_INTERNAL_ERROR` — simplest, loses actionability.
2. Add a single `SUBSTRATE_OS_ERROR` code with the raw errno in `data` — machine-readable but forces caller to know Linux/macOS errno values.
3. Add granular codes per kernel condition with explicit recovery hints — most actionable for agents.

## Decision Outcome

Chosen option: "Add granular codes per kernel condition", because agents branch on stable string codes, not on raw errno integers.

### New Stable Error Codes

The following six codes extend the taxonomy defined in ADR-0010. Code integers continue the `-320xx` series:

| Code | JSON-RPC code | Triggering errno(s) | Meaning |
|---|---|---|---|
| `SUBSTRATE_SYMLINK_LOOP` | -32014 | `ELOOP` | Symlink chain exceeds OS resolution limit |
| `SUBSTRATE_IO_ERROR` | -32015 | `EIO` | Hardware-level I/O failure (bad sector, device error) |
| `SUBSTRATE_STORAGE_FULL` | -32016 | `ENOSPC`, `EDQUOT` | Destination has no available space or quota exceeded |
| `SUBSTRATE_READ_ONLY_FS` | -32017 | `EROFS` | Target filesystem is mounted read-only |
| `SUBSTRATE_ENCODING_ERROR` | -32018 | — | Path or string contains non-UTF-8 bytes |
| `SUBSTRATE_TRANSIENT_IO` | -32019 | `EBUSY`, `ESTALE`, `EAGAIN` | Resource temporarily unavailable |

#### Recovery Hints (each ≤ 150 characters)

| Code | `recovery_hint` |
|---|---|
| `SUBSTRATE_SYMLINK_LOOP` | `symlink chain exceeds OS resolution limit; verify no cycles in linked paths` |
| `SUBSTRATE_IO_ERROR` | `underlying device reported a hardware error; check disk health and retry` |
| `SUBSTRATE_STORAGE_FULL` | `destination has no available space; free disk space or change target directory` |
| `SUBSTRATE_READ_ONLY_FS` | `target filesystem is mounted read-only; choose a writable destination` |
| `SUBSTRATE_ENCODING_ERROR` | `path contains bytes that cannot be encoded as UTF-8; rename the file or use a different path` |
| `SUBSTRATE_TRANSIENT_IO` | `resource temporarily unavailable; retry after a brief delay` |

**Distinction from `SUBSTRATE_SYMLINK_ESCAPE` (-32003):** SYMLINK_ESCAPE is a security policy violation — the resolved target exits the allowlist boundary. SYMLINK_LOOP is an OS-level failure — the kernel's symlink-resolution counter overflows regardless of policy.

### errno → Substrate Code Mapping Table

Covers Linux and macOS. Errno constants use POSIX names.

| errno | Substrate code | Notes |
|---|---|---|
| `ENOENT` (after canonicalize, race) | `SUBSTRATE_NOT_FOUND` | TOCTOU: path existed during check but not during open |
| `EACCES`, `EPERM` (file ops) | `SUBSTRATE_PERMISSION_DENIED` | Standard file/dir access denial |
| `EPERM` (kill/signal) | `SUBSTRATE_PERMISSION_DENIED` | `structuredContent.hints.error_recovery` overrides with proc-specific hint |
| `ESRCH` | `SUBSTRATE_NOT_FOUND` | `structuredContent.hints.error_recovery`: "process no longer exists" |
| `EMFILE`, `ENFILE` | `SUBSTRATE_RESOURCE_LIMIT` | Hint refined: "process or system FD limit exhausted; reduce concurrent open files or raise ulimit" |
| `ENAMETOOLONG` | `SUBSTRATE_INVALID_ARGUMENT` | Hint refined: "path or name exceeds OS limit; shorten components" |
| `EXDEV` | `SUBSTRATE_INVALID_ARGUMENT` | Hint refined: "cross-device rename not allowed; use copy+delete instead" |
| `EINVAL` (from OS) | `SUBSTRATE_INTERNAL_ERROR` | Indicates a server-side bug passing invalid arguments to syscalls |
| `ELOOP` | `SUBSTRATE_SYMLINK_LOOP` | New |
| `EIO` | `SUBSTRATE_IO_ERROR` | New |
| `ENOSPC`, `EDQUOT` | `SUBSTRATE_STORAGE_FULL` | New |
| `EROFS` | `SUBSTRATE_READ_ONLY_FS` | New |
| `EBUSY`, `ESTALE`, `EAGAIN` | `SUBSTRATE_TRANSIENT_IO` | New; subject to retry policy below |
| `EINTR` | Retried internally | Never surfaces; see retry policy |

### Retry Policy

| errno | Server-side behavior |
|---|---|
| `EINTR` | Retried indefinitely inside substrate. Never surfaces to the caller unless an unrelated error occurs during retry. |
| `EAGAIN`, `EBUSY` | Retried up to 3 times with exponential backoff: 50 ms → 100 ms → 200 ms. After 3 exhausted retries, surfaces as `SUBSTRATE_TRANSIENT_IO`. |
| `ESTALE` (NFS) | Retried once after NFS revalidation. If retry fails, surfaces as `SUBSTRATE_TRANSIENT_IO`. |

All other errno values are not retried and map directly per the table above.

### Consequences

#### Positive

- Agents can branch on `SUBSTRATE_STORAGE_FULL` to suggest freeing space rather than retrying.
- `SUBSTRATE_TRANSIENT_IO` signals retry-eligibility without requiring the caller to know errno semantics.
- Hardware errors (`SUBSTRATE_IO_ERROR`) are surfaced for operator alerting rather than silently swallowed.
- SYMLINK_LOOP and SYMLINK_ESCAPE remain distinct, preserving the security audit signal.

#### Negative

- Adapter must be extended with six new `From` impls.
- CUE schema in `docs/arch/schemas/error-response.cue` must add the six new code literals and enforce the 150-character hint cap.
- `spec validate --lane full` count increases from 13 to 19 codes.

## Validation

- Unit tests assert every new errno variant maps to the correct stable code.
- Integration tests for each new code assert the `data` shape and that `recovery_hint` length ≤ 150 characters.
- CUE schema constraint `len(recovery_hint) <= 150` is added and enforced by `spec validate`.
- EINTR retry is verified by injecting a mock that returns EINTR before success.
- EAGAIN backoff is verified by asserting three retries and exponential delay via a mock clock.

## Links

- [ADR-0010](0010-error-taxonomy.md) — parent error taxonomy (13-code baseline)
- [ADR-0033](0033-transactional-write-pattern.md) — storage-full preflight (proactive ENOSPC avoidance)
- [ADR-0035](0035-path-safety-hardening.md) — path safety (SYMLINK_ESCAPE policy)
- [ADR-0036](0036-startup-error-contract.md) — startup error contract (pre-MCP-session failures)
