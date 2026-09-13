Feature: sys.info returns host system snapshot in a compact response
  As an LLM agent driving substrate
  I want to retrieve basic host system information in a single call
  So that I can reason about the environment without multiple round-trips

  Background:
    Given a running substrate server connected to the host OS

  Scenario: sys.info returns all required fields
    When the client calls sys.info
    Then the structured content contains a hostname field of non-empty string type
    And the structured content contains a kernel field of non-empty string type
    And the structured content contains an uptime_seconds field of positive integer type
    And the structured content contains a load_average field with entries for 1m, 5m, and 15m
    And the structured content contains a mem field with total_bytes, used_bytes, and free_bytes
    And no error is returned

  Scenario: sys.info content text is within token budget
    When the client calls sys.info
    Then the content text representation is at most 80 tokens

  Scenario: uptime_seconds reflects actual host uptime
    Given the host has been running for at least 60 seconds
    When the client calls sys.info
    Then the uptime_seconds value is greater than or equal to 60

  Scenario: load_average values are non-negative floats
    When the client calls sys.info
    Then the load_average 1m value is a non-negative float
    And the load_average 5m value is a non-negative float
    And the load_average 15m value is a non-negative float

  Scenario: mem used_bytes plus free_bytes does not exceed total_bytes
    When the client calls sys.info
    Then the sum of mem.used_bytes and mem.free_bytes is less than or equal to mem.total_bytes
