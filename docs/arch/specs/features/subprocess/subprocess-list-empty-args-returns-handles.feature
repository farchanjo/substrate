# ADR-0052 cross-ref: subprocess bounded context — subprocess.list tool
# Bug: Default::default() on SubprocessListRequest produced page_size=0 because
# #[serde(default = "default_page_size")] is consumed only by serde, not by the
# auto-derived Default impl.  Fix: manual Default impl calling default_page_size().
Feature: subprocess.list with empty args returns live handles
  As an LLM agent using substrate
  I want subprocess.list to return registered subprocess handles when called with no arguments
  So that I can enumerate live subprocesses without having to specify every optional field

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And at least one subprocess is currently active

  Scenario: subprocess.list with absent arguments field returns the live handle
    When the client calls subprocess.list with no arguments (omitting the arguments field entirely)
    Then the response contains at least 1 handle in the handles array
    And the response does not contain an error

  Scenario: subprocess.list with an empty JSON object returns the live handle
    When the client calls subprocess.list with an empty JSON object as arguments
    Then the response contains at least 1 handle in the handles array
    And the response does not contain an error

  Scenario: subprocess.list with explicit page_size=500 returns the live handle
    When the client calls subprocess.list with arguments page_size=500
    Then the response contains at least 1 handle in the handles array
    And the response does not contain an error

  Scenario: subprocess.list with explicit page_size=0 returns INVALID_ARGUMENT (ADR-0060)
    When the client calls subprocess.list with arguments page_size=0
    Then the response contains an error with code "SUBSTRATE_INVALID_ARGUMENT"
    And the error references the field "page_size"
