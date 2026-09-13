# ADR-0066 cross-ref: event stream — replay on reconnect is a summary plus tail, not a full dump
Feature: reconnecting to a detached Stack replays a summary plus tail, not the backlog
  As an LLM agent reconnecting to a detached Stack
  I want a distilled catch-up rather than the full event backlog
  So that the model context is not flooded with stale output

  Scenario: reconnect after a large event gap delivers a summary plus the last events
    Given a detached Stack that accumulated more events than the replay cap while no client was attached
    When a client reconnects and reads the events resource from its last cursor
    Then a gap summary aggregating the missed events is delivered
    And only the last N events are replayed in full rather than the entire backlog
