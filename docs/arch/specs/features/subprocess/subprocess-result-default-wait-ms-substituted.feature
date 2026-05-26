# ADR-0059
Feature: subprocess.result injects result_default_wait_ms when wait_ms is absent from the request
  As an LLM agent driving substrate
  I want subprocess.result to long-poll for the configured default duration when I omit wait_ms
  So that a Bucket E subprocess that exits quickly is returned inline without requiring an explicit timeout

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.quotas.result_default_wait_ms set to 5000
    And the server configuration has jobs.quotas.result_max_wait_ms set to 30000
    And the binary "/usr/bin/env" is present in the subprocess allowlist

  Scenario: Omitting wait_ms on a subprocess that exits quickly causes inline return with state=succeeded
    Given the client has spawned a subprocess running "/usr/bin/env" expected to exit in under 500 ms
    When the client calls subprocess.result with the subprocess_id and no wait_ms field in the request payload
    Then the dispatcher substitutes wait_ms with jobs.quotas.result_default_wait_ms before polling
    And the response contains the final ToolOutput before 5000 ms elapse
    And the response contains field "state" equal to "succeeded"

  Scenario: Explicit wait_ms=0 bypasses substitution and returns immediately with state=running
    Given the client has spawned a subprocess that is still running
    When the client calls subprocess.result with the subprocess_id and wait_ms=0 explicitly present in the request payload
    Then the dispatcher does not substitute wait_ms and returns without blocking
    And the response contains field "state" equal to "running"
    And the response arrives within 100 ms
