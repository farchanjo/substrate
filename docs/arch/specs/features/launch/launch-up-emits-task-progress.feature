# ADR-0069 cross-ref: notification contract — launch.up routes lifecycle over tasks/status
# ADR-0049 cross-ref: Tasks primitive — CreateTaskResult{taskId} + notifications/tasks/status
Feature: launch.up streams per-service bring-up lifecycle over its Task status channel
  As a small LLM watching a stack come up
  I want live lifecycle notifications during bring-up
  So that I can follow each service reaching readiness without polling

  Scenario: launch.up emits STARTED and READY lifecycle events for each service
    Given a Profile with services db, api, and web is brought up with launch.up
    When the bring-up Task runs
    Then launch.up returns a CreateTaskResult with a taskId
    And a notifications/tasks/status event is emitted for each service STARTED transition
    And a notifications/tasks/status event is emitted for each service READY transition
    And the tasks/status events carry the launch.up Task taskId
