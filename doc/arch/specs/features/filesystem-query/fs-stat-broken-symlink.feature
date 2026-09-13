Feature: fs.stat on a broken symlink returns SUBSTRATE_NOT_FOUND without panicking
  As an LLM agent driving substrate
  I want fs.stat to distinguish a dangling symlink from a genuine panic or internal error
  So that I receive a clean actionable error rather than an opaque SUBSTRATE_INTERNAL_ERROR

  Background:
    Given an allowlist with root "/work/repo"
    And the symlink "/work/repo/dead_link" exists and points to "/work/repo/nonexistent"
    And "/work/repo/nonexistent" does not exist on disk

  Scenario: fs.stat on a dangling symlink returns SUBSTRATE_NOT_FOUND
    When the client calls fs.stat with path="/work/repo/dead_link"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: SUBSTRATE_NOT_FOUND is returned and not SUBSTRATE_INTERNAL_ERROR
    When the client calls fs.stat with path="/work/repo/dead_link"
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object does not have field "code" equal to "SUBSTRATE_INTERNAL_ERROR"

  Scenario: Server continues accepting requests after a broken symlink stat
    When the client calls fs.stat with path="/work/repo/dead_link"
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    When the client subsequently calls fs.stat with path="/work/repo" (the root directory)
    Then the server returns a success response for the second call

  Scenario: fs.stat on a valid symlink pointing to an existing file succeeds
    Given the symlink "/work/repo/good_link" exists and points to "/work/repo/main.rs"
    And "/work/repo/main.rs" exists on disk
    When the client calls fs.stat with path="/work/repo/good_link"
    Then the response does not contain an error object
    And the response includes file metadata

  Scenario: The recovery_hint for a broken symlink does not mention SUBSTRATE_INTERNAL_ERROR
    When the client calls fs.stat with path="/work/repo/dead_link"
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object field "recovery_hint" does not contain the string "SUBSTRATE_INTERNAL_ERROR"
