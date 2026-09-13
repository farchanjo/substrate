Feature: proc.signal requires elicitation before sending destructive signals
  As a safety control in substrate
  I want destructive signals to require explicit user confirmation
  So that processes are not terminated without human awareness

  # ADR-0004 Layer 4 and ADR-0008 §Elicitation Matrix both specify that
  # SIGKILL, SIGTERM, and SIGSTOP require elicitation confirmation.

  Background:
    Given the host has a running process with pid=9876 and name="worker"
    And the process pid=9876 is within the allowed PID range

  Scenario: SIGKILL without elicitation confirmation returns SUBSTRATE_CONFIRMATION_REQUIRED
    When the client calls proc.signal with pid=9876 and signal="SIGKILL" and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the process pid=9876 is still running

  Scenario: SIGKILL with confirmed elicitation terminates the process
    When the client calls proc.signal with pid=9876 and signal="SIGKILL" and elicitation_confirmed=true
    Then the process pid=9876 is no longer running
    And the tool returns a success result with the signal sent and target pid

  Scenario: SIGTERM without elicitation confirmation returns SUBSTRATE_CONFIRMATION_REQUIRED
    # SIGTERM is included in the elicitation-required set per ADR-0004 and ADR-0008.
    When the client calls proc.signal with pid=9876 and signal="SIGTERM" and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the process pid=9876 is still running

  Scenario: SIGTERM with confirmed elicitation sends the signal
    When the client calls proc.signal with pid=9876 and signal="SIGTERM" and elicitation_confirmed=true
    Then the signal SIGTERM is sent to process pid=9876
    And the tool returns a success result with the signal sent and target pid

  Scenario Outline: Destructive signals require elicitation
    When the client calls proc.signal with pid=9876 and signal=<signal> and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED

    Examples:
      | signal   |
      | SIGKILL  |
      | SIGTERM  |
      | SIGSTOP  |
