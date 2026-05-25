# ADR-0057 cross-ref: subprocess output pagination and search
Feature: subprocess.search returns paginated match results
  As an LLM agent using substrate
  I want to retrieve search matches page by page
  So that I can handle large match sets without exhausting response payload limits

  Scenario: first page of five matches from ten-match result set in Tail order
    Given a subprocess wrote 20 stdout lines of which exactly 10 contain the text "ERROR" and has exited with Succeeded
    When subprocess.search is called with pattern "ERROR" and pagination offset 0 and page_size 5 and order Tail
    Then the response total_matches equals 10
    And matches contains exactly 5 entries
    And the entries are the 5 most-recent matching lines in newest-first order
    And next_offset equals 5
