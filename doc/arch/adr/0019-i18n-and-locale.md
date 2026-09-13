---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0019 — Internationalization and Locale Handling

## Context and Problem Statement

`substrate` surfaces OS data (file names, process names, dates, numbers, paths) to LLM agents via structured MCP responses. The consuming agent is an LLM; it expects consistent, machine-parseable output regardless of the operator's system locale. If the server inherits the OS locale, number formatting, date formatting, and string collation may vary between deployments, making agent prompts and downstream parsing fragile.

We must also handle the reality that modern filesystems permit arbitrary byte sequences in file names that may not be valid UTF-8. The server must not silently drop or corrupt non-UTF-8 names.

## Decision Drivers

- LLM agents parse structured text; locale-sensitive output (e.g., `1.234,56` vs `1,234.56`) breaks agent extraction.
- ISO 8601 UTC dates are unambiguous across time zones and locales.
- Non-UTF-8 file names are valid on Linux/macOS; silently dropping them causes inconsistent directory listings.
- The server must not interfere with `LANG`/`LC_*` environment variables because they may affect child processes launched by tools (e.g., `ps`, `df`).
- Rust's standard library uses the platform locale only in a narrow set of places; explicit formatting avoids all locale coupling.

## Considered Options

1. All output in en-US; ISO 8601 UTC; C-locale numbers; OsStr preserved with UTF-8 replacement — accepted.
2. Inherit system locale for all output — rejected; makes agent behavior non-deterministic across deployments.
3. Translate all output to the agent's detected language — rejected; substrate has no knowledge of the agent's language; translation adds a dependency and latency.
4. Reject non-UTF-8 file names with an error — rejected; this silently hides files, breaking the contract that tools report all directory contents.
5. Percent-encode non-UTF-8 bytes — rejected; percent-encoding is not a standard representation in JSON strings; it requires the agent to decode, adding complexity.

## Decision Outcome

Chosen option: "en-US fixed locale with UTF-8 replacement for non-UTF-8 OsStr", because it produces consistent, machine-parseable output for LLM agents while preserving visibility into non-UTF-8 file names through a well-defined replacement strategy.

### Output Language

All human-readable strings in MCP responses (error messages, status labels, field names) are written in en-US. No translation layer exists. This applies to:

- Tool error messages (`ToolError` display strings).
- Log messages written to stderr via `tracing`.
- Structured response field values where the value is a status label.

### Date and Time Formatting

All dates and timestamps are formatted as ISO 8601 UTC with the `Z` suffix:

```
2026-05-21T14:32:00Z
```

- File modification times, creation times, and access times are converted from the OS representation to `chrono::DateTime<Utc>` before serialization.
- No locale-sensitive date formatting (e.g., `21/05/2026` or `May 21, 2026`) is used anywhere in MCP responses.
- Sub-second precision is included when the OS provides it: `2026-05-21T14:32:00.123456789Z`.

### Number Formatting

All numeric values in MCP responses use C-locale formatting:

- Decimal separator: `.` (period).
- No thousands separator.
- Integers: decimal, no leading zeros (except hexadecimal addresses prefixed with `0x`).
- Floating-point: Rust's default `Display` for `f64`, which is C-locale compatible.

File sizes are reported in bytes as an integer. Human-readable size strings (e.g., `1.2 MiB`) are provided as a supplementary field, always using SI binary prefixes (KiB, MiB, GiB) and a `.` decimal separator.

### `LANG` / `LC_*` Environment Variables

The server reads `LANG` and `LC_*` at startup for informational logging only (to record the operator environment). It does not set, override, or unset them. Child processes launched by tools inherit the full environment including `LANG`/`LC_*`; this is intentional so that locale-sensitive tools (e.g., `date`, `ls`) behave as configured on the host.

The server itself never calls locale-sensitive C library functions (`setlocale`, `strftime` with locale formats, `strtod` with locale decimal point) because Rust's standard library does not expose them.

### Non-UTF-8 File Names (OsStr Handling)

`std::ffi::OsStr` is the canonical type for file names. When a file name cannot be losslessly converted to UTF-8 (`OsStr::to_str()` returns `None`), the server applies the following strategy:

1. **Preserve**: include the file name in the response.
2. **Replace**: use `OsStr::to_string_lossy()`, which replaces invalid UTF-8 sequences with the Unicode replacement character `U+FFFD` (`\u{FFFD}`).
3. **Flag**: include a boolean field `name_is_lossless: false` in the file entry to signal that the displayed name is not byte-for-byte identical to the on-disk name.

```json
{
  "name": "file\u{FFFD}name",
  "name_is_lossless": false,
  "path": "/some/dir/file\u{FFFD}name"
}
```

This approach:
- Never silently drops files.
- Gives the LLM agent a usable representation.
- Signals that the name cannot be round-tripped without ambiguity.

Tools that operate on files by path accept the lossless path string from a prior listing response; the server reconstructs the original `OsString` by round-tripping through the filesystem (the path string is matched against a re-read directory listing, not reconstructed from the JSON bytes).

### Consequences

#### Positive

- Agent prompts that parse dates, numbers, and file names work identically across macOS, Linux, and any operator locale.
- Non-UTF-8 file names are visible; no silent data loss.
- The `name_is_lossless` flag gives the agent actionable information.

#### Negative

- Non-UTF-8 paths cannot be round-tripped through JSON without additional metadata. The `name_is_lossless: false` flag is a hint but not a full solution; agents that need to operate on such files must be designed to handle this case.
- Date conversion from OS timestamps requires `chrono` (already a transitive dependency via other crates); this must be verified by `cargo-machete` to remain in use.
- The fixed en-US locale is non-negotiable even for deployments where the operator prefers a different language; substrate has no localization infrastructure.

## Validation

- Unit test: construct an `OsString` containing invalid UTF-8 bytes; assert that `to_string_lossy()` produces the replacement character and that `name_is_lossless` is `false`.
- Integration test: run a directory listing on a directory containing a non-UTF-8 file name (Linux only); assert the file appears in the response with the replacement character.
- Property test: for any `chrono::DateTime<Utc>`, assert that the formatted string matches the regex `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$`.
- Code review checklist: no `chrono::Local` usage; no `format!("{:.2}", x)` with locale-sensitive floats; no `LANG`/`LC_ALL` setenv calls.

## Cross-References

- ADR-0009: path sandboxing and filesystem access control; non-UTF-8 path handling interacts with sandbox enforcement.
- ADR-0010: structured MCP response schema; defines the JSON field layout for file entries including `name_is_lossless`.
