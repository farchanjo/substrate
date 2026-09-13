# ADR-0052 cross-ref: subprocess bounded context — per-client quota enforcement
# ADR-0040 cross-ref: async job control-plane quota model
Feature: Client exceeding subprocess.max_per_client quota receives SUBSTRATE_QUOTA_EXCEEDED
  As an LLM agent using substrate
  I want the server to enforce per-client subprocess quotas
  So that a single client cannot exhaust server resources with unbounded parallel spawns

  Scenario: client exceeding subprocess.max_per_client quota receives SUBSTRATE_QUOTA_EXCEEDED
    Given subprocess.max_per_client is 4
    And a single client_id has 4 subprocess jobs in Running state
    When that client invokes subprocess.spawn for a 5th
    Then the response is an error with code SUBSTRATE_QUOTA_EXCEEDED
    And no new JobEntry is created
