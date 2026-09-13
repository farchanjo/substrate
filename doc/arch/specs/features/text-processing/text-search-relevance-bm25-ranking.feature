Feature: text.search ranks matches by BM25 relevance when the content index is active
  As an LLM agent driving substrate
  I want text.search to return the most relevant matches first, not merely the first-found
  So that I can act on the strongest signal without re-ranking results in my own context

  Background:
    Given a running substrate server with the fs-index and fs-index-content features enabled
    And an allowlist with root "/work/repo"
    And the content index has been built for "/work/repo"

  Scenario: A document with higher term frequency and shorter length ranks above a weaker match
    Given the file "/work/repo/src/hot.rs" contains the term "cache" 8 times across 40 lines
    And the file "/work/repo/src/cold.rs" contains the term "cache" 1 time across 400 lines
    When the client calls text.search with root="/work/repo" and pattern="cache"
    Then the structured content has relevance_ranked=true
    And "/work/repo/src/hot.rs" is ranked above "/work/repo/src/cold.rs"
    And each match entry includes a score field

  Scenario: A document matching more of the query's terms ranks above a partial match
    Given the file "/work/repo/src/full.rs" contains both "retry" and "backoff"
    And the file "/work/repo/src/partial.rs" contains only "retry"
    When the client calls text.search with root="/work/repo" and pattern="retry backoff"
    Then "/work/repo/src/full.rs" is ranked above "/work/repo/src/partial.rs"

  Scenario: The content index is disabled and the response falls back to unranked order
    Given a running substrate server with the fs-index-content feature compiled out
    When the client calls text.search with root="/work/repo" and pattern="cache"
    Then the structured content has relevance_ranked=false
    And matches are returned in file-scan order, not score order

  Scenario: A root that has not completed its first content rebuild falls back to unranked order
    Given the content index rebuild for "/work/repo" has not yet completed
    When the client calls text.search with root="/work/repo" and pattern="cache"
    Then the structured content has relevance_ranked=false
    And the response includes results drawn from the unranked line-scan fallback path
