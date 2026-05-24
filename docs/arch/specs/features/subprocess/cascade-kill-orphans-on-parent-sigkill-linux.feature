# ADR-0053 cross-ref: process lifecycle cascade contract — Linux PR_SET_PDEATHSIG
# ADR-0052 cross-ref: subprocess bounded context — Linux-specific orphan prevention
Feature: Linux PR_SET_PDEATHSIG ensures child dies when substrate is SIGKILL'd
  As an operator
  I want child processes to self-terminate when substrate is killed with SIGKILL
  So that no orphaned processes survive even when graceful shutdown is bypassed

  Scenario: Linux PR_SET_PDEATHSIG ensures child dies when substrate is SIGKILL'd
    Given target OS is Linux
    And a subprocess job is in Running state with PR_SET_PDEATHSIG(SIGTERM) configured in pre_exec
    When substrate receives SIGKILL externally
    Then the kernel delivers SIGTERM to the child within kernel scheduling latency
    And the child exits without becoming an init-orphan
