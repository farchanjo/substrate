Feature: Bucket C tools return job_id immediately with state=pending
  As an LLM agent driving substrate
  I want archive.zip.create and archive.tar.create to return a job_id synchronously
  So that I can track long-running archive operations without blocking on the RPC call

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the client has completed MCP initialization with progressToken support

  Scenario: archive.zip.create on a source larger than 10 MiB returns job receipt within 50 ms
    Given an allowlist root "/work/data" containing a directory tree larger than 10 MiB
    When the client calls archive.zip.create with src="/work/data" and dest="/work/out.zip" and a progressToken
    Then the server returns a structuredContent response within 50 ms
    And the response hints map contains field "job_id" matching the UUIDv7 Crockford pattern
    And the response hints map contains field "job_state" equal to "pending"
    And the response hints map contains field "polling_endpoint" equal to "job.status"

  Scenario: archive.tar.create returns job_id and emits SUBSTRATE_JOB_STATE_TRANSITION audit with state=pending
    Given an allowlist root "/work/src" containing source files
    When the client calls archive.tar.create with src="/work/src" and dest="/work/out.tar"
    Then the server returns a structuredContent response containing a "job_id" in the hints map
    And an audit event is emitted with tool_name matching "archive_tar_create"
    And the audit event has field "job_state" equal to "pending"
    And the audit event has field "correlation_id" equal to the returned job_id

  Scenario: Bucket A tool sys.hostname does NOT return job_id
    When the client calls sys.hostname
    Then the server returns an inline result without a "job_id" field in structuredContent hints
    And the response arrives within 10 ms
