Feature: proc.signal blocks signals to privileged and system PIDs
  As a security boundary in substrate
  I want signals to system-critical processes to be denied
  So that substrate cannot destabilize the host operating system

  Background:
    Given the substrate process signal allowlist excludes PID 1 and kernel threads

  Scenario: Sending any signal to PID 1 (init/systemd) returns SUBSTRATE_PERMISSION_DENIED
    When the client calls proc.signal with pid=1 and signal="SIGTERM" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PERMISSION_DENIED
    And the signal is not delivered to PID 1

  Scenario: Sending SIGKILL to PID 1 also returns SUBSTRATE_PERMISSION_DENIED
    When the client calls proc.signal with pid=1 and signal="SIGKILL" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PERMISSION_DENIED
    And PID 1 is still running

  Scenario Outline: Signaling well-known system PIDs returns SUBSTRATE_PERMISSION_DENIED
    When the client calls proc.signal with pid=<pid> and signal="SIGTERM" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PERMISSION_DENIED

    Examples:
      | pid |
      | 0   |
      | 1   |
      | 2   |

  Scenario: Signaling a PID within the allowlist succeeds
    Given the host has a running process with pid=5000 within the allowed PID range
    When the client calls proc.signal with pid=5000 and signal="SIGTERM" and elicitation_confirmed=false
    Then the signal SIGTERM is sent to process pid=5000
    And no SUBSTRATE_PERMISSION_DENIED error is returned
