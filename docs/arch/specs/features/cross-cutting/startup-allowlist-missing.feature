Feature: substrate exits with code 77 when the configured allowlist root does not exist
  As an operator deploying substrate
  I want a clear and structured error on startup when the allowlist path is invalid
  So that misconfiguration is immediately visible and actionable before any client connects

  Background:
    Given substrate is configured with allowlist root "/nonexistent/path/that/does/not/exist"

  Scenario: Startup fails with exit code 77 and a structured stderr JSON line
    When substrate starts
    Then the process exits with code 77
    And exactly one JSON line is written to stderr
    And that JSON line has field "code" equal to "SUBSTRATE_ALLOWLIST_ROOT_MISSING"
    And that JSON line has field "recovery_hint" whose length is between 1 and 150 characters
    And that JSON line has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And that JSON line has field "timestamp" in ISO 8601 format

  Scenario: No stdout output is produced on startup failure
    When substrate starts
    Then the process exits with code 77
    And no bytes are written to stdout

  Scenario: The structured stderr line includes the configured path that was missing
    When substrate starts
    Then the process exits with code 77
    And the stderr JSON line details include field "path" equal to "/nonexistent/path/that/does/not/exist"

  Scenario: Startup succeeds when the allowlist root exists and is a directory
    Given substrate is configured with allowlist root "/work/repo"
    And the directory "/work/repo" exists on disk
    When substrate starts
    Then the process does not exit immediately with a non-zero code
    And no SUBSTRATE_ALLOWLIST_ROOT_MISSING error is emitted
