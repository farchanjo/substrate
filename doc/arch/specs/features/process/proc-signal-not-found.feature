Feature: proc.signal returns SUBSTRATE_NOT_FOUND when the target PID does not exist
  As an LLM agent driving substrate
  I want a clear error when signalling a PID that is not running
  So that I can distinguish a missing process from a permission error or other failure

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And PID 99999 does not refer to any running process on the system

  Scenario: proc.signal with a non-existent PID returns SUBSTRATE_NOT_FOUND
    When the client calls proc.signal with pid=99999 and signal=SIGTERM and elicitation_confirmed=true
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the recovery_hint mentions "process does not exist" or "no such process"

  Scenario: SUBSTRATE_NOT_FOUND for missing PID is not confused with SUBSTRATE_PERMISSION_DENIED
    When the client calls proc.signal with pid=99999 and signal=SIGTERM and elicitation_confirmed=true
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object does not have field "code" equal to "SUBSTRATE_PERMISSION_DENIED"

  Scenario: SUBSTRATE_NOT_FOUND details include the requested PID
    When the client calls proc.signal with pid=99999 and signal=SIGTERM and elicitation_confirmed=true
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object details include field "pid" equal to 99999

  Scenario: proc.signal on a running process within the allowlist proceeds to elicitation
    Given a running process with PID 12345 owned by the current user and within the process allowlist
    When the client calls proc.signal with pid=12345 and signal=SIGTERM and elicitation_confirmed=true
    Then the response does not contain a SUBSTRATE_NOT_FOUND error

  Scenario Outline: proc.signal returns SUBSTRATE_NOT_FOUND for any non-existent PID regardless of signal
    Given PID <pid> does not refer to any running process
    When the client calls proc.signal with pid=<pid> and signal=<signal> and elicitation_confirmed=true
    Then the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object details include field "pid" equal to <pid>

    Examples:
      | pid   | signal  |
      | 99999 | SIGTERM |
      | 99998 | SIGKILL |
      | 99997 | SIGHUP  |
