Feature: Operations lasting >= 1 second emit ProgressNotification with progressToken
  As an LLM agent driving substrate
  I want progress events for long-running operations
  So that I can display feedback and detect stalled operations

  Background:
    Given a running substrate server with MCP progress notifications enabled

  Scenario: fs.find over a large tree emitting >= 1s emits ProgressNotification
    Given the directory "/work/repo" contains enough files that fs.find takes >= 1 second
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" including a progressToken
    Then at least one ProgressNotification is received before the final result
    And each ProgressNotification includes the progressToken from the request
    And each ProgressNotification includes a progress value between 0 and 1 (inclusive)

  Scenario: archive.tar.create emits progress notifications
    Given archiving "/work/repo/src" will take >= 1 second
    When the client calls archive.tar.create with src="/work/repo/src" and progressToken="tok-42"
    Then at least one ProgressNotification with progressToken="tok-42" is emitted
    And the final ProgressNotification has progress=1.0 or total=current

  Scenario: ProgressNotification is not emitted for sub-second operations
    Given a directory "/work/repo/tiny" containing 3 files
    When the client calls fs.find with root="/work/repo/tiny" and pattern="*" and progressToken="tok-fast"
    Then no ProgressNotification is emitted before the result
    And the result arrives without intermediate notifications

  Scenario: Each ProgressNotification has a monotonically increasing progress value
    Given an operation that emits multiple ProgressNotifications
    When all ProgressNotifications for progressToken="tok-seq" are collected
    Then the progress values in emission order are non-decreasing
