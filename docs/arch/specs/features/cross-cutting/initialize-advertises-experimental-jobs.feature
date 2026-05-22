Feature: MCP initialize response advertises experimental capabilities for jobs and tier diagnostics
  As an LLM agent client connecting to substrate
  I want the initialize response to declare experimental capability extensions
  So that I can discover job control-plane support and tier diagnostics without out-of-band configuration

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: initialize response includes capabilities.experimental.substrate.jobs true when substrate-jobs crate is wired
    Given the substrate-jobs crate is compiled and wired into the composition root
    When the client sends an MCP initialize request with the current protocol version
    Then the initialize response includes field capabilities.experimental.substrate.jobs equal to true

  Scenario: initialize response includes capabilities.experimental.substrate.simd_tier with the SimdTier string value
    Given substrate has completed the capability probe and detected a SimdTier
    When the client sends an MCP initialize request
    Then the initialize response includes field capabilities.experimental.substrate.simd_tier
    And that field value is one of "avx512", "avx2", "sse42", "sse2", "neon", or "portable"
    And the value matches the simd_tier field from the SUBSTRATE_SIMD_TIER_DETECTED audit event emitted at startup

  Scenario: initialize response includes capabilities.experimental.substrate.platform_tiers as an object mapping port name to tier name
    Given substrate has completed the capability probe and selected tiers for all ports
    When the client sends an MCP initialize request
    Then the initialize response includes field capabilities.experimental.substrate.platform_tiers
    And that field is a JSON object where each key is a port name such as "DirWalker", "FsWatcher", "PathJail", "Hash", or "Stat"
    And each value is the chosen_tier string returned by the corresponding PortFactory

  Scenario: Client on MCP protocol older than 2025-11-25 receives experimental capabilities but no progress notifications
    Given the client sends an MCP initialize request declaring protocolVersion "2025-06-18"
    When substrate processes the initialize handshake and computes capability intersection
    Then the initialize response still includes capabilities.experimental.substrate.jobs
    And the initialize response still includes capabilities.experimental.substrate.simd_tier
    And the initialize response still includes capabilities.experimental.substrate.platform_tiers
    But the initialize response does not include capabilities.experimental.elicitation in the intersection
    And the job control-plane pull-only path remains usable for that client session
