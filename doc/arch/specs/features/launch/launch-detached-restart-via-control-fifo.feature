# ADR-0068 cross-ref (2026-07-01 amendment): launch.restart against a
# DETACHED stack routes a dedicated restart command over the control FIFO
# instead of double-spawning the Service in-session behind the supervisor
Feature: launch.restart against a detached stack restarts exactly one Service
  As an operator running a detached long-lived stack
  I want launch.restart to reach the detached supervisor for one named Service
  So that only that Service is freshly spawned, never double-spawned behind the supervisor's back

  Scenario: launch.restart on a detached stack respawns only the named Service
    Given a running detached Stack with Services db and api
    When launch.restart is invoked for api
    Then a restart command naming api is sent to the detached supervisor over the control FIFO
    And the supervisor stops and respawns only api, leaving db untouched
    And launch.restart does not report success until the durable registry shows a fresh api process
    And the restart is not counted against api's crash-loop budget
