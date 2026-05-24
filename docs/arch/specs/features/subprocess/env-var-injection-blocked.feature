# ADR-0052 cross-ref: subprocess bounded context — env var injection prevention
# ADR-0004 cross-ref: security model Layer 5 (subprocess sandbox)
Feature: LD_PRELOAD in env_override is rejected regardless of allowlist
  As an LLM agent using substrate
  I want the server to unconditionally block library-injection environment variables
  So that dynamic linker hijacking is prevented even if the caller attempts to supply them

  Scenario: LD_PRELOAD in env_override is rejected regardless of allowlist
    Given subprocess.spawn is invoked with env_override containing key LD_PRELOAD
    When the subprocess_invariants Rego policy evaluates the request
    Then the policy denies with msg containing "banned env var"
    And no child process is created
    And error code SUBSTRATE_SUBPROCESS_ENV_BANNED is returned
