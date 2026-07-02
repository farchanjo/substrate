# ADR-0068/ADR-0056 cross-ref (2026-07-01 amendment): launch.up(detach)
# publishes its durable registry before spawn_all and treats a live,
# correctly pinned supervisor as the complete readiness contract, rather
# than waiting for every Service to finish bringing up
Feature: launch.up under the detach policy returns before every Service is ready
  As an operator bringing up a detached stack with slow health probes
  I want launch.up to confirm the supervisor exists without waiting for every Service
  So that a Service with a multi-minute readiness probe does not make launch.up time out

  Scenario: launch.up under detach returns once the supervisor publishes its registry, before children finish starting
    Given a trusted Profile whose Services include one with a slow health probe
    When launch.up is invoked with on_client_disconnect set to detach
    Then the call succeeds once the durable registry shows the supervisor alive and pinned to the Profile's config hash
    And it does not wait for every Service to reach Ready
    And each Service's own readiness continues to be gated by the supervisor after launch.up returns
