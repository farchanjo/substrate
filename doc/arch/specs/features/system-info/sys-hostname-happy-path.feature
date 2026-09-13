Feature: sys.hostname returns the system hostname as reported by the OS
  As an LLM agent driving substrate
  I want to retrieve the host's short hostname without invoking an external command
  So that I can label diagnostic output and reason about host identity

  Background:
    Given a running substrate server connected to the host OS

  Scenario: sys.hostname returns a non-empty hostname field
    When the client calls sys.hostname
    Then the structured content contains a hostname field of non-empty string type
    And no error is returned

  Scenario: sys.hostname content text is within token budget
    When the client calls sys.hostname
    Then the content text representation is at most 80 tokens

  Scenario: sys.hostname content text starts with the prefix sys.hostname
    When the client calls sys.hostname
    Then the content text starts with "sys.hostname:"

  Scenario: sys.hostname is idempotent across successive calls
    When the client calls sys.hostname twice in rapid succession
    Then both hostname values are identical
    And no error is returned on either call

  Scenario: sys.hostname platform parity — uses nix gethostname on all platforms
    Given the server is running on any supported platform
    When the client calls sys.hostname
    Then the hostname field is a non-empty string regardless of platform
    And the structured content does not contain an error code field
