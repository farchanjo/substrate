Feature: Push and pull channels resolve races via the job state machine
  As an LLM agent driving substrate
  I want concurrent push notifications and pull result calls to converge on the same terminal state
  So that the client never observes inconsistent views of a completed job

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the client has submitted an archive.tar.create job with a progressToken equal to the job_id

  Scenario: Concurrent subscriber and long-poll caller both reflect the same terminal state
    Given the client is subscribed to notifications/progress for the job_id
    And the client has called job.result with the job_id and wait_ms=30000 concurrently
    When the archive.tar.create job completes successfully
    Then the final notifications/progress event contains job_state="succeeded"
    And the job.result response contains field "state" equal to "succeeded"
    And both reflect the same job_id and sequence_number for the terminal event

  Scenario: Out-of-order progress event arriving after terminal state is silently dropped
    Given the archive.tar.create job has transitioned to state succeeded
    When a stale notifications/progress event with job_state="running" arrives at the client after the terminal notification
    Then the client does not emit an error or produce an inconsistent state
    And the last observed state on the client remains "succeeded"

  Scenario: sequence_number is strictly monotonic across all progress events of a single job
    Given the client has subscribed to notifications/progress for an active job_id
    When the job emits multiple notifications/progress events during its run
    Then the sequence_number of each successive event is strictly greater than the sequence_number of the previous event
    And no two events for the same job_id share the same sequence_number
