Feature: text.head returns the first N lines of a file
  As an LLM agent driving substrate
  I want to inspect the beginning of a file without loading it entirely
  So that I can detect its format and decide whether further processing is needed

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/data/log.txt" contains 200 lines numbered "line 1" through "line 200"

  Scenario: text.head returns the first 10 lines by default
    When the client calls text.head with path="/work/repo/data/log.txt"
    Then the structured content contains a lines field of array type
    And the lines array has exactly 10 entries
    And the structured content contains a line_count field equal to 10
    And no error is returned

  Scenario: text.head with explicit n returns exactly that many lines
    When the client calls text.head with path="/work/repo/data/log.txt" and n=25
    Then the lines array has exactly 25 entries
    And the first entry equals "line 1"
    And the last entry equals "line 25"

  Scenario: text.head on a file shorter than n returns all lines
    Given the file "/work/repo/data/short.txt" contains exactly 5 lines
    When the client calls text.head with path="/work/repo/data/short.txt" and n=20
    Then the lines array has exactly 5 entries
    And no error is returned

  Scenario: text.head returns SUBSTRATE_NOT_FOUND for a missing file
    Given no file exists at "/work/repo/missing.txt"
    When the client calls text.head with path="/work/repo/missing.txt"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters

  Scenario: text.head returns SUBSTRATE_PATH_OUTSIDE_JAIL for a path outside the allowlist
    When the client calls text.head with path="/etc/shadow"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_PATH_OUTSIDE_JAIL"

  Scenario: text.head line_count field matches the length of the lines array
    When the client calls text.head with path="/work/repo/data/log.txt" and n=15
    Then the line_count field equals the length of the lines array
