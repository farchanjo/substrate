Feature: job.result long-poll honors wait_ms and respects the configured maximum
  As an LLM agent driving substrate
  I want job.result to block until the job completes or the timeout elapses
  So that I can retrieve results efficiently without polling in a tight loop

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.result_max_wait_ms set to 30000

  Scenario: job.result with wait_ms=5000 returns final result if job completes before 5000 ms
    Given the client has submitted an archive.tar.create job expected to finish in under 2000 ms
    When the client calls job.result with the job_id and wait_ms=5000
    Then the response contains the final ToolOutput before 5000 ms elapse
    And the response contains field "state" equal to "succeeded"

  Scenario: job.result with wait_ms=5000 on a running job returns SUBSTRATE_TIMEOUT within 5000 ms
    Given the client has submitted an archive.tar.create job expected to run longer than 5000 ms
    When the client calls job.result with the job_id and wait_ms=5000
    Then the response contains an error object within 5000 ms
    And the error object has field "code" equal to "SUBSTRATE_TIMEOUT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" equal to the submitted job_id

  Scenario: job.result with wait_ms=60000 is capped at result_max_wait_ms and returns SUBSTRATE_RESULT_WAIT_EXCEEDED
    Given the client has submitted a long-running archive.tar.create job
    When the client calls job.result with the job_id and wait_ms=60000
    Then the server caps the actual wait at 30000 ms
    And the response contains an error object after the cap elapses
    And the error object has field "code" equal to "SUBSTRATE_RESULT_WAIT_EXCEEDED"
    And the error object has field "recovery_hint" mentioning "result_max_wait_ms" or "cap"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
