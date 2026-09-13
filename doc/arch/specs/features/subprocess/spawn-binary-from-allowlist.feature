# ADR-0052 cross-ref: subprocess bounded context — subprocess.spawn tool
# ADR-0004 cross-ref: security model Layer 1 (binary allowlist enforcement)
Feature: Spawn binary that is in the subprocess binary allowlist
  As an LLM agent using substrate
  I want to invoke subprocess.spawn with a binary that has been explicitly allowed
  So that the child process starts and completes normally

  Scenario: Spawn binary that is in subprocess_binary_allowlist
    Given binary "/usr/bin/echo" is in security.subprocess_binary_allowlist
    And elicitation_confirmed is true
    When subprocess.spawn is invoked with binary_path "/usr/bin/echo" and args ["hello"]
    Then the response contains a job_id
    And the JobEntry state transitions through Pending to Running to Succeeded
    And the exit_code is 0
