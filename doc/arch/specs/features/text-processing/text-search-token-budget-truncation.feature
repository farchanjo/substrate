Feature: text.search truncates ranked results to a token budget, not just a match count
  As an LLM agent with a bounded context window
  I want text.search to stop returning matches once a token budget is exhausted
  So that a single call cannot flood my context regardless of how many files match

  Background:
    Given a running substrate server with the fs-index and fs-index-content features enabled
    And an allowlist with root "/work/repo"
    And the content index has been built for "/work/repo"
    And the directory "/work/repo" contains 200 files each matching the pattern "TODO"

  Scenario: A small token budget truncates the response before page_size is reached
    When the client calls text.search with root="/work/repo", pattern="TODO", and token_budget=200
    Then the structured content has fewer than 50 match entries
    And the cumulative estimated token count of the returned matches does not exceed 200
    And the structured content has truncated_by_budget=true

  Scenario: A generous token budget is not the limiting factor and page_size applies instead
    When the client calls text.search with root="/work/repo", pattern="TODO", and token_budget=1000000
    Then the structured content has exactly 50 match entries
    And the structured content has truncated_by_budget=false
    And the structured content includes a next_cursor token

  Scenario: Omitting token_budget applies the configured default
    Given index.token_budget_default is configured to 2000
    When the client calls text.search with root="/work/repo" and pattern="TODO" and no token_budget is supplied
    Then the cumulative estimated token count of the returned matches does not exceed 2000

  Scenario: Truncation prioritizes the highest-scoring matches, not file-scan order
    Given the file "/work/repo/src/best.rs" has the single highest BM25 score for pattern "TODO"
    When the client calls text.search with root="/work/repo", pattern="TODO", and a token_budget too small for all matches
    Then the returned matches include an entry from "/work/repo/src/best.rs"
