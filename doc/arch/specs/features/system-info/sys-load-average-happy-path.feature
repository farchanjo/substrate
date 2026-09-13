Feature: sys.load_average returns the 1-, 5-, and 15-minute load averages
  As an LLM agent driving substrate
  I want to read CPU load averages without parsing /proc/loadavg or calling sysctl
  So that I can assess host load before dispatching expensive filesystem or process tools

  Background:
    Given a running substrate server connected to the host OS

  Scenario: sys.load_average returns all three load fields
    When the client calls sys.load_average
    Then the structured content contains a "1m" field of float type
    And the structured content contains a "5m" field of float type
    And the structured content contains a "15m" field of float type
    And no error is returned

  Scenario: All load average values are non-negative
    When the client calls sys.load_average
    Then the "1m" value is a non-negative float
    And the "5m" value is a non-negative float
    And the "15m" value is a non-negative float

  Scenario: sys.load_average content text is within token budget
    When the client calls sys.load_average
    Then the content text representation is at most 80 tokens

  Scenario: sys.load_average is consistent with sys.info load_average sub-record
    When the client calls sys.load_average
    And the client calls sys.info
    Then the "1m" value from sys.load_average matches the load_average 1m value from sys.info
    And the "5m" value from sys.load_average matches the load_average 5m value from sys.info
    And the "15m" value from sys.load_average matches the load_average 15m value from sys.info

  Scenario: sys.load_average platform parity — Linux reads /proc/loadavg, macOS uses getloadavg
    Given the server is running on any supported platform
    When the client calls sys.load_average
    Then all three load average fields are present and non-negative regardless of platform
    And the structured content does not contain an error code field
