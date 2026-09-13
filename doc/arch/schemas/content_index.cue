// DDD role: ValueObject
package schemas

// #Bm25Params configures Okapi BM25 relevance ranking per ADR-0072.
// Defaults follow the canonical Robertson/Zaragoza parameterization used by
// most production search engines (Lucene, Elasticsearch).
#Bm25Params: {
	// k1 controls term-frequency saturation: higher values let repeated term
	// occurrences within one document continue to raise its score for longer
	// before the marginal contribution of an additional occurrence flattens.
	k1: number & >=0.0 & <=3.0 | *1.2

	// b controls document-length normalization: 0.0 disables length
	// normalization entirely (a long document is not penalized relative to a
	// short one for the same raw term frequency); 1.0 applies it fully.
	b: number & >=0.0 & <=1.0 | *0.75
}

// #TokenPositions carries the zero-based token offsets of every occurrence of
// one term within one document.
#TokenPositions: [...uint]

// #ByteOffsets carries absolute byte offsets within a file.
#ByteOffsets: [...uint]

// #PostingEntry is a single document's occurrence record for one term within
// one #IndexShard, per ADR-0072's content inverted index.
#PostingEntry: {
	// doc_id references the indexed document by its stable per-shard
	// identifier. Not stable across shards; a document's postings never span
	// more than one shard.
	doc_id: #Counter

	// term_freq is the number of occurrences of the term within the document.
	term_freq: uint & >=1

	// positions carries the zero-based token offsets of each occurrence,
	// captured once at index time and used to build a context-window snippet
	// without re-scanning the file at query time for the common (fresh) case.
	positions: #TokenPositions
}

// #PostingEntryList is the per-document occurrence list of one term, sorted by
// doc_id.
#PostingEntryList: [...#PostingEntry]

// #PostingList aggregates every #PostingEntry for a single term within one
// #IndexShard, plus the document frequency the BM25 IDF term consumes.
#PostingList: {
	// term is the normalized token text (case-folded per the index's
	// tokenization policy).
	term: string & !=""

	// doc_freq is the number of distinct documents containing term within
	// this shard; feeds the BM25 inverse-document-frequency component.
	doc_freq: uint & >=1

	// entries is the per-document occurrence list, sorted by doc_id.
	entries: #PostingEntryList
}

// #IndexShard (a.k.a. segment) is one unit of the LSM-lite segmented content
// index per ADR-0072. At most one shard in a published snapshot is the
// mutable-until-next-publish "hot" delta; every other shard is immutable,
// Arc-shared, and produced either by an initial rebuild or by a background
// compaction merge. Shards are never mutated in place once frozen.
#IndexShard: {
	// shard_id is a monotonically increasing per-process identifier.
	shard_id: #Counter

	// generation is the publish generation at which this shard's current
	// contents became visible to readers (see #IndexEvent ShardPublished).
	generation: #Counter

	// doc_count is the number of documents represented in this shard.
	doc_count: uint & >=0

	// term_count is the number of distinct terms in this shard's postings.
	// Zero when content_index_enabled is false (metadata-only shard).
	term_count: uint & >=0

	// bytes_estimated is a rough memory footprint, accounted against the same
	// max_bytes / max_entries eviction budget as #IndexConfig.
	bytes_estimated: uint & >=0

	// is_hot marks the mutable, frequently-rebuilt delta shard. At most one
	// shard per published snapshot has is_hot: true.
	is_hot: bool | *false
}

// #IndexCommand is the single-writer work-queue message consumed by the
// IndexerActor per ADR-0072. Producers: fs-mutation write-through (Layer 1),
// the FS watcher (Layer 2), and the TTL rebuild ticker (Layer 3). Transport:
// one bounded mpsc<IndexCommand> channel; see ADR-0072 §Concurrency Model.
// Discriminated union on `kind`; each variant carries only the fields its
// case needs.
#IndexCommand: {
	{
		kind: "Upsert"
		path: string & !=""

		// content_hash is present only when content_index_enabled and the
		// document's content was read as part of producing this command.
		content_hash?: string & =~"^[0-9a-f]{64}$" // blake3 hex digest

		// token_count is present only alongside content_hash; feeds BM25
		// document-length normalization (avgdl).
		token_count?: uint & >=0

		// mtime and size are always present: they back both the metadata
		// index and the Layer-0 fast-path revalidation check.
		mtime: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"
		size:  uint & >=0
	} | {
		kind: "Remove"
		path: string & !=""
	} | {
		kind: "Rescan"
		root:   string & !=""
		reason: "watch_overflow" | "queue_saturated" | "allowlist_reload"
	} | {
		kind: "Rebuild"
		root:   string & !=""
		reason: "ttl_expired" | "startup" | "operator_requested"
	}
}

// #IndexEvent is the broadcast<IndexEvent> fan-out message the IndexerActor
// publishes after each committed batch per ADR-0072. A subscriber that lags
// (per tokio::sync::broadcast's RecvError::Lagged semantics) MUST treat the
// lag as equivalent to a missed Rescan and re-validate rather than trust its
// local cache — see ADR-0072 §Consequences (Risks).
#IndexEvent: {
	{
		kind:       "Upserted"
		path:       string & !=""
		generation: uint
	} | {
		kind:       "Removed"
		path:       string & !=""
		generation: uint
	} | {
		kind:       "ShardPublished"
		generation: uint
		shard_id:   uint
	}
}

// #RankedMatch is one ranked content-search hit returned by text.search when
// the content index is active, per ADR-0072. Supersedes the unranked
// (file_path, line_number, line_text) tuple for an indexed root; text.search
// falls back to the unranked shape automatically when the content index is
// disabled or the root has not completed its first content rebuild — see
// ADR-0072 §Fallback Semantics and hints.relevance_ranked.
#RankedMatch: {
	// path is the matched file.
	path: string & !=""

	// score is the BM25 relevance score (higher is more relevant). Not
	// normalized across queries; meaningful only for ranking within one
	// response.
	score: #Score

	// snippet is a context-window excerpt built via grep-searcher around the
	// highest-scoring match position in this document.
	snippet: #ShortText

	// snippet_start_line is the 1-based line number of the first line
	// included in snippet.
	snippet_start_line: uint & >=1

	// match_offsets carries the byte offsets, within the file, of the term
	// occurrences that contributed to this document's score.
	match_offsets: #ByteOffsets

	// term_freq is the total matched-term occurrence count in this document
	// across all matched query terms.
	term_freq: uint & >=1
}
