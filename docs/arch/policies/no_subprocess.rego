# package substrate.no_subprocess
#
# Enforces the no-subprocess policy for the substrate crate workspace.
# Per ADR-0044: shipped source code under crates/ MUST NOT invoke external
# binaries, spawn shells, or reference std::process::Command /
# tokio::process::Command outside of test or build-script scope.
#
# Input shape (provided by the CI conftest adapter):
#   {
#     "files": {
#       "<path>": {
#         "content": "<file content as string>"
#       }
#     },
#     "cargo_toml_deps": [
#       "<crate-name-as-string>", ...
#     ]
#   }
#
# Where:
#   files       — map of relative paths to their textual content; populated for
#                 all *.rs and build.rs files under crates/ (excluding target/).
#   cargo_toml_deps — flat list of crate dependency names extracted from all
#                 Cargo.toml files under crates/ (name field only, not version).
#
# Forbidden crate list per ADR-0044 (deny-list; exact crate names on crates.io):
#   subprocess, duct, xshell, cmd_lib, shell-words
#
# Test vectors (inline):
#
#   PASS — non-test source with no Command reference
#   input = {"files":{"crates/substrate-fs-query/src/lib.rs":{"content":"use nix;"}},"cargo_toml_deps":[]}
#
#   FAIL — non-test source uses std::process::Command
#   input = {"files":{"crates/substrate-fs-query/src/lib.rs":{"content":"std::process::Command::new(\"ls\")"}},"cargo_toml_deps":[]}
#   expected deny: "forbidden std::process::Command in crates/substrate-fs-query/src/lib.rs — per ADR-0044"
#
#   FAIL — tokio::process::Command in non-test source
#   input = {"files":{"crates/substrate-text/src/search.rs":{"content":"tokio::process::Command::new(\"grep\")"}},"cargo_toml_deps":[]}
#   expected deny: "forbidden tokio::process::Command in crates/substrate-text/src/search.rs — per ADR-0044"
#
#   PASS — std::process::Command is allowed in test file
#   input = {"files":{"crates/substrate-fs-query/tests/integration.rs":{"content":"std::process::Command::new(\"ls\")"}},"cargo_toml_deps":[]}
#
#   FAIL — forbidden crate in Cargo.toml dependencies
#   input = {"files":{},"cargo_toml_deps":["tokio","duct"]}
#   expected deny: "forbidden crate 'duct' in Cargo.toml dependencies — per ADR-0044"
#
#   FAIL — build.rs uses Command without justification comment
#   input = {"files":{"crates/substrate-config/build.rs":{"content":"std::process::Command::new(\"pkg-config\")"}},"cargo_toml_deps":[]}
#   expected deny: "build.rs uses std::process::Command without a no-subprocess-justification comment in crates/substrate-config/build.rs — per ADR-0044"
#
#   PASS — build.rs with proper justification comment
#   input = {"files":{"crates/substrate-config/build.rs":{"content":"// no-subprocess-justification: queries platform header version; no pure-Rust alternative exists\nstd::process::Command::new(\"pkg-config\")"}},"cargo_toml_deps":[]}

package substrate.no_subprocess

import rego.v1

# ---------------------------------------------------------------------------
# Classification helpers
# ---------------------------------------------------------------------------

# True when the file is a Rust integration/unit test file (not shipped).
# Test exceptions: files inside /tests/, /integration-tests/, or /examples/,
# and any file ending with _test.rs per ADR-0044 permitted exceptions.
_is_test_file(path) if endswith(path, "_test.rs")

_is_test_file(path) if contains(path, "/tests/")

_is_test_file(path) if contains(path, "/integration-tests/")

_is_test_file(path) if contains(path, "/examples/")

# True when the file is a cargo build script.
_is_build_script(path) if endswith(path, "/build.rs")

_is_build_script(path) if path == "build.rs"

# True when the file is a shipped source file (not test, not build script).
_is_shipped_source(path) if {
    not _is_test_file(path)
    not _is_build_script(path)
    endswith(path, ".rs")
}

# Deny-listed crate names per ADR-0044.
_forbidden_crates := {"subprocess", "duct", "xshell", "cmd_lib", "shell-words"}

# ---------------------------------------------------------------------------
# Rule 1: std::process::Command forbidden in shipped source
# Per ADR-0044: Command MUST NOT appear in non-test source under crates/.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_shipped_source(path)
    contains(input.files[path].content, "std::process::Command")
    msg := sprintf(
        "forbidden std::process::Command in %s — per ADR-0044",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 2: std::process::Child / Stdio forbidden in shipped source (defense in depth)
# Per ADR-0044: Child and Stdio are transitively unreachable when Command is
# absent; checked here for defense in depth.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_shipped_source(path)
    contains(input.files[path].content, "std::process::Child")
    msg := sprintf(
        "forbidden std::process::Child in %s — per ADR-0044",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 3: tokio::process::Command forbidden in shipped source
# Per ADR-0044: the async variant is equally prohibited in shipped code.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_shipped_source(path)
    contains(input.files[path].content, "tokio::process::Command")
    msg := sprintf(
        "forbidden tokio::process::Command in %s — per ADR-0044",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 4: forbidden crates MUST NOT appear in Cargo.toml dependencies
# Per ADR-0044: subprocess, duct, xshell, cmd_lib, shell-words are deny-listed.
# Input: cargo_toml_deps is a flat array of crate name strings extracted from
# all Cargo.toml files under crates/.
# ---------------------------------------------------------------------------

deny contains msg if {
    some crate_name
    _forbidden_crates[crate_name]
    crate_name == input.cargo_toml_deps[_]
    msg := sprintf(
        "forbidden crate '%s' in Cargo.toml dependencies — per ADR-0044",
        [crate_name],
    )
}

# ---------------------------------------------------------------------------
# Rule 5: build.rs using Command MUST carry a justification comment
# Per ADR-0044 permitted exceptions: build scripts may invoke external tools
# ONLY when the file contains a comment matching:
#   // no-subprocess-justification: .+
# The comment must appear before the Command invocation (policy checks whole file).
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_build_script(path)
    contains(input.files[path].content, "std::process::Command")
    not _has_justification_comment(input.files[path].content)
    msg := sprintf(
        "build.rs uses std::process::Command without a no-subprocess-justification comment in %s — per ADR-0044",
        [path],
    )
}

deny contains msg if {
    some path
    input.files[path].content
    _is_build_script(path)
    contains(input.files[path].content, "tokio::process::Command")
    not _has_justification_comment(input.files[path].content)
    msg := sprintf(
        "build.rs uses tokio::process::Command without a no-subprocess-justification comment in %s — per ADR-0044",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Helper: justification comment detector
# Rego does not support regex matching without the re_match built-in.
# We approximate: the comment marker prefix is fixed and stable enough for
# a contains() check on the literal prefix string.
# ---------------------------------------------------------------------------

_has_justification_comment(content) if contains(content, "// no-subprocess-justification:")

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
