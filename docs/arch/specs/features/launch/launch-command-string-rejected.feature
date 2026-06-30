# ADR-0064 cross-ref: command must be a TOML array, never a string (argument-injection surface)
Feature: a Service command declared as a string is rejected at parse time
  As an operator authoring a Profile
  I want the command field to require an array form
  So that no shell-style argument injection surface is introduced

  Scenario: a string-form command fails Profile parsing
    Given a trusted Profile whose service declares command as a single string
    When the Profile is parsed
    Then parsing fails with a validation error
    And no Stack is started
