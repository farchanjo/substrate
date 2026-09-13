# ADR-0066 cross-ref: event stream — redaction at the source before the event-log or model context
Feature: a secret printed by a child is redacted before it reaches any consumer
  As an operator running substrate
  I want matching output redacted at the source
  So that a secret a child prints never reaches the event-log or the model context

  Scenario: a line matching a redact pattern is stored and emitted redacted
    Given a Service whose redact patterns match a secret token
    When the child prints a line containing that token
    Then the line written to the event-log is redacted
    And the event delivered to the client is redacted
