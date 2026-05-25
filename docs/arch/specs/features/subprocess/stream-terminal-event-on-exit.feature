# ADR-0054 cross-ref: subprocess stdout/stderr stream multiplex via notifications/progress
# ADR-0054 §"Dispatcher Task" — terminal sentinel event is the final notifications/progress
# event emitted by the dispatcher task after all stdout/stderr chunks have been flushed.
# progress_token MUST equal job_id per ADR-0040 triple-equality invariant.
Feature: terminal sentinel event is emitted exactly once on subprocess exit
  As an LLM agent using substrate
  I want exactly one terminal notifications/progress event emitted after the child exits
  So that I can know the final state and safely call subprocess.result

  Background:
    Given subprocess.spawn is invoked with capture_kind "stream"
    And the progress_token in every emitted event equals the job_id

  Scenario: terminal sentinel event carries job_state Succeeded
    Given the child exits with status 0
    When all notifications/progress events for the job have been received
    Then exactly one event has job_state "Succeeded"
    And that event has chunk_base64 ""
    And that event has chunk_bytes 0
    And that event has no stream field
    And that event has no chunk_seq field
    And that event has no byte_offset field
    And that terminal event is delivered after all stdout and stderr chunk events
    And that terminal event is delivered before subprocess.result returns

  Scenario: terminal sentinel event carries job_state Failed
    Given the child exits with status 1
    When all notifications/progress events for the job have been received
    Then exactly one event has job_state "Failed"
    And that event has chunk_base64 ""
    And that event has chunk_bytes 0
    And that event has no stream field
    And that event has no chunk_seq field
    And that event has no byte_offset field
    And that terminal event is delivered after all stdout and stderr chunk events
    And that terminal event is delivered before subprocess.result returns

  Scenario: terminal sentinel event carries job_state TimedOut
    Given the child has not exited within timeout_secs
    When the dispatcher task triggers the signal cascade
    When all notifications/progress events for the job have been received
    Then exactly one event has job_state "TimedOut"
    And that event has chunk_base64 ""
    And that event has chunk_bytes 0
    And that event has no stream field
    And that event has no chunk_seq field
    And that event has no byte_offset field
    And that terminal event is delivered after all stdout and stderr chunk events
    And that terminal event is delivered before subprocess.result returns

  Scenario: terminal sentinel event carries job_state Cancelled
    Given job.cancel is called while the child is Running
    When the dispatcher task processes the cancellation token
    When all notifications/progress events for the job have been received
    Then exactly one event has job_state "Cancelled"
    And that event has chunk_base64 ""
    And that event has chunk_bytes 0
    And that event has no stream field
    And that event has no chunk_seq field
    And that event has no byte_offset field
    And that terminal event is delivered after all stdout and stderr chunk events
    And that terminal event is delivered before subprocess.result returns
