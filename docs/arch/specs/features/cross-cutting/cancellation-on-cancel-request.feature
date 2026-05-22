Feature: Operations are cancelled within 1 second of receiving $/cancelRequest
  As an LLM agent driving substrate
  I want in-flight operations to respect cancellation signals promptly
  So that I can recover from runaway operations without waiting for timeout

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: Long-running fs.find is cancelled via $/cancelRequest within 1 second
    Given the client has sent fs.find with root="/work/repo" which is running
    When the client sends $/cancelRequest for the in-flight fs.find request id
    Then the server returns an error response with code SUBSTRATE_CANCELLED within 1 second
    And no further result chunks are emitted for that request

  Scenario: Long-running text.search is cancelled via $/cancelRequest
    Given the client has sent text.search with root="/work/repo" which is running
    When the client sends $/cancelRequest for the in-flight text.search request id
    Then the server returns an error response with code SUBSTRATE_CANCELLED within 1 second
    And partial results from before cancellation are not included in the final response

  Scenario: Cancellation of an already-completed request is a no-op
    Given a fs.find request that has already returned its final response
    When the client sends $/cancelRequest for the completed request id
    Then the server does not return an error
    And the server does not emit duplicate results

  Scenario: Cancellation token propagates to CancellationToken inside the handler
    Given the client has sent archive.tar_create which is compressing data
    When the client sends $/cancelRequest for the archive.tar_create request id
    Then the CancellationToken associated with the handler is signalled as cancelled
    And the server returns SUBSTRATE_CANCELLED within 1 second
