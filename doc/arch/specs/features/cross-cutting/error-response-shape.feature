Feature: Every error response includes code, recovery_hint, and correlation_id
  As an LLM agent driving substrate
  I want all error responses to carry a consistent error envelope
  So that I can present actionable recovery guidance and correlate errors with server logs

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"

  Scenario Outline: Error envelope is well-formed for every defined error code
    Given the server is configured to emit error code <code> for the next matching operation
    When the triggering operation is dispatched
    Then the response contains an error object
    And the error object has field "code" equal to "<code>"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

    Examples:
      | code                              |
      | SUBSTRATE_NOT_FOUND               |
      | SUBSTRATE_PERMISSION_DENIED       |
      | SUBSTRATE_PATH_TRAVERSAL_BLOCKED  |
      | SUBSTRATE_CONFIRMATION_REQUIRED   |
      | SUBSTRATE_CANCELLED               |
      | SUBSTRATE_TIMEOUT                 |
      | SUBSTRATE_INVALID_ARGUMENT        |
      | SUBSTRATE_INTERNAL_ERROR          |
      | SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED |
      | SUBSTRATE_SYMLINK_LOOP            |
      | SUBSTRATE_IO_ERROR                |
      | SUBSTRATE_STORAGE_FULL            |
      | SUBSTRATE_READ_ONLY_FS            |
      | SUBSTRATE_ENCODING_ERROR          |
      | SUBSTRATE_TRANSIENT_IO            |
      | SUBSTRATE_CONFIG_INVALID          |
      | SUBSTRATE_ALLOWLIST_ROOT_MISSING  |
      | SUBSTRATE_FD_LIMIT_TOO_LOW        |

  Scenario: recovery_hint is never empty even for SUBSTRATE_INTERNAL_ERROR
    Given the server is configured to emit SUBSTRATE_INTERNAL_ERROR for the next operation
    When the triggering operation is dispatched
    Then the error object field "recovery_hint" is not an empty string
    And the error object field "recovery_hint" does not exceed 150 characters

  Scenario: correlation_id in error response matches the correlation_id written to stderr
    Given the server is configured to emit SUBSTRATE_IO_ERROR for the next operation
    When the triggering operation is dispatched
    Then the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the server stderr contains a log line whose "correlation_id" matches the response correlation_id
