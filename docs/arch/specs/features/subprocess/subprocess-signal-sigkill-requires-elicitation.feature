# ADR-0052 cross-ref: subprocess bounded context — elicitation gate for destructive signals
# ADR-0004 cross-ref: security model Layer 4 (elicitation mandatory for SIGKILL/SIGTERM/SIGSTOP)
Feature: subprocess.signal requires elicitation before sending destructive signals
  As a safety control in substrate
  I want destructive signals sent to subprocesses to require explicit user confirmation
  So that no subprocess is forcibly terminated without human awareness

  Background:
    Given the subprocess_binary_allowlist includes "/usr/bin/sleep"
    And a subprocess job is in Running state with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"
    And the signal_allowlist includes SIGKILL and SIGTERM and SIGSTOP

  Scenario: SIGKILL without elicitation confirmation returns SUBSTRATE_CONFIRMATION_REQUIRED
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGKILL" and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the subprocess job remains in Running state

  Scenario: SIGKILL with confirmed elicitation terminates the subprocess
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGKILL" and elicitation_confirmed=true
    Then the signal SIGKILL is delivered to the subprocess process group
    And the tool returns a success result with signal="SIGKILL" and job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"

  Scenario: SIGTERM without elicitation confirmation returns SUBSTRATE_CONFIRMATION_REQUIRED
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGTERM" and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the subprocess job remains in Running state

  Scenario: SIGTERM with confirmed elicitation sends the signal
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGTERM" and elicitation_confirmed=true
    Then the signal SIGTERM is delivered to the subprocess process group
    And the tool returns a success result with signal="SIGTERM" and job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"

  Scenario Outline: Destructive signals require elicitation
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal=<signal> and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED

    Examples:
      | signal   |
      | SIGKILL  |
      | SIGTERM  |
      | SIGSTOP  |
