Feature: text.tail returns the last N lines of a file
  As an LLM agent driving substrate
  I want to inspect the end of a file without loading it entirely
  So that I can read recent log entries and append-only output without full-file reads

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/data/app.log" contains 200 lines numbered "line 1" through "line 200"

  Scenario: text.tail returns the last 10 lines by default
    When the client calls text.tail with path="/work/repo/data/app.log"
    Then the structured content contains a lines field of array type
    And the lines array has exactly 10 entries
    And the structured content contains a line_count field equal to 10
    And no error is returned

  Scenario: text.tail with explicit n returns exactly those last lines
    When the client calls text.tail with path="/work/repo/data/app.log" and n=25
    Then the lines array has exactly 25 entries
    And the first entry equals "line 176"
    And the last entry equals "line 200"

  Scenario: text.tail on a file shorter than n returns all lines
    Given the file "/work/repo/data/short.txt" contains exactly 5 lines
    When the client calls text.tail with path="/work/repo/data/short.txt" and n=20
    Then the lines array has exactly 5 entries
    And no error is returned

  Scenario: text.tail returns SUBSTRATE_NOT_FOUND for a missing file
    Given no file exists at "/work/repo/missing.txt"
    When the client calls text.tail with path="/work/repo/missing.txt"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters

  Scenario: text.tail returns SUBSTRATE_PATH_OUTSIDE_JAIL for a path outside the allowlist
    When the client calls text.tail with path="/var/log/syslog"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_PATH_OUTSIDE_JAIL"

  Scenario: text.tail line_count field matches the length of the lines array
    When the client calls text.tail with path="/work/repo/data/app.log" and n=15
    Then the line_count field equals the length of the lines array
