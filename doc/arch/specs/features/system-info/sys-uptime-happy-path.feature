Feature: sys.uptime returns system uptime in seconds and as a human-readable string
  As an LLM agent driving substrate
  I want to retrieve the system uptime without parsing /proc/uptime or sysctl output
  So that I can report how long the host has been running in a single call

  Background:
    Given a running substrate server connected to the host OS
    And the host has been running for at least 60 seconds

  Scenario: sys.uptime returns both seconds and human fields
    When the client calls sys.uptime
    Then the structured content contains a seconds field of positive integer type
    And the structured content contains a human field of non-empty string type
    And no error is returned

  Scenario: seconds value is consistent with human-readable string
    When the client calls sys.uptime
    Then the seconds value is greater than or equal to 60
    And the human field is a non-empty string that reflects the seconds value

  Scenario: sys.uptime content text is within token budget
    When the client calls sys.uptime
    Then the content text representation is at most 80 tokens

  Scenario: sys.uptime is idempotent across successive calls
    When the client calls sys.uptime twice in rapid succession
    Then both seconds values are non-decreasing
    And no error is returned on either call

  Scenario: sys.uptime platform parity — Linux reads /proc/uptime, macOS uses sysctl KERN_BOOTTIME
    Given the server is running on any supported platform
    When the client calls sys.uptime
    Then the seconds field is a positive integer regardless of platform
    And the structured content does not contain an error code field
