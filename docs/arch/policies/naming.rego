# package substrate.naming
#
# Validates naming conventions for all substrate artifacts:
#   - tool_name       : dot-notation, e.g. "fs.read"
#   - error_code      : SCREAMING_SNAKE prefixed with SUBSTRATE_
#   - cue_filename    : snake_case with .cue extension
#   - cue_def         : #PascalCase definition name
#   - adr_filename    : four-digit sequence + kebab-case slug + .md extension
#   - markdown_filename: kebab-case with .md or .mdx extension
#
# Input shape:
#   {
#     "tool_name":          "fs.read",          // optional, omit to skip check
#     "error_code":         "SUBSTRATE_OK",     // optional
#     "cue_filename":       "mcp_tool_spec.cue",// optional
#     "cue_def":            "#ToolSpec",        // optional
#     "adr_filename":       "0004-security-model.md", // optional
#     "markdown_filename":  "getting-started.md"      // optional
#   }
#
# Test vectors (inline):
#
#   PASS — valid tool name
#   input = {"tool_name":"fs.read"}
#
#   PASS — valid error code
#   input = {"error_code":"SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"}
#
#   PASS — valid CUE filename
#   input = {"cue_filename":"mcp_tool_spec.cue"}
#
#   PASS — valid CUE def
#   input = {"cue_def":"#ToolSpec"}
#
#   PASS — valid ADR filename
#   input = {"adr_filename":"0004-security-model.md"}
#
#   PASS — valid markdown filename
#   input = {"markdown_filename":"getting-started.mdx"}
#
#   FAIL — tool name with wrong separator
#   input = {"tool_name":"fs_read"}
#   expected deny: "tool_name 'fs_read': must match ^(fs|proc|sys|text|archive|job|net|subprocess)\\.[a-z_]+$"
#
#   FAIL — unknown namespace in tool name
#   input = {"tool_name":"bogus.tool"}
#   expected deny: "tool_name 'bogus.tool': must match ^(fs|proc|sys|text|archive|job|net|subprocess)\\.[a-z_]+$"
#
#   FAIL — error code missing prefix
#   input = {"error_code":"PATH_OUTSIDE_ALLOWLIST"}
#   expected deny: "error_code 'PATH_OUTSIDE_ALLOWLIST': must match ^SUBSTRATE_[A-Z_]+$"
#
#   FAIL — CUE filename with camelCase
#   input = {"cue_filename":"mcpToolSpec.cue"}
#   expected deny: "cue_filename 'mcpToolSpec.cue': must be snake_case with .cue extension"
#
#   FAIL — CUE def without # sigil
#   input = {"cue_def":"ToolSpec"}
#   expected deny: "cue_def 'ToolSpec': must match ^#[A-Z][a-zA-Z0-9]*$"
#
#   FAIL — ADR filename without leading digits
#   input = {"adr_filename":"security-model.md"}
#   expected deny: "adr_filename 'security-model.md': must match ^[0-9]{4}-[a-z0-9-]+\\.md$"
#
#   FAIL — markdown filename with uppercase
#   input = {"markdown_filename":"GettingStarted.md"}
#   expected deny: "markdown_filename 'GettingStarted.md': must be kebab-case with .md or .mdx extension"

package substrate.naming

import rego.v1

# ---------------------------------------------------------------------------
# Pattern constants
# ---------------------------------------------------------------------------

_tool_name_pattern     := `^(fs|proc|sys|text|archive|job|net|subprocess)\.[a-z_]+$`
_error_code_pattern    := `^SUBSTRATE_[A-Z_]+$`
_cue_filename_pattern  := `^[a-z][a-z0-9_]*\.cue$`
_cue_def_pattern       := `^#[A-Z][a-zA-Z0-9]*$`
_adr_filename_pattern  := `^[0-9]{4}-[a-z0-9-]+\.md$`
_markdown_filename_pattern := `^[a-z0-9][a-z0-9-]*\.(md|mdx)$`

# ---------------------------------------------------------------------------
# deny rules — each fires only when the relevant field is present in input
# ---------------------------------------------------------------------------

deny contains msg if {
    v := input.tool_name
    not regex.match(_tool_name_pattern, v)
    msg := sprintf(
        "tool_name '%s': must match ^(fs|proc|sys|text|archive|job|net|subprocess)\\.[a-z_]+$",
        [v],
    )
}

deny contains msg if {
    v := input.error_code
    not regex.match(_error_code_pattern, v)
    msg := sprintf(
        "error_code '%s': must match ^SUBSTRATE_[A-Z_]+$",
        [v],
    )
}

deny contains msg if {
    v := input.cue_filename
    not regex.match(_cue_filename_pattern, v)
    msg := sprintf(
        "cue_filename '%s': must be snake_case with .cue extension",
        [v],
    )
}

deny contains msg if {
    v := input.cue_def
    not regex.match(_cue_def_pattern, v)
    msg := sprintf(
        "cue_def '%s': must match ^#[A-Z][a-zA-Z0-9]*$",
        [v],
    )
}

deny contains msg if {
    v := input.adr_filename
    not regex.match(_adr_filename_pattern, v)
    msg := sprintf(
        "adr_filename '%s': must match ^[0-9]{4}-[a-z0-9-]+\\.md$",
        [v],
    )
}

deny contains msg if {
    v := input.markdown_filename
    not regex.match(_markdown_filename_pattern, v)
    msg := sprintf(
        "markdown_filename '%s': must be kebab-case with .md or .mdx extension",
        [v],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
