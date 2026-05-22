Feature: fs.read rejects special files with SUBSTRATE_INVALID_ARGUMENT
  As an LLM agent driving substrate
  I want fs.read to reject FIFOs and sockets rather than blocking indefinitely
  So that I receive a clear error instead of a hung connection when targeting non-regular files

  Background:
    Given an allowlist with root "/work/repo"
    And the path "/work/repo/pipe" is a FIFO (named pipe) on disk
    And the path "/work/repo/sock" is a Unix domain socket on disk

  Scenario: fs.read on a FIFO returns SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.read with path="/work/repo/pipe"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object has field "recovery_hint" matching ".*regular files only.*"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: fs.read on a Unix domain socket returns SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.read with path="/work/repo/sock"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object has field "recovery_hint" matching ".*regular files only.*"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: fs.read on a FIFO does not block — error is returned within 2 seconds
    When the client calls fs.read with path="/work/repo/pipe"
    Then the server returns a response within 2 seconds
    And the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"

  Scenario: fs.read on a FIFO error includes the file type in details
    When the client calls fs.read with path="/work/repo/pipe"
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "file_type" equal to "fifo"

  Scenario: fs.read on a Unix socket error includes the file type in details
    When the client calls fs.read with path="/work/repo/sock"
    Then the error object has field "code" equal to "SUBSTRATE_INVALID_ARGUMENT"
    And the error object details include field "file_type" equal to "socket"

  Scenario: fs.read on a regular file within the allowlist succeeds normally
    Given the file "/work/repo/hello.txt" exists on disk with content "hello"
    When the client calls fs.read with path="/work/repo/hello.txt"
    Then the response does not contain an error object
    And the response content includes "hello"
