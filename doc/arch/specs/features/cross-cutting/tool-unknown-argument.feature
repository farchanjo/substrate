Feature: Unknown arguments to any tool are rejected with SUBSTRATE_INVALID_ARGUMENT in strict mode
  As an LLM agent driving substrate
  I want unrecognized parameters to be explicitly rejected
  So that typos and API drift are caught early rather than silently ignored

  Background:
    Given a running substrate server in strict argument validation mode
    And an allowlist with root "/work/repo"

  Scenario: fs.find with an extra unrecognized argument is rejected
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and bogus=true
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the error object details include field "offending_field" equal to "bogus"

  Scenario: fs.read with an unknown parameter is rejected
    When the client calls fs.read with path="/work/repo/main.rs" and turbo_mode=true
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "offending_field" equal to "turbo_mode"

  Scenario: fs.remove with an unknown parameter is rejected before elicitation check
    When the client calls fs.remove with path="/work/repo/old.rs" and elicitation_confirmed=true and extra_flag=1
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "offending_field" equal to "extra_flag"
    And the file "/work/repo/old.rs" still exists on disk

  Scenario Outline: Multiple tools reject unknown argument "bogus" consistently
    When the client calls <tool> with valid required parameters and bogus=true
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "offending_field" equal to "bogus"

    Examples:
      | tool           |
      | fs.stat        |
      | fs.find        |
      | text.search    |
      | proc.list      |

  Scenario: Known parameter with wrong type is also rejected with SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.find with root=42 and pattern="*.rs"
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "offending_field" equal to "root"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
