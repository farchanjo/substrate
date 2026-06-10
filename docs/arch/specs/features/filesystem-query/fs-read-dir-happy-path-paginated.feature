Feature: fs.read_dir returns paginated directory entries
  As an LLM agent driving substrate
  I want to list the direct children of a directory with cursor-based pagination
  So that I can navigate large directories without exhausting context

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo/src" contains 80 immediate children

  Scenario: Default pagination returns first 50 entries with a cursor
    When the client calls fs.read_dir with path="/work/repo/src"
    Then the structured content has exactly 50 entries
    And the structured content includes a next_cursor token
    And the content text reports "80 entries in /work/repo/src. Showing 50."

  Scenario: Following the cursor returns the remaining entries without overlap
    Given the prior fs.read_dir call returned cursor "cur_dir_page1"
    When the client calls fs.read_dir with path="/work/repo/src" and cursor="cur_dir_page1"
    Then the structured content has exactly 30 entries
    And the entries do not overlap with the first page
    And the structured content does not include a next_cursor token

  Scenario: Last page has no next_cursor
    Given the prior fs.read_dir calls have consumed 60 entries via cursor "cur_dir_page2"
    When the client calls fs.read_dir with path="/work/repo/src" and cursor="cur_dir_page2"
    Then the structured content has exactly 20 entries
    And the structured content does not include a next_cursor token

  Scenario Outline: Explicit page_size is honored within bounds
    When the client calls fs.read_dir with path="/work/repo/src" and page_size=<size>
    Then the structured content has exactly <expected> entries

    Examples:
      | size | expected |
      | 10   | 10       |
      | 50   | 50       |
      | 500  | 80       |

  Scenario: Path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.read_dir with path="/tmp/outside"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no filesystem read is performed

  Scenario: Path pointing to a regular file returns SUBSTRATE_NOT_A_DIRECTORY
    Given the path "/work/repo/src/main.rs" is a regular file
    When the client calls fs.read_dir with path="/work/repo/src/main.rs"
    Then the tool returns error code SUBSTRATE_NOT_A_DIRECTORY
