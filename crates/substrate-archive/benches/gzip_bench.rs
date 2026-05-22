//! Criterion benchmark for gzip compression — `substrate-archive`.
//!
//! Measures throughput of `flate2::write::GzEncoder` at compression level 6
//! on a synthetic 1 MiB payload. Baseline for the `archive.tar.create` tool
//! gzip path (ADR-0030).

#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "benchmark binary: panics are the correct failure mode; missing_docs not required"
)]

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write as _;

const ONE_MIB: usize = 1 << 20;

fn bench_gzip_compress_level6(c: &mut Criterion) {
    // Use a compressible pattern (repeating ASCII) for realistic compression ratios.
    let input: Vec<u8> = "abcdefghijklmnopqrstuvwxyz\n"
        .bytes()
        .cycle()
        .take(ONE_MIB)
        .collect();

    let mut group = c.benchmark_group("gzip_compress_1mib");
    group.throughput(Throughput::Bytes(ONE_MIB as u64));

    group.bench_function("gz_level6", |b| {
        b.iter(|| {
            let mut encoder = GzEncoder::new(Vec::with_capacity(ONE_MIB / 4), Compression::new(6));
            encoder
                .write_all(black_box(&input))
                .expect("GzEncoder::write_all failed");
            let compressed = encoder.finish().expect("GzEncoder::finish failed");
            black_box(compressed.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_gzip_compress_level6);
criterion_main!(benches);
