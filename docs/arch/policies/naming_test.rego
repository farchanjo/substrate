package substrate.naming

import rego.v1

# ---------------------------------------------------------------------------
# Tests for tool_name validation
# ---------------------------------------------------------------------------

test_valid_tool_name_allowed if {
    count(deny) == 0 with input as {"tool_name": "fs.read"}
}

test_valid_net_tcp_list_tool_name_allowed if {
    count(deny) == 0 with input as {"tool_name": "net.tcp_list"}
}

test_valid_subprocess_spawn_tool_name_allowed if {
    count(deny) == 0 with input as {"tool_name": "subprocess.spawn"}
}

test_valid_job_result_tool_name_allowed if {
    count(deny) == 0 with input as {"tool_name": "job.result"}
}

test_tool_name_wrong_separator_denied if {
    deny["tool_name 'fs_read': must match ^(fs|proc|sys|text|archive|job|net|subprocess)\\.[a-z_]+$"] with input as {
        "tool_name": "fs_read",
    }
}

test_tool_name_unknown_namespace_denied if {
    deny["tool_name 'bogus.tool': must match ^(fs|proc|sys|text|archive|job|net|subprocess)\\.[a-z_]+$"] with input as {
        "tool_name": "bogus.tool",
    }
}

# ---------------------------------------------------------------------------
# Tests for error_code validation
# ---------------------------------------------------------------------------

test_valid_error_code_allowed if {
    count(deny) == 0 with input as {"error_code": "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"}
}

test_error_code_missing_prefix_denied if {
    deny["error_code 'PATH_OUTSIDE_ALLOWLIST': must match ^SUBSTRATE_[A-Z_]+$"] with input as {
        "error_code": "PATH_OUTSIDE_ALLOWLIST",
    }
}

# ---------------------------------------------------------------------------
# Tests for cue_filename validation
# ---------------------------------------------------------------------------

test_valid_cue_filename_allowed if {
    count(deny) == 0 with input as {"cue_filename": "mcp_tool_spec.cue"}
}

test_camel_case_cue_filename_denied if {
    deny["cue_filename 'mcpToolSpec.cue': must be snake_case with .cue extension"] with input as {
        "cue_filename": "mcpToolSpec.cue",
    }
}

# ---------------------------------------------------------------------------
# Tests for cue_def validation
# ---------------------------------------------------------------------------

test_valid_cue_def_allowed if {
    count(deny) == 0 with input as {"cue_def": "#ToolSpec"}
}

test_cue_def_without_sigil_denied if {
    deny["cue_def 'ToolSpec': must match ^#[A-Z][a-zA-Z0-9]*$"] with input as {
        "cue_def": "ToolSpec",
    }
}

# ---------------------------------------------------------------------------
# Tests for adr_filename validation
# ---------------------------------------------------------------------------

test_valid_adr_filename_allowed if {
    count(deny) == 0 with input as {"adr_filename": "0004-security-model.md"}
}

test_adr_filename_without_leading_digits_denied if {
    deny["adr_filename 'security-model.md': must match ^[0-9]{4}-[a-z0-9-]+\\.md$"] with input as {
        "adr_filename": "security-model.md",
    }
}

# ---------------------------------------------------------------------------
# Tests for markdown_filename validation
# ---------------------------------------------------------------------------

test_valid_markdown_filename_allowed if {
    count(deny) == 0 with input as {"markdown_filename": "getting-started.mdx"}
}

test_uppercase_markdown_filename_denied if {
    deny["markdown_filename 'GettingStarted.md': must be kebab-case with .md or .mdx extension"] with input as {
        "markdown_filename": "GettingStarted.md",
    }
}
