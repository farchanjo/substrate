Feature: Client notifications/cancelled for an in-flight tool call is mapped to job.cancel
  As an LLM agent driving substrate
  I want MCP protocol cancellation notifications to propagate to the async job system
  So that in-flight worker tasks are cancelled promptly without a separate code path

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the client has submitted an archive.tar.create job with progressToken equal to the job_id
    And the job is currently in state running

  Scenario: notifications/cancelled with progressToken matching job_id cancels the worker task
    When the client sends a notifications/cancelled message with progressToken equal to the active job_id
    Then the server maps the notification to job.cancel for that job_id
    And the job CancellationToken is signalled within 100 ms
    And a subsequent call to job.status for that job_id returns state="cancelled" within 1000 ms

  Scenario: CancellationToken biased select acknowledges cancellation within 1 second
    When the client sends a notifications/cancelled message for the active job_id
    Then the server emits a notifications/progress event with job_state="cancelled" within 1000 ms
    And the emitted event contains the same job_id as the cancellation notification

  Scenario: Transactional tmp files are cleaned up before the terminal state is recorded
    Given the archive.tar.create worker has created one or more .tmp.<uuid7> files under the destination path
    When the client sends a notifications/cancelled message for the active job_id
    Then all .tmp.<uuid7> files under the destination path are removed before the job state is recorded as cancelled
    And a subsequent call to job.status returns state="cancelled"
    And no .tmp.<uuid7> files remain under the destination path
