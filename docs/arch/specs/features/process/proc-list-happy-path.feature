Feature: proc.list returns paginated process snapshots
  As an LLM agent driving substrate
  I want to enumerate running processes with resource metrics
  So that I can identify resource usage and parent-child relationships

  Background:
    Given the host has more than 100 running processes

  Scenario: Default page returns up to 50 process snapshots
    When the client calls proc.list
    Then the structured content has exactly 50 process entries
    And each entry contains fields: pid, name, cpu_percent, mem_percent, parent_pid
    And the structured content includes a next_cursor token

  Scenario: Each process entry contains required fields
    When the client calls proc.list
    Then every entry has a non-null pid field of integer type
    And every entry has a non-empty name field of string type
    And every entry has a cpu_percent field of float type between 0 and 100
    And every entry has a mem_percent field of float type between 0 and 100
    And every entry has a parent_pid field which is null for root processes

  Scenario: Cursor pagination returns next batch without duplicates
    Given the first proc.list call returned cursor "proc_cur_1"
    When the client calls proc.list with cursor="proc_cur_1"
    Then the returned PIDs do not overlap with the first page PIDs

  Scenario Outline: page_size parameter controls result count
    Given the host has at least <size> running processes
    When the client calls proc.list with page_size=<size>
    Then the structured content has exactly <size> process entries

    Examples:
      | size |
      | 10   |
      | 25   |
      | 50   |
