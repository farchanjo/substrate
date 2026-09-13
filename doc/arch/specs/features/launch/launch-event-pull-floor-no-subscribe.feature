# ADR-0066 cross-ref: a client without resources.subscribe degrades to the launch.logs pull floor
Feature: a client without resource subscriptions uses the pull floor
  As a client that does not support resources/subscribe
  I want events to remain available without push notifications
  So that the pull floor is always the contract

  Scenario: no subscription is attempted for a non-subscribing client
    Given a client that does not advertise resources.subscribe
    When a Stack emits lifecycle and semantic events
    Then no notifications/resources/updated poke is sent
    And the events remain readable via launch.status and launch.logs polling
