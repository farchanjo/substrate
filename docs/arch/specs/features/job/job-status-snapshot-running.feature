Feature: job.status returns running snapshot without blocking writers
  As an LLM agent driving substrate
  I want to poll job.status during an active job and after completion
  So that I can observe progress and detect terminal states without interfering with the worker

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the client has submitted an archive.tar.create job that is running

  Scenario: job.status during an active run returns state=running with progress_pct
    Given the archive.tar.create job has been running for at least 100 ms
    When the client calls job.status with the active job_id
    Then the response contains field "state" equal to "running"
    And the response contains field "progress_pct" with an integer value between 0 and 100
    And the response contains field "elapsed_ms" with a positive integer value
    And the response contains field "sequence_number" with an integer value greater than or equal to 0

  Scenario: job.status after job reaches a terminal state returns the terminal state
    Given the archive.tar.create job has completed successfully
    When the client calls job.status with that job_id
    Then the response contains field "state" equal to "succeeded"
    And the response contains field "progress_pct" equal to 100

  Scenario: job.status for an unknown job_id returns SUBSTRATE_JOB_NOT_FOUND
    When the client calls job.status with job_id="01JAAAAAAAAAAAAAAAAAAAAAA"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_JOB_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 Crockford pattern
