Feature: Malformed JSON-RPC input is rejected with well-defined protocol errors
  As an LLM agent driving substrate
  I want consistent and safe handling of structurally invalid input
  So that the server never panics or leaks state when receiving malformed messages

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"

  Scenario: params field as array instead of object returns JSON-RPC error -32600
    When the client sends a JSON-RPC message with "params" set to an array value []
    Then the response contains a JSON-RPC error with code -32600
    And the error message describes an invalid request
    And the session remains open for subsequent valid requests

  Scenario: Message exceeding 1 MiB is rejected with JSON-RPC error -32600 and session closed
    When the client sends a JSON-RPC message whose byte length exceeds 1048576
    Then the response contains a JSON-RPC error with code -32600
    And the error message indicates the message size limit was exceeded
    And the server closes the session after sending the error response

  Scenario: Path argument containing an embedded NUL byte returns SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.read with a path argument that contains an embedded NUL byte
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the error object details include field "offending_field" equal to "path"

  Scenario: Request with id set to null is processed normally and response carries id=null
    When the client sends a valid fs.stat request with "id" explicitly set to null
    Then the server processes the request
    And the response carries "id" equal to null
    And no protocol error is returned

  Scenario: Request missing the "jsonrpc" field returns JSON-RPC error -32600
    When the client sends a JSON object that omits the "jsonrpc" field
    Then the response contains a JSON-RPC error with code -32600
    And the session remains open for subsequent valid requests

  Scenario: Request with method set to a non-string value returns JSON-RPC error -32600
    When the client sends a JSON-RPC message where "method" is set to the integer 42
    Then the response contains a JSON-RPC error with code -32600
    And the session remains open for subsequent valid requests
