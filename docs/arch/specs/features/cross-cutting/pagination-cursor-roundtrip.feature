Feature: Cursor-based pagination covers all entries without duplicates or gaps
  As an LLM agent driving substrate
  I want sequential cursor pages to yield every item exactly once
  So that I can consume large result sets reliably across multiple calls

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo" contains exactly 200 files matching "*.rs"

  Scenario: Four pages of 50 cover all 200 entries without duplicates or gaps
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and page_size=50
    Then the structured content has exactly 50 entries and includes next_cursor "cur_1"
    When the client calls fs.find with cursor="cur_1" and page_size=50
    Then the structured content has exactly 50 entries and includes next_cursor "cur_2"
    And the entries on page 2 do not overlap with page 1
    When the client calls fs.find with cursor="cur_2" and page_size=50
    Then the structured content has exactly 50 entries and includes next_cursor "cur_3"
    And the entries on page 3 do not overlap with pages 1 or 2
    When the client calls fs.find with cursor="cur_3" and page_size=50
    Then the structured content has exactly 50 entries and does not include a next_cursor
    And the entries on page 4 do not overlap with pages 1, 2, or 3
    And the union of all four pages equals the full set of 200 files

  Scenario: Cursor is opaque and must not be constructed by the client
    Given a valid cursor "cur_1" returned by a prior fs.find call
    When the client calls fs.find with a manually crafted cursor value "page=2"
    Then the tool returns error code SUBSTRATE_INVALID_ARGUMENT

  Scenario: Expired or invalid cursor returns SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.find with cursor="completely_invalid_cursor_xyz"
    Then the tool returns error code SUBSTRATE_INVALID_ARGUMENT
