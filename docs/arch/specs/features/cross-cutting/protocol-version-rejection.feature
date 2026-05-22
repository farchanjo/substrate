Feature: substrate rejects clients with unsupported protocol versions
  As a compatibility boundary in substrate
  I want connections from clients using protocol versions older than 2025-06-18 to be rejected at handshake time
  So that behavioral assumptions about newer protocol features are never violated

  Background:
    Given a running substrate server requiring protocolVersion >= "2025-06-18"

  Scenario: Client with protocolVersion "2024-01-01" is rejected at initialization
    When a client sends an initialize request with protocolVersion="2024-01-01"
    Then the server returns error code SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED
    And the connection is closed without processing further requests

  Scenario: Client with protocolVersion exactly "2025-06-18" is accepted
    When a client sends an initialize request with protocolVersion="2025-06-18"
    Then the server returns a successful initialize response
    And the client may proceed with tool calls

  Scenario: Client with protocolVersion newer than "2025-06-18" is accepted
    When a client sends an initialize request with protocolVersion="2026-01-01"
    Then the server returns a successful initialize response
    And the client may proceed with tool calls

  Scenario Outline: Specific outdated versions are all rejected
    When a client sends an initialize request with protocolVersion=<version>
    Then the server returns error code SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED

    Examples:
      | version      |
      | 2024-11-05   |
      | 2025-01-01   |
      | 2025-03-26   |
      | 2025-06-17   |
