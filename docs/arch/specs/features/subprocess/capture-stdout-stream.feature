# ADR-0054 cross-ref: subprocess stdout/stderr stream multiplex via notifications/progress
# ADR-0052 cross-ref: subprocess bounded context — capture_kind "stream" behavior
Feature: stdout chunks stream via notifications/progress
  As an LLM agent using substrate
  I want subprocess stdout chunks delivered as notifications/progress events
  So that I can process output incrementally without waiting for the process to exit

  Scenario: stdout chunks stream via notifications/progress
    Given subprocess.spawn is invoked with capture_kind "stream"
    When the child writes 8192 bytes to stdout
    Then at least 2 notifications/progress events are emitted with stream "stdout"
    And each event payload contains seq and chunk_base64 and byte_offset
    And the seq values are monotonic without gaps
