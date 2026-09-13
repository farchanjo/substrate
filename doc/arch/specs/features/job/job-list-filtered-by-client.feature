Feature: job.list is paginated and filtered by the requesting client_id
  As an LLM agent driving substrate
  I want job.list to return only the jobs submitted by my client
  So that cross-client job visibility is prevented and results are paginated for large sets

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And client "client-A" has submitted 3 archive.tar.create jobs
    And client "client-B" has submitted 2 archive.zip.create jobs

  Scenario: Client A only sees its own jobs in job.list response
    When client "client-A" calls job.list
    Then the response contains exactly the 3 jobs submitted by client "client-A"
    And no job submitted by client "client-B" appears in the response

  Scenario: page_size respects the pagination cap of 50 default and 500 maximum
    Given client "client-A" has submitted 60 archive.tar.create jobs
    When client "client-A" calls job.list without specifying page_size
    Then the response contains at most 50 job entries
    And the response contains a cursor field for the next page
    When client "client-A" calls job.list with page_size=600
    Then the server caps page_size at 500 and returns at most 500 job entries

  Scenario: Cursor round-trip retrieves the next page of results
    Given client "client-A" has submitted 60 archive.tar.create jobs
    When client "client-A" calls job.list with page_size=50 and no cursor
    Then the response contains 50 job entries and a non-empty cursor value
    When client "client-A" calls job.list with page_size=50 and the returned cursor value
    Then the response contains the remaining 10 job entries
    And the response does not contain a cursor field or contains an empty cursor field
