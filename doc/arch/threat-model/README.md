# Threat Model

## Overview

Substrate's threat model is maintained inline as a STRIDE-Lite analysis in
[ADR-0029](../adr/0029-threat-model.md). That ADR is the canonical source: it
defines the attacker profile, enumerates concrete attack scenarios mapped to the
six STRIDE categories, names the implemented mitigation for each threat, and
records the residual risks that substrate does not solve on its own.

This directory holds no threat content today. It is reserved for future expanded
per-bounded-context threat documents, should any bounded context grow a threat
surface large enough to warrant a standalone file rather than an amendment to
ADR-0029.

## Attacker profile

The primary attacker is a prompt-injected payload, delivered through a user
message, document content, or tool output, that causes the LLM to emit malicious
tool arguments to substrate. The secondary attacker is a compromised or
malicious MCP client process on the same machine. Network-based remote attackers
are out of scope because substrate binds no network listener by default, as are
physical-access scenarios. See [ADR-0029](../adr/0029-threat-model.md) for the
full profile.

## STRIDE categories covered

[ADR-0029](../adr/0029-threat-model.md) covers all six STRIDE categories. Each
entry below is the summary; the ADR carries the full scenario and the residual
risk.

### TM-001: Spoofing

**Category:** Spoofing.

**Description:** MCP session-ID forgery, and path arguments that impersonate an allowlisted path.

**Mitigations:**

- Server-generated session UUIDs.
- Canonical path resolution through the path jail.

### TM-002: Tampering

**Category:** Tampering.

**Description:** Path traversal, Zip Slip during archive extraction, symlink escape, null-byte injection, TOCTOU symlink swap, archive symlink-member chaining, macOS Unicode normalization mismatch, APFS firmlink bypass, and symlink allowlist roots.

**Mitigations:**

- `strict-path` resolution, kernel-atomic (`openat2` / `O_NOFOLLOW_ANY`).
- Archive symlink-member ban.
- NFC normalization plus `F_GETPATH` re-validation.
- Config-load canonicalization.

### TM-003: Repudiation

**Category:** Repudiation.

**Description:** Mutation without an audit trail, and audit-log line injection.

**Mitigations:**

- Structured audit events around every invocation.
- CRLF stripping on logged string fields.

### TM-004: Information Disclosure

**Category:** Information disclosure.

**Description:** Secret exfiltration over the outbound network, secret leakage into the audit log, directory enumeration beyond scope, and hard links to pre-existing external files.

**Mitigations:**

- Outbound network off by default.
- The redaction pipeline.
- Pre-call allowlist checks.
- `nlink` warnings with an opt-in hard-link rejection.

### TM-005: Denial of Service

**Category:** Denial of service.

**Description:** Resource exhaustion via tool sequences, signal-on-init, zip bombs, ENOSPC mid-write partial files, SIGBUS via concurrent mmap truncation, child fork-bombs, and subprocess orphans on substrate SIGKILL.

**Mitigations:**

- Dry-run and elicitation gates.
- Depth and size limits, plus transactional writes.
- Disabled blake3 mmap.
- Per-client and global subprocess quotas.
- Death-signal and watchdog-pipe orphan controls.

### TM-006: Elevation of Privilege

**Category:** Elevation of privilege.

**Description:** Writing to privileged paths, destructive signals on arbitrary PIDs, env-var injection (LD_PRELOAD and equivalents), a child inheriting substrate's UID, and a child escaping cwd via `chdir`.

**Mitigations:**

- Elicitation gates on permission changes and destructive signals.
- The env-var hard-ban list.
- The optional UID drop.
- The binary allowlist.

## Subprocess threat expansion

The subprocess bounded context substantially expands the threat surface; its
additional STRIDE entries are amended into
[ADR-0029](../adr/0029-threat-model.md) (amendment dated 2026-05-24). The
operator-facing contracts deferred by threats E-NEW-1 (UID inheritance), D-NEW-4
(Linux orphan cleanup), and D-NEW-5 (macOS orphan cleanup) are documented in the
[operator guide](../operations/operator-guide.md).

## Cross-references

- [ADR-0029](../adr/0029-threat-model.md) - the inline STRIDE-Lite threat model
- [ADR-0004](../adr/0004-security-model.md) - security model implementing the
  named mitigations
- [ADR-0035](../adr/0035-path-safety-hardening.md) - path-safety hardening
  (TOCTOU, firmlink, Unicode, archive symlink-member, hard-link threats)
- [operator guide](../operations/operator-guide.md) - deferred subprocess
  contracts
