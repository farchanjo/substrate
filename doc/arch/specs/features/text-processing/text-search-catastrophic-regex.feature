Feature: text.search protects against catastrophic regex backtracking
  As an LLM agent driving substrate
  I want regex patterns prone to exponential backtracking to be bounded in execution time
  So that a malicious or accidentally complex pattern cannot exhaust server resources

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And an allowlist with root "/work/repo"
    And the file "/work/repo/corpus.txt" exists on disk containing a string of 5000 'a' characters

  Scenario: Catastrophic regex pattern returns within the timeout budget
    When the client calls text.search with root="/work/repo" and pattern="(a+)+b"
    Then the server returns a response within 30 seconds
    And the response is either a SUBSTRATE_TIMEOUT error or a normal result
    And no resource exhaustion is observed during the 30-second window

  Scenario: SUBSTRATE_TIMEOUT envelope includes recovery_hint and correlation_id
    Given the server is configured with a regex execution timeout that triggers on catastrophic patterns
    When the client calls text.search with root="/work/repo" and pattern="(a+)+b"
    And the response is a SUBSTRATE_TIMEOUT error
    Then the error object includes the field "code" with value "SUBSTRATE_TIMEOUT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: Server remains responsive after a catastrophic regex attempt
    When the client calls text.search with root="/work/repo" and pattern="(a+)+b"
    Then the server returns a response within 30 seconds
    When the client subsequently calls text.search with root="/work/repo" and pattern="hello"
    Then the server returns a response for the second call within 5 seconds

  Scenario: Simple safe regex on the same corpus completes without timeout
    When the client calls text.search with root="/work/repo" and pattern="aaa"
    Then the response does not contain an error object with code "SUBSTRATE_TIMEOUT"
    And the server returns a result within 5 seconds

  Scenario: Another known catastrophic pattern is also bounded
    Given the file "/work/repo/corpus2.txt" contains a string of 5000 'a' characters followed by 'b'
    When the client calls text.search with root="/work/repo" and pattern="(a|aa)+"
    Then the server returns a response within 30 seconds
    And the response is either a SUBSTRATE_TIMEOUT error or a normal result
    And no resource exhaustion is observed during the 30-second window
