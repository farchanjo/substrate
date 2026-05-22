package substrate.no_subprocess

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Rule 1: std::process::Command forbidden in shipped source
# ---------------------------------------------------------------------------

test_shipped_source_without_command_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-fs-query/src/lib.rs": {
                "content": "use nix::fcntl::OFlag;",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_shipped_source_with_std_command_denied if {
    deny["forbidden std::process::Command in crates/substrate-fs-query/src/lib.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-fs-query/src/lib.rs": {
                "content": "let _ = std::process::Command::new(\"ls\");",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 3: tokio::process::Command forbidden in shipped source
# ---------------------------------------------------------------------------

test_shipped_source_with_tokio_command_denied if {
    deny["forbidden tokio::process::Command in crates/substrate-text/src/search.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-text/src/search.rs": {
                "content": "tokio::process::Command::new(\"grep\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_command_in_test_file_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-fs-query/tests/integration.rs": {
                "content": "std::process::Command::new(\"ls\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 4: forbidden crates in Cargo.toml dependencies
# ---------------------------------------------------------------------------

test_forbidden_crate_in_deps_denied if {
    deny["forbidden crate 'duct' in Cargo.toml dependencies — per ADR-0044"] with input as {
        "files": {},
        "cargo_toml_deps": ["tokio", "duct"],
    }
}

test_clean_deps_allowed if {
    count(deny) == 0 with input as {
        "files": {},
        "cargo_toml_deps": ["tokio", "serde", "anyhow"],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 5: build.rs Command requires justification comment
# ---------------------------------------------------------------------------

test_build_rs_command_without_justification_denied if {
    deny["build.rs uses std::process::Command without a no-subprocess-justification comment in crates/substrate-config/build.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-config/build.rs": {
                "content": "std::process::Command::new(\"pkg-config\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_build_rs_command_with_justification_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-config/build.rs": {
                "content": "// no-subprocess-justification: queries platform header version; no pure-Rust alternative exists\nstd::process::Command::new(\"pkg-config\")",
            },
        },
        "cargo_toml_deps": [],
    }
}
