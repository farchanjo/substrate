Feature: Tools requiring elicitation return SUBSTRATE_CONFIRMATION_REQUIRED when client lacks the capability
  As a safety boundary in substrate
  I want confirmation-gated operations to fail gracefully when the client cannot perform elicitation
  So that destructive actions are never executed silently by clients that cannot prompt the user

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"
    And the file "/work/repo/src/main.rs" exists on disk
    And the connected client did not advertise the "elicitation" capability during initialization

  Scenario: fs.remove without elicitation capability returns SUBSTRATE_CONFIRMATION_REQUIRED
    When the client calls fs.remove with path="/work/repo/src/main.rs"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_CONFIRMATION_REQUIRED"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the recovery_hint mentions either "elicitation" capability or "elicitation_confirmed" flag
    And the file "/work/repo/src/main.rs" still exists on disk

  Scenario: fs.remove with explicit elicitation_confirmed=true bypasses the capability check
    When the client calls fs.remove with path="/work/repo/src/main.rs" and elicitation_confirmed=true
    Then the file "/work/repo/src/main.rs" does not exist on disk
    And the tool returns a success result confirming deletion

  Scenario: proc.signal with SIGKILL without elicitation capability returns SUBSTRATE_CONFIRMATION_REQUIRED
    Given a running process with PID 12345 owned by the current user
    When the client calls proc.signal with pid=12345 and signal=SIGKILL
    Then the error object has field "code" equal to "SUBSTRATE_CONFIRMATION_REQUIRED"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the recovery_hint mentions either "elicitation" capability or "elicitation_confirmed" flag

  Scenario: Client advertising elicitation capability is prompted rather than rejected
    Given the connected client advertised the "elicitation" capability during initialization
    When the client calls fs.remove with path="/work/repo/src/main.rs"
    Then the server initiates an elicitation request to the client
    And no immediate SUBSTRATE_CONFIRMATION_REQUIRED error is returned
