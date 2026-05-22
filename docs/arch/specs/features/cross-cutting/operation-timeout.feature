Feature: Operations cancelled when global timeout is exceeded
  As an LLM agent driving substrate
  I want long-running operations to be interrupted at the configured deadline
  So that the server remains responsive and does not block indefinitely

  Background:
    Given a running substrate server with global_timeout_secs=1
    And an allowlist with root "/work/repo"
    And the directory tree under "/work/repo" is at least 10 levels deep with 500 nodes per level

  Scenario: fs.find on a deep tree exceeds the 1-second deadline
    When the client calls fs.find with root="/work/repo" and pattern="**/*.rs"
    Then the server returns an error response within 2 seconds
    And the error object has field "code" equal to "SUBSTRATE_TIMEOUT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: Timeout error includes the configured deadline in its details
    When the client calls fs.find with root="/work/repo" and pattern="**/*.rs"
    Then the error object has field "code" equal to "SUBSTRATE_TIMEOUT"
    And the error object details include field "timeout_secs" equal to 1

  Scenario: A fast operation completes normally under the same global timeout
    Given the directory "/work/repo" contains exactly 3 files matching "*.toml"
    When the client calls fs.find with root="/work/repo" and pattern="*.toml"
    Then the server returns a success response
    And no SUBSTRATE_TIMEOUT error is emitted

  Scenario: Timeout does not leave partial result chunks on the wire
    When the client calls fs.find with root="/work/repo" and pattern="**/*.rs"
    Then the error object has field "code" equal to "SUBSTRATE_TIMEOUT"
    And no partial result chunks are present in the response stream after the error
