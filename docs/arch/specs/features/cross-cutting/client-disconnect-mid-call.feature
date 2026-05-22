Feature: substrate cancels in-flight operations and exits cleanly on client disconnect
  As an operator of substrate
  I want the server to detect stdin EOF and shut down gracefully
  So that orphaned background work does not consume resources after the client has gone away

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"
    And the directory tree under "/work/repo" contains at least 10,000 files

  Scenario: stdin EOF cancels the in-flight fs.find via CancellationToken
    Given the client has dispatched fs.find with root="/work/repo" and pattern="**/*.rs" which is running
    When the client closes stdin (EOF) before the operation completes
    Then the CancellationToken associated with the fs.find handler is signalled as cancelled
    And no further bytes are written to stdout after the EOF is detected

  Scenario: Substrate drains in-flight work for up to 5 seconds then exits 0
    Given the client has dispatched fs.find which is running
    When the client closes stdin
    Then the server waits at most 5 seconds for the handler to complete or cancel
    And the process exits with code 0

  Scenario: Process exits 0 even if the in-flight operation does not finish within 5 seconds
    Given the client has dispatched an operation that ignores CancellationToken and runs indefinitely
    When the client closes stdin
    Then the server forcibly terminates the operation after 5 seconds
    And the process exits with code 0

  Scenario: No partial JSON-RPC result is written to stdout after EOF
    Given the client has dispatched fs.find which is running and has begun emitting chunks
    When the client closes stdin mid-stream
    Then no additional JSON-RPC messages are written to stdout after the EOF is detected
    And the stdout stream is left in a consistent state (no partial JSON frames)

  Scenario: Subsequent fs.find requests are not accepted after stdin EOF
    Given the client closes stdin (EOF)
    When a new JSON-RPC request arrives on a different channel after EOF
    Then the server does not process the new request
    And the process exits with code 0
