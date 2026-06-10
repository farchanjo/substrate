Feature: sys.df returns mounted filesystem statistics in structured form
  As an LLM agent driving substrate
  I want to enumerate mounted filesystems with capacity and usage metrics
  So that I can reason about disk availability without invoking the df command

  Background:
    Given a running substrate server connected to the host OS
    And the host has at least one mounted filesystem

  Scenario: sys.df returns a mounts array with at least one entry
    When the client calls sys.df
    Then the structured content contains a mounts field of array type
    And the mounts array has at least one entry
    And no error is returned

  Scenario: Each mount entry contains required fields
    When the client calls sys.df
    Then every mounts entry has a device field of non-empty string type
    And every mounts entry has a mount field of non-empty string type
    And every mounts entry has a fstype field of non-empty string type
    And every mounts entry has a total_bytes field of non-negative integer type
    And every mounts entry has a used_bytes field of non-negative integer type
    And every mounts entry has an available_bytes field of non-negative integer type
    And every mounts entry has a use_pct field of float type between 0.0 and 100.0

  Scenario: used_bytes does not exceed total_bytes for any mount
    When the client calls sys.df
    Then for every mounts entry the used_bytes value is less than or equal to total_bytes

  Scenario: sys.df content text is within token budget
    When the client calls sys.df
    Then the content text representation is at most 80 tokens

  Scenario: sys.df platform parity — Linux uses statvfs, macOS uses getmntinfo
    Given the server is running on any supported platform
    When the client calls sys.df
    Then the mounts array is non-empty regardless of platform
    And the structured content does not contain an error code field
