# ADR-0056 cross-ref: subprocess supervisor semantics — HttpGet health probe
# ADR-0003 cross-ref: outbound-net Cargo feature gate applies to HttpGet variant
Feature: HttpGet health probe drives state transitions through Starting and Ready
  As an LLM agent using substrate
  I want an HttpGet health probe to gate the Ready state transition for a long-lived service
  So that traffic is only routed to the service after it is confirmed healthy

  Background:
    Given subprocess.spawn is invoked with health_probe HttpGet url "http://127.0.0.1:8080/actuator/health" expected_status 200 interval_ms 500 startup_grace_ms 1000
    And the outbound-net Cargo feature is active

  Scenario: probe succeeds after startup grace and job transitions to Ready
    Given the child process has been spawned and the job is in state Starting
    When startup_grace_ms elapses without any probe attempt
    And the probe issues a GET to http://127.0.0.1:8080/actuator/health
    And the response status is 200
    Then the job state transitions to Ready
    And a notifications/progress event is emitted with job_state Ready
    And a SUBSTRATE_SUBPROCESS_STATE_TRANSITION audit event is emitted for the Starting to Ready transition

  Scenario: three consecutive probe failures after Ready cause state transition to Failed
    Given the child process is in state Ready
    When the health probe returns status 503 on three consecutive poll intervals
    Then the job state transitions to Failed
    And a SUBSTRATE_SUPERVISOR_PROBE_FAILED audit event is emitted for each individual failure
    And the restart_policy is applied to the Failed state

  Scenario: probe failures during startup_grace_ms window are not counted toward failure threshold
    Given the child process has been spawned and the job is in state Starting
    And startup_grace_ms has not yet elapsed
    When the health probe returns status 503 during the startup grace window
    Then the consecutive failure counter is not incremented
    And the job remains in state Starting
    And no SUBSTRATE_SUPERVISOR_PROBE_FAILED audit event is emitted for failures within the grace window
