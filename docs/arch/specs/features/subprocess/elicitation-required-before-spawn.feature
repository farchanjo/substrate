# ADR-0052 cross-ref: subprocess bounded context — mandatory elicitation gate
# ADR-0004 cross-ref: security model Layer 4 (elicitation mandatory for destructive ops)
Feature: subprocess.spawn without elicitation_confirmed is gated at Layer 4
  As an LLM agent using substrate
  I want the server to require explicit human confirmation before spawning any child process
  So that no subprocess is started without operator awareness

  Scenario: subprocess.spawn without elicitation_confirmed is gated at Layer 4
    Given subprocess.spawn is invoked without elicitation_confirmed flag
    Then an MCP elicitation form is emitted to the client describing the spawn request
    And no child process is created until elicitation_confirmed is true
    And re-invocation with elicitation_confirmed false also returns SUBSTRATE_ELICITATION_REQUIRED
