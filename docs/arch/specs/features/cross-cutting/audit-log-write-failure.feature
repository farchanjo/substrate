Feature: Tool proceeds when audit log write fails and emits a WARN line to stderr
  As an operator of substrate
  I want audit log failures to be non-fatal by default
  So that a broken log target does not block legitimate tool operations

  Background:
    Given a running substrate server with log_write_error_policy=warn_stderr_fallback
    And an allowlist with root "/work/repo"
    And the file "/work/repo/src/old_file.rs" exists on disk
    And the audit log target directory "/var/log/substrate/" is owned by root with mode 0555 (read-only to substrate)

  Scenario: fs.remove proceeds when audit log target is non-writable
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=true
    Then the file "/work/repo/src/old_file.rs" does not exist on disk
    And the tool returns a success result confirming deletion
    And exactly one WARN-level line is written to stderr mentioning the audit log fallback
    And that stderr line is not structured as an error response (no "code" field at root)

  Scenario: The stderr WARN line identifies the audit log path that was not writable
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=true
    Then a WARN-level line is written to stderr
    And that WARN line references the audit log target path "/var/log/substrate/"

  Scenario: Audit log write failure does not produce a SUBSTRATE_IO_ERROR to the client
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=true
    Then the response does not contain an error object
    And the response does not contain field "code" equal to "SUBSTRATE_IO_ERROR"

  Scenario: With log_write_error_policy=fail, audit log write failure blocks the tool
    Given the server is configured with log_write_error_policy=fail
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=true
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_IO_ERROR"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the file "/work/repo/src/old_file.rs" still exists on disk
