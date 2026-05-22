Feature: text.search silently skips binary files
  As an LLM agent driving substrate
  I want binary files to be excluded from text search results
  So that grep-searcher detection prevents garbled output and false matches

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/assets/logo.png" is a binary PNG file
    And the file "/work/repo/target/debug/substrate" is a binary ELF executable
    And the file "/work/repo/src/main.rs" is a UTF-8 text file containing "substrate"

  Scenario: Binary files are not included in search results
    When the client calls text.search with root="/work/repo" and pattern="substrate"
    Then the match entries do not include file_path="/work/repo/assets/logo.png"
    And the match entries do not include file_path="/work/repo/target/debug/substrate"

  Scenario: Text files matching the pattern are still returned
    When the client calls text.search with root="/work/repo" and pattern="substrate"
    Then at least one match entry has file_path="/work/repo/src/main.rs"

  Scenario: Skipped binary files are reported in the metadata
    When the client calls text.search with root="/work/repo" and pattern="substrate"
    Then the structured content metadata includes a skipped_binary_count field with value >= 2

  Scenario: Search across a directory with only binary files returns zero matches
    Given the directory "/work/repo/bin_only/" contains only binary files
    When the client calls text.search with root="/work/repo/bin_only" and pattern="."
    Then the structured content has exactly 0 match entries
    And no error is returned
    And the skipped_binary_count metadata value equals the number of files in "/work/repo/bin_only/"
