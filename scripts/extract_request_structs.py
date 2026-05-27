#!/usr/bin/env python3
"""extract_request_structs.py — Scan Rust source for request struct metadata.

Produces JSON on stdout in the shape expected by
docs/arch/policies/request_default_invariants.rego (ADR-0061):

  {
    "structs": [
      {
        "name":              "<StructName>",
        "derive_default":    true | false,
        "has_null_shortcut": true | false,
        "fields": [
          { "name": "<field>", "serde_default_fn": "<fn_name>" | null }
        ]
      },
      ...
    ]
  }

Detection rules
---------------
* Scans all ``*.rs`` files under ``crates/`` (or the directory given via CLI).
* A struct is included when its name ends in ``Request``.
* ``derive_default``: ``true`` when ``#[derive(...Default...)]`` appears in
  the attribute block immediately before the ``struct`` keyword, without an
  intervening ``impl Default`` for the same struct elsewhere in the same file.
  A manual ``impl Default for <Name>`` anywhere in the same file sets
  ``derive_default = false`` even if ``#[derive(Default)]`` is also present
  (the manual impl takes priority — serde is bypassed via the shortcut, not derive).
* ``has_null_shortcut``: ``true`` when ``is_null()`` or the literal
  ``empty-object shortcut`` appears within the same file as the struct
  AND adjacent to a ``::default()`` call (heuristic sufficient for this codebase).
* ``serde_default_fn``: non-null when a field line matches
  ``#[serde(... default = "<fn>" ...)]`` (the explicit-function form, NOT
  ``#[serde(default)]`` without a function argument).

Limitations
-----------
* Regex-based, not a full Rust AST parser.  Sufficient for the codebase
  as of ADR-0061; update when struct naming conventions change.
* Inline modules (nested ``mod``) are treated as part of the same file.
* Only the first ``#[derive(...)]`` block before a given struct is inspected.

Usage
-----
  python3 scripts/extract_request_structs.py [crates_dir]
  python3 scripts/extract_request_structs.py crates/substrate-domain/src

Exit codes: 0 on success, 1 on I/O error.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import NamedTuple


# ---------------------------------------------------------------------------
# Regex patterns
# ---------------------------------------------------------------------------

# Matches a #[derive(...)] attribute block that contains "Default".
# Captures the full derive list so we can check for the word "Default".
_DERIVE_RE = re.compile(r"#\[derive\(([^)]+)\)\]")

# Matches a struct declaration (pub or not, with optional generic params).
_STRUCT_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)")

# Matches a manual impl Default for <Name>.
_IMPL_DEFAULT_RE = re.compile(r"\bimpl\s+(?:\w+::)*Default\s+for\s+(\w+)")

# Matches #[serde(default = "fn_name")] — explicit function form only.
# Does NOT match #[serde(default)] without a function argument.
_SERDE_DEFAULT_FN_RE = re.compile(r'#\[serde\([^)]*\bdefault\s*=\s*"([^"]+)"[^)]*\)\]')

# Matches a field declaration: optional visibility, name, colon.
_FIELD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:")

# Heuristic for the is_null()||empty_object shortcut:
# the handler file contains both ".is_null()" and "::default()" in proximity.
_IS_NULL_RE = re.compile(r"\.is_null\(\)")
_DEFAULT_CALL_RE = re.compile(r"::\s*default\(\)")


# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------


class FieldMeta(NamedTuple):
    name: str
    serde_default_fn: str | None


class StructMeta(NamedTuple):
    name: str
    derive_default: bool
    has_null_shortcut: bool
    fields: list[FieldMeta]


# ---------------------------------------------------------------------------
# Core parsing
# ---------------------------------------------------------------------------


def _has_null_shortcut(source: str) -> bool:
    """True when the file contains both is_null() and ::default() calls.

    This is the heuristic for the is_null() || empty_object => Req::default()
    pattern.  Checking co-occurrence in the same file is sufficient for this
    codebase because shortcut handlers and their request structs are always
    co-located.
    """
    return bool(_IS_NULL_RE.search(source)) and bool(_DEFAULT_CALL_RE.search(source))


def _extract_structs(source: str, *, file_shortcut: bool) -> list[StructMeta]:
    """Extract all *Request structs from a single Rust source file."""
    lines = source.splitlines()

    # Collect all struct names that have a manual impl Default anywhere in the file.
    manual_defaults: set[str] = {
        m.group(1) for m in _IMPL_DEFAULT_RE.finditer(source)
    }

    results: list[StructMeta] = []
    i = 0
    while i < len(lines):
        line = lines[i]

        # --- Collect attribute block preceding a struct declaration -----------
        # Walk backward from line i to gather consecutive attribute lines.
        attrs: list[str] = []
        j = i
        # Check if this line itself is a struct declaration.
        struct_match = _STRUCT_RE.match(line)
        if not struct_match:
            i += 1
            continue

        struct_name = struct_match.group(1)
        if not struct_name.endswith("Request"):
            i += 1
            continue

        # Gather attribute lines that immediately precede this struct line.
        k = i - 1
        while k >= 0:
            stripped = lines[k].strip()
            if stripped.startswith("#[") or stripped.startswith("///") or stripped.startswith("//"):
                if stripped.startswith("#["):
                    attrs.append(stripped)
                k -= 1
            else:
                break

        # --- Determine derive_default ----------------------------------------
        has_derive_default = False
        for attr in attrs:
            for dm in _DERIVE_RE.finditer(attr):
                derives = [d.strip() for d in dm.group(1).split(",")]
                if "Default" in derives:
                    has_derive_default = True
                    break

        # Manual impl takes precedence: if there's a manual Default anywhere
        # in this file, derive_default reports false even if #[derive(Default)]
        # is present (manual impl overrides the derived one in Rust).
        if struct_name in manual_defaults:
            has_derive_default = False

        # --- Parse struct body for fields ------------------------------------
        struct_line_has_brace = "{" in line
        i += 1  # move past the struct declaration line
        if not struct_line_has_brace:
            # Opening brace is on a separate line; advance until we find it.
            while i < len(lines) and "{" not in lines[i] and "}" not in lines[i]:
                i += 1
            if i < len(lines) and "{" in lines[i] and "}" not in lines[i]:
                i += 1

        fields: list[FieldMeta] = []
        depth = 1  # brace depth inside the struct body
        pending_serde_fn: str | None = None

        while i < len(lines) and depth > 0:
            fline = lines[i]
            depth += fline.count("{") - fline.count("}")

            # Capture #[serde(default = "fn")] on attribute lines.
            serde_m = _SERDE_DEFAULT_FN_RE.search(fline)
            if serde_m:
                pending_serde_fn = serde_m.group(1)
                i += 1
                continue

            # Skip other attribute lines (doc comments, #[allow], etc.).
            stripped = fline.strip()
            if stripped.startswith("#[") or stripped.startswith("///") or stripped.startswith("//"):
                i += 1
                continue

            # Check for a field declaration.
            if depth == 1:
                fm = _FIELD_RE.match(fline)
                if fm:
                    field_name = fm.group(1)
                    # Skip tuple struct numeric field names and Rust keywords.
                    if not field_name.isdigit():
                        fields.append(FieldMeta(
                            name=field_name,
                            serde_default_fn=pending_serde_fn,
                        ))
                pending_serde_fn = None

            i += 1

        results.append(StructMeta(
            name=struct_name,
            derive_default=has_derive_default,
            has_null_shortcut=file_shortcut,
            fields=fields,
        ))

    return results


def scan_directory(root: Path) -> list[StructMeta]:
    """Walk ``root`` recursively, parse every ``*.rs`` file, return all structs."""
    all_structs: list[StructMeta] = []
    for rs_file in sorted(root.rglob("*.rs")):
        # Skip generated files and test directories if desired.
        try:
            source = rs_file.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"warning: could not read {rs_file}: {exc}", file=sys.stderr)
            continue

        file_shortcut = _has_null_shortcut(source)
        structs = _extract_structs(source, file_shortcut=file_shortcut)
        all_structs.extend(structs)

    return all_structs


# ---------------------------------------------------------------------------
# Deduplication: if the same struct name appears in multiple files (e.g. a
# public type in a library crate re-exported elsewhere), keep the entry where
# derive_default=false (manual impl wins), else the first occurrence.
# ---------------------------------------------------------------------------


def _deduplicate(structs: list[StructMeta]) -> list[StructMeta]:
    seen: dict[str, StructMeta] = {}
    for s in structs:
        if s.name not in seen:
            seen[s.name] = s
        else:
            # Prefer the entry with derive_default=False (manual impl).
            existing = seen[s.name]
            if existing.derive_default and not s.derive_default:
                seen[s.name] = s
    return list(seen.values())


# ---------------------------------------------------------------------------
# Serialisation
# ---------------------------------------------------------------------------


def _to_json(structs: list[StructMeta]) -> dict:
    return {
        "structs": [
            {
                "name": s.name,
                "derive_default": s.derive_default,
                "has_null_shortcut": s.has_null_shortcut,
                "fields": [
                    {"name": f.name, "serde_default_fn": f.serde_default_fn}
                    for f in s.fields
                ],
            }
            for s in structs
        ]
    }


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    root_arg = argv[1] if len(argv) > 1 else "crates"
    root = Path(root_arg)
    if not root.is_dir():
        print(f"error: '{root}' is not a directory", file=sys.stderr)
        return 1

    structs = scan_directory(root)
    structs = _deduplicate(structs)

    print(json.dumps(_to_json(structs), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
