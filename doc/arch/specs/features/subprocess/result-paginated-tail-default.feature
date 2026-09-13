# ADR-0057 cross-ref: subprocess output pagination and search
Feature: subprocess.result returns paginated output in Tail order by default
  As an LLM agent using substrate
  I want to retrieve subprocess captured output page by page starting from the most-recent lines
  So that I can inspect the tail of long-running process output without loading the full aggregate

  Scenario: first page of three lines from ten-line stdout in default Tail order
    Given a subprocess wrote exactly 10 numbered lines to stdout and has exited with Succeeded
    When subprocess.result is called with pagination offset 0 and page_size 3 and order omitted
    Then the response stdout_lines contains exactly 3 entries
    And stdout_lines[0] is line 10 and stdout_lines[1] is line 9 and stdout_lines[2] is line 8
    And stdout_total_lines equals 10
    And stdout_next_offset equals 3

  Scenario: second page of three lines from ten-line stdout in default Tail order
    Given a subprocess wrote exactly 10 numbered lines to stdout and has exited with Succeeded
    When subprocess.result is called with pagination offset 3 and page_size 3 and order omitted
    Then the response stdout_lines contains exactly 3 entries
    And stdout_lines[0] is line 7 and stdout_lines[1] is line 6 and stdout_lines[2] is line 5
    And stdout_total_lines equals 10
    And stdout_next_offset equals 6
