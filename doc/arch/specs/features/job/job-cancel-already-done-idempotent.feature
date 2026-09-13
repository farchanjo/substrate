Feature: job.cancel is idempotent for terminal and in-flight jobs
  As an LLM agent driving substrate
  I want job.cancel to be safe to call multiple times without errors
  So that retry logic and race conditions in the client do not produce unexpected failures

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: job.cancel on a succeeded job returns state=already_done without error
    Given the client has submitted an archive.tar.create job that has completed with state=succeeded
    When the client calls job.cancel with that job_id
    Then the response does not contain an error object
    And the response contains field "state" equal to "already_done"

  Scenario: job.cancel called twice on a running job: first cancels, second returns already_done
    Given the client has submitted an archive.tar.create job that is currently running
    When the client calls job.cancel for that job_id the first time
    Then the job transitions to state cancelled
    When the client calls job.cancel for the same job_id a second time
    Then the response does not contain an error object
    And the response contains field "state" equal to "already_done"
