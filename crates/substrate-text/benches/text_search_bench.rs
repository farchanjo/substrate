//! Criterion benchmark for regex-based text search — `substrate-text`.
//!
//! Measures throughput of `regex::Regex::find_iter` over a ~1 MiB haystack.
//! Used as the regression baseline for the `text.search` tool (ADR-0030).
#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "benchmark harness: expect in bench setup is acceptable; docs not required"
)]

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use regex::Regex;

fn bench_regex_search_1mib(c: &mut Criterion) {
    // ~44 bytes * 24 000 repetitions ≈ 1 MiB
    let haystack: String = "the quick brown fox jumps over the lazy dog\n".repeat(24_000);
    let regex = Regex::new(r"\bfox\b").expect("regex compile failed");

    let mut group = c.benchmark_group("text_search_1mib");
    group.throughput(Throughput::Bytes(haystack.len() as u64));

    group.bench_function("regex_find_iter", |b| {
        b.iter(|| {
            let count: usize = regex.find_iter(black_box(&haystack)).count();
            black_box(count);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_regex_search_1mib);
criterion_main!(benches);
