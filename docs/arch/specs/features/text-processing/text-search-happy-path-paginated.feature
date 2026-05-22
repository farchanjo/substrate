Feature: text.search returns paginated matches with cursor continuations
  As an LLM agent driving substrate
  I want to search file content by pattern across a directory tree
  So that I can locate relevant code spans without loading entire files

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo" contains text files with 120 lines matching "TODO"

  Scenario: Default search returns first 50 matches with a cursor
    When the client calls text.search with root="/work/repo" and pattern="TODO"
    Then the structured content has exactly 50 match entries
    And each entry contains fields: file_path, line_number, line_text
    And the structured content includes a next_cursor token

  Scenario: Following the cursor returns the next batch of matches
    Given a prior text.search call returned cursor "txt_cur_1"
    When the client calls text.search with root="/work/repo" and pattern="TODO" and cursor="txt_cur_1"
    Then the structured content has exactly 50 match entries
    And the (file_path, line_number) pairs do not overlap with the first page

  Scenario: Last page has no cursor and correct entry count
    Given prior calls have consumed 100 matches via cursor "txt_cur_2"
    When the client calls text.search with root="/work/repo" and pattern="TODO" and cursor="txt_cur_2"
    Then the structured content has exactly 20 match entries
    And the structured content does not include a next_cursor token

  Scenario: Regex pattern matches the correct lines
    Given the file "/work/repo/src/lib.rs" contains "// TODO(urgent): fix me" on line 42
    When the client calls text.search with root="/work/repo" and pattern="TODO\(urgent\)"
    Then at least one match entry has file_path="/work/repo/src/lib.rs" and line_number=42

  Scenario: Case-insensitive flag matches uppercase and lowercase
    Given the file "/work/repo/README.md" contains "todo: update docs" on line 5
    When the client calls text.search with root="/work/repo" and pattern="todo" and case_insensitive=true
    Then a match entry with file_path="/work/repo/README.md" and line_number=5 is returned
