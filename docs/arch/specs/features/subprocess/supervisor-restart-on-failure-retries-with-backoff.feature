# ADR-0056 cross-ref: subprocess supervisor semantics — OnFailure restart policy
# ADR-0053 cross-ref: process lifecycle cascade contract applies to re-spawned children
Feature: OnFailure restart policy retries with backoff and exhausts max_retries
  As an LLM agent using substrate
  I want a supervised subprocess to restart automatically on non-zero exit up to max_retries times
  So that transient crashes are recovered without requiring a new spawn call

  Background:
    Given subprocess.spawn is invoked with restart_policy OnFailure max_retries 3 backoff_ms 200

  Scenario: child exits non-zero and transitions through Restarting before reaching Running
    Given the child process is in state Running
    When the child exits with exit code 1
    Then the job state transitions to Restarting
    And after backoff_ms elapses the job state transitions to Starting
    And after the child is re-spawned the job state transitions to Running
    And a SUBSTRATE_SUBPROCESS_STATE_TRANSITION audit event is emitted for each transition
    And a SUBSTRATE_SUPERVISOR_RESTARTING audit event is emitted with the exit_code and restart_count

  Scenario: child exceeds max_retries and job reaches terminal Failed
    Given the child process has already been restarted 3 times each with exit code 1
    When the third re-spawned child also exits with exit code 1
    Then the job state transitions to Failed
    And no further restart is attempted
    And a SUBSTRATE_SUPERVISOR_MAX_RETRIES_EXCEEDED error is surfaced in subprocess.result

  Scenario: child exits with code zero and restart is not triggered
    Given the child process is in state Running
    When the child exits with exit code 0
    Then the job state transitions to Succeeded
    And no restart is attempted
    And the job is in a terminal state

  Scenario: retry counter resets after child remains stable for at least twice backoff_ms
    Given the child process has been restarted once and is now in state Running
    When the child remains in Running state for a duration of at least 2 times backoff_ms
    And then the child exits with exit code 1
    Then the restart counter is reset to zero before applying the new restart attempt
    And the job has max_retries restart attempts available for the new failure cycle
