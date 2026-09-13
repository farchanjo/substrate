Feature: No-subprocess policy is verified at startup and emits an audit event
  As a security auditor reviewing a substrate deployment
  I want a structured audit event confirming no subprocess paths are reachable in the binary
  So that the no-subprocess contract from ADR-0044 is observable in the runtime audit trail

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: After capability probe completes substrate emits SUBSTRATE_SUBPROCESS_POLICY_VERIFIED with optional binary hash
    Given substrate has completed the capability probe phase at startup
    When the composition root finishes initializing all port factories
    Then exactly one audit event with code "SUBSTRATE_SUBPROCESS_POLICY_VERIFIED" is written to stderr
    And that audit event has a non-empty "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And that audit event has a "timestamp" in ISO 8601 format
    And that audit event optionally includes field "binary_hash" containing a 64-character lowercase hex string

  Scenario: The policy verification audit event is emitted even when PathJail is operating in degraded tier
    Given the host kernel lacks openat2 support on Linux or O_NOFOLLOW_ANY on macOS
    And the config key security.refuse_degraded_jail is set to false
    When substrate completes startup in degraded jail mode
    Then an audit event with code "SUBSTRATE_SUBPROCESS_POLICY_VERIFIED" is still emitted to stderr
    And that event is emitted after the SUBSTRATE_JAIL_DEGRADED audit event

  Scenario: Build-time policy gate failure prevents the binary from being shipped so no runtime emission is possible
    Given the Rego policy no_subprocess.rego is wired into spec validate lane full in CI
    When a pull request introduces std::process::Command in a non-test source file under crates
    Then conftest test against no_subprocess.rego exits non-zero
    And the CI gate blocks the merge
    And no SUBSTRATE_SUBPROCESS_POLICY_VERIFIED event can be emitted because no binary is produced
