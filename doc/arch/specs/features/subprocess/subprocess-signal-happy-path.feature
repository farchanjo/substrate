# ADR-0052 cross-ref: subprocess bounded context — subprocess.signal tool
# ADR-0004 cross-ref: security model Layer 4 (elicitation required for destructive signals)
Feature: subprocess.signal delivers an allowed signal to an owned subprocess
  As an LLM agent using substrate
  I want to send a non-destructive signal to a running subprocess by job_id
  So that I can pause, resume, or notify a child process without terminating it

  Background:
    Given the subprocess_binary_allowlist includes "/usr/bin/sleep"
    And a subprocess job is in Running state with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"
    And the signal_allowlist includes SIGUSR1 and SIGCONT and SIGSTOP

  Scenario: SIGUSR1 is delivered to a running subprocess with elicitation_confirmed=true
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGUSR1" and elicitation_confirmed=true
    Then the tool returns a success result
    And the result contains signal="SIGUSR1" and job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"
    And the subprocess job remains in Running state

  Scenario: SIGCONT is delivered to a paused subprocess with elicitation_confirmed=true
    Given the subprocess job is in Running state and was paused by a prior SIGSTOP
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR3" and signal="SIGCONT" and elicitation_confirmed=true
    Then the tool returns a success result
    And the result contains signal="SIGCONT" and job_id="01HXZQ1BVKM3YF4P7NTSW02AR3"

  Scenario: subprocess.signal against an unknown job_id returns SUBSTRATE_JOB_NOT_FOUND
    When subprocess.signal is invoked with job_id="01AAAAAAAAAAAAAAAAAAAAAAAA" and signal="SIGUSR1" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_JOB_NOT_FOUND

  Scenario: subprocess.signal against a Completed job returns SUBSTRATE_JOB_TERMINAL_STATE
    Given a subprocess job is in Completed state with job_id="01HXZQ1BVKM3YF4P7NTSW02AR4"
    When subprocess.signal is invoked with job_id="01HXZQ1BVKM3YF4P7NTSW02AR4" and signal="SIGUSR1" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_JOB_TERMINAL_STATE
