Feature: If PathJail tier 1 is unavailable and refuse_degraded_jail is true, startup aborts
  As an operator with strict path-safety requirements
  I want substrate to refuse to start when kernel-enforced path jailing is unavailable
  So that a silent TOCTOU regression is structurally impossible without explicit operator consent

  Background:
    Given the host kernel does not support openat2 on Linux or O_NOFOLLOW_ANY on macOS
    And has_openat2 is false on Linux or has_o_nofollow_any is false on macOS

  Scenario: Default config refuse_degraded_jail true causes substrate to abort startup with SUBSTRATE_JAIL_DEGRADED_REFUSED
    Given the config key security.refuse_degraded_jail is set to true
    When substrate starts and runs the capability probe
    Then the process exits with a non-zero exit code
    And exactly one JSON line is written to stderr with field "code" equal to "SUBSTRATE_RUNTIME_INIT_FAILED"
    And that JSON line details include a nested error with code "SUBSTRATE_JAIL_DEGRADED_REFUSED"
    And no bytes are written to stdout
    And an audit event with code "SUBSTRATE_JAIL_DEGRADED" is emitted to stderr with severity "warn" before the abort

  Scenario: When refuse_degraded_jail is false substrate proceeds but still emits SUBSTRATE_JAIL_DEGRADED with severity warn
    Given the config key security.refuse_degraded_jail is set to false
    When substrate starts and runs the capability probe
    Then the process does not exit with a non-zero code immediately
    And a tracing warn line indicating degraded path jail is present in stderr before the first MCP initialize response
    And an audit event with code "SUBSTRATE_JAIL_DEGRADED" is emitted to stderr with severity "warn"
    And that audit event includes a field "missing_capability" describing the absent kernel feature
    And substrate continues to accept MCP initialize requests using the userspace strict-path fallback
