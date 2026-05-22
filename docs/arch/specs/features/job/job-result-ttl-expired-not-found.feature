Feature: jobs.result_ttl_secs GC enforces TTL eviction of completed jobs
  As an LLM agent driving substrate
  I want expired job entries to be evicted from the registry
  So that the server does not accumulate unbounded in-memory state between restarts

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.result_ttl_secs set to 5
    And the server configuration has jobs.gc_interval_secs set to 1

  Scenario: job.result after the TTL window returns SUBSTRATE_JOB_NOT_FOUND
    Given the client has submitted an archive.tar.create job that has completed successfully
    And at least 6 seconds have elapsed since the job entered the terminal state
    When the client calls job.result with that job_id
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_JOB_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 Crockford pattern

  Scenario: Audit event records GC eviction of expired job
    Given the client has submitted an archive.tar.create job that has completed successfully
    And at least 6 seconds have elapsed since the job entered the terminal state
    When the background GC task wakes and evicts the expired job entry
    Then an audit event is emitted with tool_name matching "gc_evict" or job tool_name
    And the audit event has field "correlation_id" equal to the evicted job_id
    And the audit event has field "job_state" equal to the terminal state at eviction time
