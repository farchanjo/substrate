# ADR-0069 cross-ref: launch tool guidance — every response names the next tool
# ADR-0066 cross-ref: launch.up result carries a resource_link to the events stream
Feature: a launch.up response guides the LLM to the next tool and the event stream
  As a small LLM driving a build workflow
  I want each response to tell me the next tool and where to watch events
  So that I can advance the workflow without external orchestration

  Scenario: launch.up sets the next-tool hint, the destructive flag, and an events link
    Given a trusted Profile is brought up with launch.up
    When the launch.up response is returned
    Then hints.next_action_suggested is the wire name launch_status
    And hints.confirm_destructive is true
    And hints.polling_endpoint is launch.status
    And the result carries a resource_link to launch://stack/<id>/events
