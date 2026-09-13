# ADR-0054 cross-ref: subprocess stdout/stderr stream multiplex via notifications/progress
# ADR-0054 §"Tokio Task Architecture" — bounded mpsc(64) channel; excess chunks are dropped
#   and counted in stream_chunks_dropped on SubprocessHandle.
# ADR-0007 cross-ref: tool card narrative arc — structuredContent hints map carries
#   error_recovery hint when stream_chunks_dropped > 0.
Feature: stream_chunks_dropped is reported when backpressure drops chunks
  As an LLM agent using substrate
  I want subprocess.result to report dropped chunk counts and a recovery hint
  So that I can detect incomplete output and throttle my consumer accordingly

  Scenario: subprocess.result reports stream_chunks_dropped and backpressure hint
    Given subprocess.spawn is invoked with capture_kind "stream"
    And the child binary writes more than 100 chunks in rapid succession
    And the MCP client consumer is slow enough to fill the bounded mpsc channel of size 64
    When the child exits and subprocess.result is called
    Then the structuredContent payload contains stream_chunks_dropped greater than 0
    And the hints map in structuredContent carries an error_recovery entry
    And that error_recovery entry mentions "backpressure" or "throttle"
