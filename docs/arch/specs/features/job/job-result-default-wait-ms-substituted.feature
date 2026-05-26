# ADR-0059
Feature: job.result injects result_default_wait_ms when wait_ms is absent from the request
  As an LLM agent driving substrate
  I want job.result to long-poll for a sensible default duration when I omit wait_ms
  So that I receive the final result inline without having to specify a timeout on every call

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.quotas.result_default_wait_ms set to 5000
    And the server configuration has jobs.quotas.result_max_wait_ms set to 30000

  Scenario: Omitting wait_ms causes the dispatcher to substitute result_default_wait_ms and return inline on completion
    Given the client has submitted an archive.tar.create job expected to finish in under 2000 ms
    When the client calls job.result with the job_id and no wait_ms field in the request payload
    Then the dispatcher substitutes wait_ms with jobs.quotas.result_default_wait_ms before polling
    And the response contains the final ToolOutput before 5000 ms elapse
    And the response contains field "state" equal to "succeeded"

  Scenario: Explicit wait_ms=0 bypasses substitution and returns immediately with state=running
    Given the client has submitted an archive.tar.create job that is still running
    When the client calls job.result with the job_id and wait_ms=0 explicitly present in the request payload
    Then the dispatcher does not substitute wait_ms and returns without blocking
    And the response contains field "state" equal to "running"
    And the response arrives within 100 ms

  Scenario: Omitting wait_ms on a slow job returns SUBSTRATE_TIMEOUT after result_default_wait_ms elapses
    Given the client has submitted an archive.tar.create job expected to run longer than 10000 ms
    When the client calls job.result with the job_id and no wait_ms field in the request payload
    Then the response contains an error object within 5000 ms
    And the error object has field "code" equal to "SUBSTRATE_TIMEOUT"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" equal to the submitted job_id
    And the error elapsed time is less than jobs.quotas.result_max_wait_ms
