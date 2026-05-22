Feature: fs.find returns paginated matches under an allowlist root
  As an LLM agent driving substrate
  I want to locate files by glob inside an allowlist root
  So that I can plan subsequent reads without exhausting context

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo" contains 120 files matching "*.rs"

  Scenario: Default pagination returns first 50 matches with a cursor
    When the client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the structured content has exactly 50 entries
    And the structured content includes a next_cursor token
    And the content text reports "120 matches under /work/repo. Showing 50."

  Scenario: Following the cursor returns next page without overlap
    Given the prior fs.find call returned cursor "cur_page1"
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and cursor="cur_page1"
    Then the structured content has exactly 50 entries
    And the entries do not overlap with the first page

  Scenario: Last page has no next_cursor
    Given the prior fs.find calls have consumed 100 entries via cursor "cur_page2"
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and cursor="cur_page2"
    Then the structured content has exactly 20 entries
    And the structured content does not include a next_cursor token

  Scenario Outline: Explicit page_size is honored within bounds
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and page_size=<size>
    Then the structured content has exactly <expected> entries

    Examples:
      | size | expected |
      | 10   | 10       |
      | 50   | 50       |
      | 500  | 120      |
