Feature: idempotency_key attaches retries to an existing job
  As an LLM agent driving substrate
  I want to submit the same job with an idempotency_key and receive the existing job_id on retry
  So that network retries do not spawn duplicate workers for the same logical operation

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the client has a stable client_id "client-A"

  Scenario: Submit with idempotency_key returns the same job_id on retry with identical arguments
    Given the client submits archive.tar.create with src="/work/src" and idempotency_key="01JABCDEFGHJKMNPQRSTVWXYZ"
    And the server returns job_id="01JABCDEFGHJKMNPQRSTVWXYZ-JOB"
    When the client submits archive.tar.create again with the same src="/work/src" and idempotency_key="01JABCDEFGHJKMNPQRSTVWXYZ"
    Then the server returns the same job_id without spawning a new worker task
    And the total number of active workers for that operation remains 1

  Scenario: Submit with same idempotency_key but different arguments results in a new job
    Given the client submits archive.tar.create with src="/work/src" and idempotency_key="01JABCDEFGHJKMNPQRSTVWXYZ"
    And the server returns job_id="01JABCDEFGHJKMNPQRSTVWXYZ-JOB"
    When the client submits archive.tar.create with src="/work/other" and the same idempotency_key="01JABCDEFGHJKMNPQRSTVWXYZ"
    Then the server treats the args_hash as different and creates a new job with a distinct job_id
    And both jobs are visible independently via job.status
