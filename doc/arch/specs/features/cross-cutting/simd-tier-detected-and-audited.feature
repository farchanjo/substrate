Feature: SimdTier is probed once at startup and recorded in audit
  As an operator running substrate on varied hardware
  I want the SIMD tier to be detected at startup and written to the audit log
  So that I can confirm which instruction set is active without inspecting binary flags

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: On x86_64 host with AVX2 available, startup emits SUBSTRATE_SIMD_TIER_DETECTED with simd_tier avx2
    Given the host CPU reports AVX2 support via is_x86_feature_detected
    And the Cargo feature simd-avx2 is compiled in
    When substrate completes its capability probe during startup
    Then exactly one audit event with code "SUBSTRATE_SIMD_TIER_DETECTED" is written to stderr before the first MCP initialize response
    And that audit event has field "simd_tier" equal to "avx2"
    And the audit event has a non-empty "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: On aarch64 Apple Silicon host, startup emits SUBSTRATE_SIMD_TIER_DETECTED with simd_tier neon
    Given the host architecture is aarch64-apple-darwin
    And is_aarch64_feature_detected returns true for neon
    When substrate completes its capability probe during startup
    Then exactly one audit event with code "SUBSTRATE_SIMD_TIER_DETECTED" is written to stderr before the first MCP initialize response
    And that audit event has field "simd_tier" equal to "neon"
    And the audit event "seq" field value is 0

  Scenario: AVX-512 is not promoted when simd-avx512 Cargo feature is absent even if CPU reports avx512f
    Given the host CPU reports AVX-512F support via is_x86_feature_detected
    But the Cargo feature simd-avx512 is NOT compiled in
    When substrate completes its capability probe during startup
    Then the audit event with code "SUBSTRATE_SIMD_TIER_DETECTED" has field "simd_tier" equal to "avx2"
    And no audit event with simd_tier equal to "avx512" is emitted

  Scenario: AVX-512 is not promoted when security.allow_avx512 is false even if simd-avx512 Cargo feature is present
    Given the host CPU reports AVX-512F support via is_x86_feature_detected
    And the Cargo feature simd-avx512 is compiled in
    But the config key security.allow_avx512 is set to false
    When substrate completes its capability probe during startup
    Then the audit event with code "SUBSTRATE_SIMD_TIER_DETECTED" has field "simd_tier" equal to "avx2"
    And no audit event with simd_tier equal to "avx512" is emitted
