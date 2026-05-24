---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0029 — Threat Model (STRIDE-Lite)

## Context and Problem Statement

Substrate exposes OS capabilities to LLM agents over MCP. The threat surface differs from a typical web service: the attacker is not a remote adversary but an untrusted input source (the LLM itself, or a prompt-injected payload) executing within the operator's trust boundary. A structured threat model is required to enumerate attack scenarios, map them to STRIDE categories, and verify that each threat has a named mitigation implemented by the security model (ADR-0004).

## Decision Drivers

- LLM-generated tool arguments are the primary attack vector; the LLM is not a trusted principal.
- The server runs on the operator's local machine with filesystem and process access.
- STRIDE provides a standard vocabulary for communicating threats to reviewers and auditors.
- Each threat must map to a concrete, verifiable mitigation — not a policy statement.
- The threat model must be updated when new tool categories are added to the surface.

## Considered Options

1. STRIDE-Lite applied to the MCP tool surface (selected)
2. Full STRIDE + DREAD scoring
3. PASTA (Process for Attack Simulation and Threat Analysis)
4. No formal threat model; rely on security model documentation only

## Decision Outcome

Chosen option: "STRIDE-Lite applied to the MCP tool surface", because it provides structured coverage at appropriate depth for a local MCP server without the overhead of full DREAD scoring or PASTA workshops at this stage of the project.

### Attacker profile

Primary attacker: a prompt-injected payload delivered through a user message, document content, or tool output that causes the LLM to emit malicious tool arguments to substrate.

Secondary attacker: a compromised or malicious MCP client process running on the same machine.

Out of scope: network-based remote attackers (substrate binds no network listener by default), and physical access scenarios.

### STRIDE threat table

#### S — Spoofing

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| S-1 | Identity spoofing via MCP session | Attacker replays or forges a `session_id` to impersonate a legitimate session | MCP session IDs are server-generated UUIDs; the server validates session state before dispatching any tool |
| S-2 | Path argument impersonates allowlisted path | LLM supplies `/allowed/path/../../etc/passwd` to pass allowlist prefix check | strict-path resolves and checks the canonical target, not the raw argument; Layer 2 jail (ADR-0004) |

#### T — Tampering

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| T-1 | Path traversal via `../` sequences | LLM emits `fs.remove path=../../../home/user/.ssh/authorized_keys` | strict-path blocks all `../` traversal; verified in property tests |
| T-2 | Zip Slip during archive extraction | Malicious archive contains entry `../../.bashrc`; LLM triggers `archive.extract` | All archive entry paths are resolved against the extraction root via strict-path before any write; entries outside root are rejected |
| T-3 | Symlink escape | LLM supplies a path that resolves through a symlink to outside the allowlist | strict-path resolves symlinks and checks the resolved target against the allowlist |
| T-4 | Null byte injection | LLM supplies `path=/allowed/path\0/etc/passwd` to split OS path parsing | CRLF/null normalizer (ADR-0018) strips null bytes; strict-path rejects paths containing null bytes |

| T-NEW-1 | TOCTOU symlink swap after canonicalize | A symlink is swapped between `canonicalize()` and `open()`, redirecting the open to a path outside the allowlist | `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` on Linux >= 5.6; `O_NOFOLLOW_ANY` on macOS >= 12; kernel-atomic resolution ([ADR-0035](0035-path-safety-hardening.md)) |
| T-NEW-2 | Archive symlink-member chaining | A symlink member is written into the extraction root first; a subsequent member write follows the symlink to a destination outside the root | Archive symlink-member ban: all symlink and hardlink members are rejected with `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` by default ([ADR-0035](0035-path-safety-hardening.md)) |
| T-NEW-3 | macOS Unicode normalization mismatch (NFC vs NFD) | Allowlist root stored as NFC; incoming path argument normalized to NFD by the filesystem; prefix check fails silently or produces a false bypass | Both allowlist roots and incoming arguments are normalized to NFC via `unicode-normalization` before comparison ([ADR-0035](0035-path-safety-hardening.md)) |
| T-NEW-4 | APFS firmlink bypass | A path canonicalized through one APFS mount point resolves to a different byte string than the allowlist root canonicalized through another, defeating the prefix check | `fcntl(fd, F_GETPATH)` re-validates the OS-final path after open against canonical roots on macOS ([ADR-0035](0035-path-safety-hardening.md)) |
| T-NEW-5 | Allowlist root configured as symlink — silent failure | Operator configures an `allowed_paths` entry that is a symlink; prefix check operates on the unresolved symlink path, causing false denials or false allowances | All allowlist roots are canonicalized at config-load; entries that fail canonicalization abort startup with `SUBSTRATE_CONFIG_INVALID` ([ADR-0035](0035-path-safety-hardening.md)) |

#### R — Repudiation

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| R-1 | Mutation without audit trail | LLM performs `fs.remove` and audit log is absent | Every tool invocation emits a structured audit event (ADR-0018) before and after execution; dry-run events are also logged |
| R-2 | Audit log injection | LLM supplies a path containing `\r\n` to inject a fake log line | CRLF stripping normalizer (ADR-0018) runs on all string fields before log emission |

#### I — Information Disclosure

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| I-1 | Secret exfiltration via outbound network | LLM sequences `fs.read` followed by a hypothetical `net.post` to exfiltrate file content | Outbound network is OFF by default; `outbound-net` Cargo feature is not compiled into the default binary (ADR-0020) |
| I-2 | Secret leakage into audit log | LLM embeds an API key in a path argument; the raw argument is logged | Redaction pipeline (ADR-0018) matches known secret patterns and replaces with `[REDACTED]` before log emission |
| I-3 | Directory enumeration beyond scope | LLM uses `fs.list` to enumerate paths outside the allowlist | All `fs.*` tools check the allowlist before any OS call; denied paths return `PATH_DENIED` without disclosing existence |

| I-NEW-1 | Hard link to pre-existing external file | A hard link inside the allowlist shares an inode with a file accessible via a path outside the allowlist; reading the hard link exposes out-of-scope content | `nlink > 1` WARN log on `fs.stat` and `fs.read`; opt-in `reject_hardlinks = true` denies outright; documented as residual risk ([ADR-0035](0035-path-safety-hardening.md)) |

#### D — Denial of Service

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| D-1 | Resource exhaustion via tool sequences | LLM generates a rapid sequence of `archive.extract` operations on large files, exhausting disk or CPU | Dry-run gate limits unintended large operations; elicitation for archive tools requires human confirmation before execution |
| D-2 | Signal-on-init | LLM sends `proc.signal pid=1 signal=SIGKILL` targeting init/systemd | Allowlist for `proc.*` tools limits which PIDs are addressable; SIGKILL targeting PID 1 requires elicitation confirmation |
| D-3 | Infinite archive nesting (zip bomb) | Archive extraction triggers recursive expansion of a deeply nested archive | Extraction depth and decompressed size limits are enforced at the archive layer; operation is aborted and logged |

| D-NEW-1 | ENOSPC mid-write leaves partial file | Disk fills during `fs.write` or `archive.extract`; a partial file is written under the final target name and observed by subsequent tool calls | `statvfs` preflight before write; write to `.tmp.<uuid7>` sibling; atomic rename on success; remove `.tmp` on cancellation or error ([ADR-0033](0033-transactional-write-pattern.md)) |
| D-NEW-2 | SIGBUS via concurrent truncation of mmap'd file | A file is memory-mapped for blake3 hashing; a concurrent writer truncates the file; the kernel delivers SIGBUS to the substrate process | `blake3` mmap disabled; file content is read via buffered I/O; SIGBUS from mmap'd files is structurally prevented ([ADR-0032](0032-signal-safety.md)) |

#### E — Elevation of Privilege

| ID | Threat | Scenario | Mitigation |
|---|---|---|---|
| E-1 | Writing to privileged paths | LLM supplies an allowlisted parent path; attempts to write to a child path with elevated permissions | `fs.set_permissions` requires explicit `dry_run: false` and elicitation; allowlist enforcement applies to all writes |
| E-2 | SIGKILL/SIGTERM/SIGSTOP on arbitrary PIDs | LLM targets a system daemon PID via `proc.signal` | SIGKILL, SIGTERM, and SIGSTOP all trigger elicitation (ADR-0004 Layer 4); no signal is sent without human confirmation |

### Residual risks

| Risk | Rationale | Treatment |
|---|---|---|
| Prompt injection escalation | A sufficiently crafted document could cause an LLM to confirm an elicitation form autonomously | Out of scope for substrate to solve; requires host-level prompt injection defenses |
| Zero-day in strict-path | A vulnerability in the `strict-path` crate could allow path escape | Dependency pinning, `cargo audit` in CI, prompt upstream disclosure |
| Operator misconfiguration of allowlist | An overly broad `allowed_paths` entry (e.g., `/`) defeats the allowlist | Documentation warns against broad entries; future lint pass on config load |

### Consequences

#### Positive

- Every identified threat has a named, implemented mitigation traceable to ADR-0004 or ADR-0018.
- STRIDE categorization supports structured security review and penetration testing scope definition.
- Residual risks are documented explicitly rather than silently assumed.

#### Negative

- STRIDE-Lite does not include DREAD likelihood scoring; prioritization of mitigations requires judgment.
- Attacker scenarios are limited to the current tool surface; new tools must trigger a threat model review.

## Validation

- Security review checklist requires STRIDE table update for each new tool category PR.
- Property-based test suite (proptest) generates adversarial inputs targeting T-1 through T-4.
- CI gate: `cargo audit` with zero high/critical advisories.
- Elicitation integration tests verify D-2 and E-2 scenarios require confirmation before execution.

## Cross-References

- ADR-0004 — Security model (implements mitigations referenced in this document)
- ADR-0016 — Archive tool design (Zip Slip mitigation details)
- ADR-0017 — Process signal tool design (SIGKILL/SIGTERM/SIGSTOP elicitation details)
- [ADR-0032](0032-signal-safety.md) — Signal safety (SIGBUS via mmap mitigation, D-NEW-2)
- [ADR-0033](0033-transactional-write-pattern.md) — Transactional write pattern (ENOSPC and cancellation mitigation, D-NEW-1)
- [ADR-0035](0035-path-safety-hardening.md) — Path safety hardening (TOCTOU, firmlink, Unicode, /proc, archive symlink-member, hard-link threats)

## Amendments

### 2026-05-24 — STRIDE expansion for subprocess BC (ADR-0052)

[ADR-0052](0052-subprocess-execution-architecture.md) introduces the subprocess bounded context, which substantially expands the threat surface. The following STRIDE entries are added to the threat table. They use the same format as existing `T-NEW-*` entries above; GFM tables are not used per spec conventions.

**S-NEW-3 — Child process spoofs identity via argv[0] manipulation.**
Scenario: a LLM-supplied binary name sets argv[0] to a trusted program name, causing audit logs to attribute actions to a different process.
Mitigation: substrate preserves the argv verbatim from the SubprocessRequest value object without modification. Every audit event emitted for the child carries both the `binary_path` resolved through the binary allowlist and a `binary_hash` (blake3 of the executable at spawn time) alongside the `correlation_id` and `job_id`. Identity is tied to the spawn record, not to argv[0].
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md).

**T-NEW-6 — Env-var injection via LLM-supplied env map containing LD_PRELOAD or equivalent.**
Scenario: the LLM includes `LD_PRELOAD=/tmp/evil.so` in the `subprocess.spawn` env map, causing the child to load a malicious shared library that escapes containment.
Mitigation: the env-var allowlist (Layer 5 of ADR-0004) hard-bans `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `LD_LIBRARY_PATH`, and `DYLD_LIBRARY_PATH` independently of the allowlist. Any spawn request containing a hard-banned key is rejected with `SUBPROCESS_ENV_BANNED` before construction of `tokio::process::Command`.
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md).

**R-NEW-1 — Stream chunk emitted to client without correlation to spawn.**
Scenario: stdout or stderr stream chunks arrive at the client with no way to associate them with the originating `subprocess.spawn` call, enabling repudiation of which child produced which output.
Mitigation: every stdout and stderr chunk carries `job_id` and `correlation_id` in its structuredContent envelope per [ADR-0054](0054-subprocess-stream-capture.md). The audit event trail for each chunk includes both identifiers, ensuring complete traceability from spawn to every byte delivered.
Cross-reference: [ADR-0054](0054-subprocess-stream-capture.md).

**I-NEW-2 — Child writes secret to stdout, surfaced to LLM via stream.**
Scenario: the child process emits an API key or credential to stdout; the stream is forwarded to the LLM agent without sanitization, causing information disclosure.
Mitigation: the redaction filter defined in [ADR-0018](0018-logging-redaction.md) is extended to cover subprocess stdout and stderr stream chunks. Redaction runs on each chunk before it is placed into the mpsc channel for delivery, ensuring secrets matching known patterns are replaced with `[REDACTED]` before any forwarding occurs.
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md), [ADR-0054](0054-subprocess-stream-capture.md).

**D-NEW-3 — Child fork-bomb exhausts process table.**
Scenario: the spawned child process forks recursively (intentionally or due to a bug), exhausting the process table and causing denial of service for the substrate server and the host OS.
Mitigation: process group ID (pgid) scoped `killpg(SIGKILL)` is triggered on Cancelled or TimedOut JobEntry terminal transitions. The per-client quota `subprocess.max_per_client` and the global `subprocess.max_concurrent` limit the number of concurrently active subprocess jobs. On Linux, `RLIMIT_NPROC` is set on the child when the `sandbox` Cargo feature is enabled, capping the total number of child processes spawnable by the child's UID.
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md), [ADR-0053](0053-subprocess-process-group-lifecycle.md).

**E-NEW-1 — Child inherits substrate UID/GID/privileges.**
Scenario: the spawned child runs as the same user as substrate, inheriting all its filesystem access and OS capabilities, allowing the child to act with full substrate privileges.
Mitigation: the optional `subprocess.drop_privs_to_uid` configuration key causes substrate to call `setuid(target_uid)` in the `pre_exec` hook before exec, dropping to a less-privileged UID. When `drop_privs_to_uid` is unset, this is documented as a residual risk in the operator guide; the binary allowlist and env allowlist (Layer 5 of ADR-0004) reduce but do not eliminate the privilege surface.
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md).

**E-NEW-2 — Child escapes cwd via chdir() syscall after spawn.**
Scenario: the spawned child calls `chdir("/")` after process start, escaping the cwd restriction enforced at spawn time, and subsequently accesses files outside the intended working directory.
Mitigation: the cwd PathJail in Layer 5 covers only spawn-time argument validation; it cannot prevent a cooperative child from calling `chdir()` post-spawn. This is documented as a residual risk. The binary allowlist restricts which programs may be executed, reducing the likelihood that an unintended program with `chdir` behavior is spawned.
Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md).

**D-NEW-4 — Subprocess orphaned on Linux SIGKILL of substrate.**
Scenario: `SIGKILL` is delivered to the substrate process on Linux; the child process continues running as an orphan adopted by init/systemd, consuming resources and potentially executing actions that were intended to be cancelled.
Mitigation: `PR_SET_PDEATHSIG(SIGTERM)` is set in the `pre_exec` hook before exec per [ADR-0053](0053-subprocess-process-group-lifecycle.md). When the parent process dies, the kernel delivers `SIGTERM` to the child automatically. Child binaries MUST implement a `SIGTERM` cleanup handler; this is documented as a contract requirement in the operator guide.
Cross-reference: [ADR-0053](0053-subprocess-process-group-lifecycle.md).

**D-NEW-5 — Subprocess orphaned on macOS SIGKILL of substrate.**
Scenario: `SIGKILL` is delivered to the substrate process on macOS; `PR_SET_PDEATHSIG` is not available; the child process continues as an orphan.
Mitigation: a watchdog pipe (cooperative, per [ADR-0053](0053-subprocess-process-group-lifecycle.md)) is established between substrate and each child at spawn. When the substrate process dies, the write end of the pipe closes; a cooperative child monitoring the read end receives EOF and self-terminates. At next substrate startup, an orphan reaper routine per [ADR-0055](0055-subprocess-orphan-reaper.md) scans for processes matching the watchdog pipe fingerprint and terminates them. Non-cooperative binaries that do not monitor the watchdog pipe represent a residual risk documented in the operator guide.
Cross-reference: [ADR-0053](0053-subprocess-process-group-lifecycle.md), [ADR-0055](0055-subprocess-orphan-reaper.md).
