//! Criterion benchmark comparing simdutf8 vs std UTF-8 validation — `substrate-text`.
//!
//! Acceptance per ADR-0043: `simdutf8::basic::from_utf8` must be >=5x faster
//! than `std::str::from_utf8` on a 1 MiB valid UTF-8 buffer.
#![allow(
    clippy::cast_possible_truncation,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "benchmark harness: cast is bounded by ONE_MIB constant; docs not required"
)]

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

const ONE_MIB: usize = 1 << 20;

fn bench_simdutf8_vs_std(c: &mut Criterion) {
    // Build a 1 MiB buffer of valid ASCII (subset of UTF-8).
    let buf: Vec<u8> = (0..ONE_MIB).map(|i| (i % 95 + 32) as u8).collect();

    let mut group = c.benchmark_group("utf8_validate_1mib");
    group.throughput(Throughput::Bytes(ONE_MIB as u64));

    group.bench_function("simdutf8_basic", |b| {
        b.iter(|| {
            let result = simdutf8::basic::from_utf8(black_box(&buf));
            black_box(result.is_ok());
        });
    });

    group.bench_function("std_from_utf8", |b| {
        b.iter(|| {
            let result = std::str::from_utf8(black_box(&buf));
            black_box(result.is_ok());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_simdutf8_vs_std);
criterion_main!(benches);
