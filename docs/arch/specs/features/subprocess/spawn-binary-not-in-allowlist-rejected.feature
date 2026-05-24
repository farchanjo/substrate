# ADR-0052 cross-ref: subprocess bounded context — binary allowlist enforcement
# ADR-0004 cross-ref: security model Layer 1 (default-deny for unlisted binaries)
Feature: Spawn binary outside allowlist is rejected at Layer 1
  As an LLM agent using substrate
  I want the server to reject subprocess.spawn for binaries not in the allowlist
  So that arbitrary code execution outside the declared trust boundary is impossible

  Scenario: Spawn binary outside allowlist is rejected at Layer 1
    Given binary "/usr/bin/curl" is NOT in security.subprocess_binary_allowlist
    When subprocess.spawn is invoked with binary_path "/usr/bin/curl"
    Then the response is an error with code SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED
    And no child process is created
