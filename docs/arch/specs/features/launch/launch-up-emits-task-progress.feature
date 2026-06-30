# ADR-0069 cross-ref: notification contract — launch.up streams per-service progress
# ADR-0049 cross-ref: Tasks primitive — progressToken survives the launch.up task lifetime
Feature: launch.up streams per-service bring-up progress over its Task token
  As a small LLM watching a stack come up
  I want live progress notifications during bring-up
  So that I can follow each service reaching readiness without polling

  Scenario: launch.up emits STARTED and READY progress for each service
    Given a Profile with services db, api, and web is brought up with launch.up
    When the bring-up Task runs
    Then a notifications/progress event is emitted for each service STARTED transition
    And a notifications/progress event is emitted for each service READY transition
    And the progress events carry the launch.up Task progressToken
