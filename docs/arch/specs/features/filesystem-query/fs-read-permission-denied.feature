Feature: fs.read returns SUBSTRATE_PERMISSION_DENIED when the file is not readable
  As an LLM agent driving substrate
  I want clear permission errors when a file cannot be opened due to OS access restrictions
  So that I can inform the user rather than receiving a silent failure or a misleading error

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/secret.txt" exists on disk with mode 0000

  Scenario: Reading a mode-0000 file returns SUBSTRATE_PERMISSION_DENIED
    When the client calls fs.read with path="/work/repo/secret.txt"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_PERMISSION_DENIED"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: No partial content is returned alongside the SUBSTRATE_PERMISSION_DENIED error
    When the client calls fs.read with path="/work/repo/secret.txt"
    Then the error object has field "code" equal to "SUBSTRATE_PERMISSION_DENIED"
    And the response does not contain a "content" field with file data

  Scenario: SUBSTRATE_PERMISSION_DENIED is returned instead of SUBSTRATE_NOT_FOUND for existing mode-0000 files
    When the client calls fs.read with path="/work/repo/secret.txt"
    Then the error object has field "code" equal to "SUBSTRATE_PERMISSION_DENIED"
    And the error object does not have field "code" equal to "SUBSTRATE_NOT_FOUND"

  Scenario: Reading a world-readable file inside the allowlist succeeds
    Given the file "/work/repo/public.txt" exists on disk with mode 0644 and content "hello"
    When the client calls fs.read with path="/work/repo/public.txt"
    Then the response does not contain an error object
    And the response content includes "hello"

  Scenario Outline: Files with various restrictive modes all return SUBSTRATE_PERMISSION_DENIED
    Given the file "/work/repo/restricted.txt" exists on disk with mode <mode>
    When the client calls fs.read with path="/work/repo/restricted.txt" as a non-root user
    Then the error object has field "code" equal to "SUBSTRATE_PERMISSION_DENIED"

    Examples:
      | mode |
      | 0000 |
      | 0200 |
      | 0020 |
