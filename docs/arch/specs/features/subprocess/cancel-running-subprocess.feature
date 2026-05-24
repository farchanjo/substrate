# ADR-0053 cross-ref: process lifecycle cascade contract (SIGTERM then SIGKILL on drain timeout)
# ADR-0052 cross-ref: subprocess bounded context — subprocess.cancel tool
Feature: subprocess.cancel triggers SIGTERM then SIGKILL on drain timeout
  As an LLM agent using substrate
  I want subprocess.cancel to terminate the child process and its process group reliably
  So that no orphaned processes accumulate when a long-running spawn is abandoned

  Scenario: subprocess.cancel triggers SIGTERM then SIGKILL on drain timeout
    Given a subprocess job is in Running state with a sleep-100s binary
    When subprocess.cancel is invoked with the job_id
    Then killpg(pgid, SIGTERM) is delivered within 50ms
    And after shutdown_drain_secs the child is reaped
    And the JobEntry transitions to Cancelled
    And all stdout and stderr mpsc buffers are drained
