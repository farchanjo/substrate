Feature: Per-client and global quotas reject excess job submissions
  As an LLM agent driving substrate
  I want the server to enforce concurrency quotas and return clear errors when limits are reached
  So that one client cannot starve other clients or exhaust server resources

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.max_per_client set to 4
    And the server configuration has jobs.max_concurrent set to 16

  Scenario: Client A with 4 active jobs receives SUBSTRATE_QUOTA_EXCEEDED on the 5th submit
    Given client "client-A" has submitted 4 archive.tar.create jobs all currently running
    When client "client-A" submits a 5th archive.tar.create job
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_QUOTA_EXCEEDED"
    And the error object has field "recovery_hint" mentioning "per-client" or "max_per_client"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And no new worker task is spawned

  Scenario: Global cap of 16 active jobs reached returns SUBSTRATE_QUOTA_EXCEEDED to any client
    Given the server has 16 active jobs distributed across multiple clients
    When client "client-B" submits any Bucket C job
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_QUOTA_EXCEEDED"
    And the error object has field "recovery_hint" mentioning "global" or "max_concurrent"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters

  Scenario: After a job completes the freed slot allows the next submission to succeed
    Given client "client-A" has 4 active jobs and the per-client cap is 4
    When one of client "client-A"'s jobs transitions to state succeeded
    And client "client-A" submits a new archive.tar.create job
    Then the server returns a job receipt with a valid job_id in the hints map
    And the response does not contain an error object
