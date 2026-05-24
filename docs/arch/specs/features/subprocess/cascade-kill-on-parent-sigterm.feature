# ADR-0053 cross-ref: process lifecycle cascade contract — substrate SIGTERM propagation
# ADR-0032 cross-ref: signal safety (SIGTERM graceful drain)
# ADR-0052 cross-ref: subprocess bounded context — cascade cleanup
Feature: substrate SIGTERM cascades killpg to all active subprocesses
  As an operator shutting down substrate gracefully
  I want SIGTERM to propagate to every active subprocess process group
  So that no child processes are left running after substrate exits

  Scenario: substrate SIGTERM cascades killpg to all active subprocesses
    Given 3 subprocess jobs are in Running state
    When substrate receives SIGTERM
    Then killpg(pgid, SIGTERM) is delivered to each of the 3 pgids
    And after shutdown_drain_secs survivors receive killpg(pgid, SIGKILL)
    And every JobEntry transitions to Cancelled or Killed
    And tmp files registered in each ChildHandle are removed
