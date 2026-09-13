# ADR-0056 cross-ref: subprocess supervisor semantics — Always restart policy
# ADR-0053 cross-ref: process lifecycle cascade contract applies on every re-spawn
Feature: Always restart policy re-spawns the child on any exit and honours cancel and shutdown
  As an LLM agent using substrate
  I want a supervised subprocess with Always restart policy to be re-spawned regardless of exit code
  So that long-lived service processes remain running without operator intervention

  Background:
    Given subprocess.spawn is invoked with restart_policy Always backoff_ms 500

  Scenario: child exits with code zero and is re-spawned
    Given the child process is in state Running
    When the child exits with exit code 0
    Then the job state transitions to Restarting
    And after backoff_ms elapses the job state transitions to Starting
    And after the child is re-spawned the job state transitions to Running
    And a SUBSTRATE_SUPERVISOR_RESTARTING audit event is emitted

  Scenario: subprocess.cancel terminates the supervisor loop and does not re-spawn
    Given the child process is in state Running
    When subprocess.cancel is called for the job
    Then the supervisor task receives the cancellation token signal
    And the child process is terminated via the cascade kill contract
    And no new child is spawned
    And the job state transitions to Cancelled
    And the job is in a terminal state

  Scenario: substrate graceful shutdown stops the supervisor and cascades SIGTERM to the child
    Given the child process is in state Running
    When substrate receives SIGTERM and begins graceful shutdown drain
    Then the supervisor task stops attempting re-spawns
    And SIGTERM is cascaded to the child process via the cascade kill contract
    And no new child is spawned after the child exits
    And the job state transitions to a terminal state before the shutdown drain completes
