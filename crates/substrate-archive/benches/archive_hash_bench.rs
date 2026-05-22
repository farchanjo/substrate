//! Criterion benchmark for blake3 hashing of large archives — `substrate-archive`.
//!
//! Uses a synthetic 10 MiB in-memory buffer to measure raw blake3 throughput
//! as the baseline for the `archive.hash` tool (ADR-0030, ADR-0043).

#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark binary: panics are the correct failure mode; missing_docs not required"
)]

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

const TEN_MIB: usize = 10 * (1 << 20);

fn bench_blake3_archive_10mib(c: &mut Criterion) {
    let buf: Vec<u8> = (0..TEN_MIB).map(|i| (i & 0xFF) as u8).collect();

    let mut group = c.benchmark_group("blake3_archive_10mib");
    group.throughput(Throughput::Bytes(TEN_MIB as u64));

    group.bench_function("blake3_mem", |b| {
        b.iter(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(black_box(&buf));
            black_box(hasher.finalize());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_blake3_archive_10mib);
criterion_main!(benches);
