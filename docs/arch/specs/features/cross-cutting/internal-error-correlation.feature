Feature: Panics inside a tool handler surface as SUBSTRATE_INTERNAL_ERROR with correlated stderr output
  As an operator of substrate
  I want handler panics to produce a structured error response and a matching stderr log entry
  So that production incidents can be diagnosed without losing the correlation between client error and server log

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"
    And the test panic hook is enabled so that the next fs.find call panics inside the handler

  Scenario: Handler panic returns SUBSTRATE_INTERNAL_ERROR to the client
    When the client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INTERNAL_ERROR"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: Panic log line on stderr carries the same correlation_id
    When the client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the error object has field "code" equal to "SUBSTRATE_INTERNAL_ERROR"
    And the server stderr contains a structured log line with level "ERROR" or "PANIC"
    And that stderr log line has field "correlation_id" equal to the response correlation_id
    And that stderr log line includes the panic source file and line number

  Scenario: Server continues accepting requests after recovering from a handler panic
    Given the test panic hook fires and the client receives SUBSTRATE_INTERNAL_ERROR
    When the client subsequently calls fs.find with root="/work/repo" and pattern="*.toml" without the panic hook
    Then the server returns a success response for the second call
    And no SUBSTRATE_INTERNAL_ERROR is returned for the second call

  Scenario: Panic details are not leaked to the client response
    When the client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the error object has field "code" equal to "SUBSTRATE_INTERNAL_ERROR"
    And the error object does not contain a field "stack_trace"
    And the error object does not contain a field "panic_message"
