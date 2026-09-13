# ADR-0066 cross-ref: the global redact denylist redacts even when the per-service list is empty
Feature: the global redaction denylist applies when the per-service list is empty
  As an operator
  I want the global token and key denylist to redact secrets
  So that a service that declares no redact patterns still cannot leak secrets

  Scenario: a secret is redacted by the global denylist alone
    Given a service with an empty per-service redact list and a global denylist matching API_KEY assignments
    When the service prints a line containing an API_KEY value
    Then the stored event-log entry and the emitted event are redacted
    And the raw secret never reaches the model context
