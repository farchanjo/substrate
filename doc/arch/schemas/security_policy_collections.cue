// DDD role: ValueObject
//
// First-class collection types for the security policy schema.
//
// Split out of security_policy.cue (one-definition-per-file): every list field
// of the policy aggregate is named here so no definition mixes a collection
// field with scalar siblings. #RedactionPatternList is shared with
// runtime_config.cue, whose logging section carries the same pattern list.
//
// Cross-references:
//   ADR-0018 — logging redaction
//   ADR-0052 — subprocess binary and environment allowlists
package schemas

// #ToolNameList is an ordered list of fully-qualified tool names
// (`namespace_name`, wire form per ADR-0062).
#ToolNameList: [...string]

// #SignalList is an ordered list of POSIX signals, restricted to #Signal.
#SignalList: [...#Signal] | *["SIGTERM", "SIGHUP", "SIGINT", "SIGUSR1", "SIGUSR2"]

// #RedactionPatternList is a list of Go-compatible regex patterns whose matches
// are replaced with [REDACTED] before a log line is written per ADR-0018.
// Empty by default.
#RedactionPatternList: [...string] | *[]

// #BinaryPathList is a list of absolute binary paths eligible for execution.
#BinaryPathList: [...#AbsolutePath] | *[]

// #BinaryAllowlistMode selects how #BinaryPathList is enforced: "allow-all"
// (default) admits any regular executable and makes the list inert; "strict"
// admits only the listed entries.
#BinaryAllowlistMode: "allow-all" | "strict" | *"allow-all"

// #EnvVarNameList is a list of environment variable names (names only, never
// values) that a child process may inherit.
#EnvVarNameList: [...string] | *[]
