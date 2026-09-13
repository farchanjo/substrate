# ADR-0054 cross-ref: subprocess stdout/stderr stream multiplex via notifications/progress
# ADR-0052 cross-ref: subprocess bounded context — stderr stream separately tagged
Feature: stderr chunks stream via notifications/progress with separate stream marker
  As an LLM agent using substrate
  I want subprocess stderr chunks delivered as notifications/progress events distinct from stdout
  So that I can route diagnostic output without mixing it with structured stdout

  Scenario: stderr chunks stream via notifications/progress with separate stream marker
    Given subprocess.spawn is invoked with capture_kind "stream"
    When the child writes 4096 bytes to stderr
    Then at least 1 notifications/progress event is emitted with stream "stderr"
    And the event payload chunk_base64 decodes to the expected bytes
