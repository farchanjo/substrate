# ADR-0065 cross-ref: reconciler reload — metadata-only change applies without restart
Feature: reloading a metadata-only change does not bounce any process
  As an operator editing a running Stack's Profile
  I want a supervisor-live change to apply without a restart
  So that a healthy process is not needlessly disrupted

  Scenario: changing only restart_policy.max_retries restarts no child
    Given a running Stack with a Service under an OnFailure restart policy
    When the Profile is edited to change only restart_policy.max_retries and reloaded
    Then the reconciler applies the new policy to the live supervisor
    And no child process is restarted
