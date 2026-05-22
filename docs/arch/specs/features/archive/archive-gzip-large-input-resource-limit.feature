Feature: archive.gzip_compress enforces resource limit for large inputs
  As a resource safety control in substrate
  I want compression of inputs exceeding 1 GiB to be blocked by default
  So that unintended disk or memory exhaustion is prevented

  Background:
    Given an allowlist with root "/work/repo"

  Scenario: Input > 1 GiB without allow_large=true returns SUBSTRATE_RESOURCE_LIMIT
    Given the file "/work/repo/data/huge.bin" has a size of 1.5 GiB
    When the client calls archive.gzip_compress with src="/work/repo/data/huge.bin" and allow_large=false
    Then the tool returns error code SUBSTRATE_RESOURCE_LIMIT
    And no output file is written to disk

  Scenario: Input > 1 GiB with allow_large=true proceeds
    Given the file "/work/repo/data/huge.bin" has a size of 1.5 GiB
    When the client calls archive.gzip_compress with src="/work/repo/data/huge.bin" and allow_large=true and elicitation_confirmed=true
    Then the tool begins compressing the file
    And at least one ProgressNotification is emitted during compression
    And the output compressed file is written on completion

  Scenario: Input exactly at 1 GiB boundary without allow_large returns SUBSTRATE_RESOURCE_LIMIT
    Given the file "/work/repo/data/boundary.bin" has a size of exactly 1 GiB
    When the client calls archive.gzip_compress with src="/work/repo/data/boundary.bin" and allow_large=false
    Then the tool returns error code SUBSTRATE_RESOURCE_LIMIT

  Scenario: Input below 1 GiB does not require allow_large
    Given the file "/work/repo/data/small.bin" has a size of 512 MiB
    When the client calls archive.gzip_compress with src="/work/repo/data/small.bin" and allow_large=false and elicitation_confirmed=true
    Then the tool compresses the file without returning SUBSTRATE_RESOURCE_LIMIT
    And the output compressed file is written on completion
