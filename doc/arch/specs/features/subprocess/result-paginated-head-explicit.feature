# ADR-0057 cross-ref: subprocess output pagination and search
Feature: subprocess.result returns paginated output in explicit Head order
  As an LLM agent using substrate
  I want to retrieve subprocess captured output page by page starting from the oldest lines
  So that I can replay captured output in chronological order

  Scenario: first page of four lines from ten-line stdout in explicit Head order
    Given a subprocess wrote exactly 10 numbered lines to stdout and has exited with Succeeded
    When subprocess.result is called with pagination offset 0 and page_size 4 and order Head
    Then the response stdout_lines contains exactly 4 entries
    And stdout_lines[0] is line 1 and stdout_lines[1] is line 2 and stdout_lines[2] is line 3 and stdout_lines[3] is line 4
    And stdout_total_lines equals 10
    And stdout_next_offset equals 4
