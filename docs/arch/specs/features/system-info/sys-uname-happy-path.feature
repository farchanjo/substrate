Feature: sys.uname returns kernel name, release, version, and machine architecture
  As an LLM agent driving substrate
  I want to retrieve kernel identification without parsing uname -a output
  So that I can branch on OS and architecture in a structured way

  Background:
    Given a running substrate server connected to the host OS

  Scenario: sys.uname returns all required fields
    When the client calls sys.uname
    Then the structured content contains a sysname field of non-empty string type
    And the structured content contains a nodename field of non-empty string type
    And the structured content contains a release field of non-empty string type
    And the structured content contains a version field of non-empty string type
    And the structured content contains a machine field of non-empty string type
    And no error is returned

  Scenario: sysname is a known OS identifier
    When the client calls sys.uname
    Then the sysname field is one of "Linux" or "Darwin"

  Scenario: sys.uname content text is within token budget
    When the client calls sys.uname
    Then the content text representation is at most 80 tokens

  Scenario: sys.uname is consistent with sys.hostname nodename
    When the client calls sys.uname
    And the client calls sys.hostname
    Then the nodename field from sys.uname matches the hostname field from sys.hostname

  Scenario: sys.uname platform parity — uses nix::sys::utsname::uname on all platforms
    Given the server is running on any supported platform
    When the client calls sys.uname
    Then all five fields are non-empty strings regardless of platform
    And the structured content does not contain an error code field
