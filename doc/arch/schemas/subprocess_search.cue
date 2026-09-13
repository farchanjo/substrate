// DDD role: ValueObject
package schemas

// Search surface of the subprocess bounded context: the regex search request, a
// single matching line, and the paged response. Per ADR-0057.

// #StreamList is the set of child output channels a search is scoped to.
#StreamList: [...#Stream]

// #SearchMatchList is one page of matching output lines.
#SearchMatchList: [...#SearchMatch]

// #SubprocessSearchRequest submits a regex search across captured subprocess output
// per ADR-0057. Results are line-oriented and optionally paginated.
#SubprocessSearchRequest: {
	// job_id identifies the target job (Crockford base32, 26 chars; aliases #JobId).
	job_id: #JobId

	// pattern is the regex applied to each captured output line.
	// Length: 1..1024 characters.
	pattern: string & =~"^.{1,1024}$"

	// streams limits search to the specified output channels.
	// Default: both stdout and stderr.
	streams: #StreamList | *["stdout", "stderr"]

	// case_insensitive, when true, applies the regex in case-insensitive mode.
	case_insensitive: bool | *false

	// pagination, when present, enables paged retrieval of matching lines.
	pagination?: #Pagination
}

// #SearchMatch is a single line that matched the search pattern per ADR-0057.
// line_number is 1-based and scoped per stream (stdout and stderr each start at 1).
#SearchMatch: {
	// stream identifies the output channel that produced the matching line.
	stream: #Stream

	// line_number is the 1-based line index within the identified stream.
	line_number: int & >=1

	// line_text is the raw text content of the matching line (newline excluded).
	line_text: #ShortText
}

// #SubprocessSearchResult is the response returned by subprocess.search per ADR-0057.
// matches contains the page of #SearchMatch entries for this request; total_matches
// reflects the full match count across all pages.
#SubprocessSearchResult: {
	// matches is the current page of matching lines.
	matches: #SearchMatchList

	// total_matches is the total number of lines matching the pattern across all pages.
	total_matches: int & >=0

	// next_offset, when present, is the pagination offset to pass in the next request
	// to retrieve the subsequent page. Absent indicates the last page.
	next_offset?: int & >=0
}