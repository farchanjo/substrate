Feature: Elicitation edge cases are handled with appropriate error codes and hints
  As an LLM agent driving substrate
  I want all abnormal elicitation outcomes to produce clear, distinguishable error responses
  So that I can react correctly to user declines, timeouts, bad input, and protocol misuse

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"
    And the file "/work/repo/src/main.rs" exists on disk
    And the connected client advertised the "elicitation" capability during initialization

  Scenario: User explicitly declines elicitation prompt returns SUBSTRATE_CONFIRMATION_REQUIRED
    Given the elicitation prompt is dispatched to the client for fs.remove
    When the user responds to the elicitation with decline=true
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_CONFIRMATION_REQUIRED"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the file "/work/repo/src/main.rs" still exists on disk

  Scenario: User does not respond within 60-second elicitation timeout returns SUBSTRATE_CONFIRMATION_REQUIRED
    Given the elicitation prompt is dispatched to the client for fs.remove
    And the elicitation timeout is configured to 60 seconds
    When 60 seconds elapse without a user response
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_CONFIRMATION_REQUIRED"
    And the error object has field "recovery_hint" mentioning "timeout" or "no response"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the file "/work/repo/src/main.rs" still exists on disk

  Scenario: User response fails schema validation returns SUBSTRATE_INVALID_ARGUMENT
    Given the elicitation prompt expects field "confirm" of type boolean
    When the user responds to the elicitation with confirm="yes" (a string, not a boolean)
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the error object details include field "offending_field" equal to "confirm"
    And the file "/work/repo/src/main.rs" still exists on disk

  Scenario: Tool handler attempts nested elicitation returns SUBSTRATE_INTERNAL_ERROR
    Given the fs.remove handler is configured to attempt a second elicitation call while one is already in flight
    When the client calls fs.remove with path="/work/repo/src/main.rs"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INTERNAL_ERROR"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the file "/work/repo/src/main.rs" still exists on disk
