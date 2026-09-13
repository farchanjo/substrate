Feature: Concurrent fs.remove calls on the same path resolve cleanly without corruption
  As an LLM agent driving substrate
  I want two simultaneous confirmed removals of the same path to produce exactly one success
  So that concurrency races do not result in panics, undefined errors, or silent double-deletion

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/target.rs" exists on disk
    And both clients have advertised the "elicitation" capability during initialization

  Scenario: Exactly one concurrent fs.remove succeeds and the other returns SUBSTRATE_NOT_FOUND
    When two clients simultaneously call fs.remove with path="/work/repo/target.rs" and elicitation_confirmed=true
    Then exactly one response is a success result confirming deletion
    And exactly one response contains an error object with field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the file "/work/repo/target.rs" does not exist on disk after both calls complete

  Scenario: The SUBSTRATE_NOT_FOUND response from the losing concurrent call includes the standard error envelope
    When two clients simultaneously call fs.remove with path="/work/repo/target.rs" and elicitation_confirmed=true
    Then the error response has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error response has field "recovery_hint" whose length is between 1 and 150 characters
    And the error response has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: Server does not panic or return SUBSTRATE_INTERNAL_ERROR under concurrent remove
    When two clients simultaneously call fs.remove with path="/work/repo/target.rs" and elicitation_confirmed=true
    Then neither response contains an error object with field "code" equal to "SUBSTRATE_INTERNAL_ERROR"
    And the server remains accepting requests after both calls complete

  Scenario: Three concurrent fs.remove calls on the same path yield exactly one success
    Given the file "/work/repo/triple.rs" exists on disk
    When three clients simultaneously call fs.remove with path="/work/repo/triple.rs" and elicitation_confirmed=true
    Then exactly one response is a success result confirming deletion
    And the remaining two responses each contain an error object with field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the file "/work/repo/triple.rs" does not exist on disk after all calls complete
