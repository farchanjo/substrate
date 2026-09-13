# ADR-0056 cross-ref: subprocess supervisor semantics — idempotent spawn by name
# ADR-0040 cross-ref: async job control-plane — triple-equality contract preserved
Feature: subprocess.spawn with name field is idempotent per client_id
  As an LLM agent using substrate
  I want subprocess.spawn with the same name to return the existing job handle when the job is non-terminal
  So that I can safely call spawn multiple times without accidentally starting duplicate processes

  Scenario: spawn with name creates a new mapping when no prior mapping exists
    Given no (client_id, "spring-backend") mapping exists in the SupervisorRegistry
    When subprocess.spawn is invoked with name "spring-backend"
    Then a new child process is started
    And the response contains a server-assigned job_id
    And the SupervisorRegistry stores the mapping (client_id, "spring-backend") to that job_id

  Scenario: spawn with name returns existing handle when mapping is non-terminal
    Given subprocess.spawn was previously called with name "spring-backend"
    And that job is in a non-terminal state
    When subprocess.spawn is invoked again with name "spring-backend" from the same client_id
    Then no new process is started
    And the response contains the same job_id as the prior spawn
    And the response contains the same pgid as the prior spawn
    And the response structured content carries idempotent_by_name true

  Scenario: spawn with name spawns a fresh process when prior mapping is terminal
    Given subprocess.spawn was previously called with name "spring-backend"
    And that job has reached a terminal state
    When subprocess.spawn is invoked again with name "spring-backend" from the same client_id
    Then a new child process is started
    And the response contains a different job_id than the prior spawn
    And the SupervisorRegistry replaces the old mapping with the new job_id

  Scenario: spawn with same name from a different client_id is independent
    Given subprocess.spawn was called with name "spring-backend" from client_id A
    And that job is in a non-terminal state
    When subprocess.spawn is invoked with name "spring-backend" from client_id B
    Then a new child process is started for client_id B
    And the job_id returned to client_id B is different from the job_id for client_id A
    And each client_id maintains an independent (client_id, "spring-backend") mapping
