//! Criterion benchmark for blake3 hashing — `substrate-fs-query`.
//!
//! Acceptance per ADR-0043: blake3 throughput must be >=2x faster than sha2
//! on inputs >= 1 MiB. This bench establishes the blake3 baseline.
#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark binary: panics are the correct failure mode; missing_docs not required"
)]

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::io::Write as _;
use tempfile::NamedTempFile;

const ONE_MIB: usize = 1 << 20;

fn bench_blake3_file_1mib(c: &mut Criterion) {
    // Write synthetic data to a temp file so the bench exercises real I/O.
    let mut file = NamedTempFile::new().expect("tempfile creation failed");
    file.write_all(&vec![0xAB_u8; ONE_MIB])
        .expect("tempfile write failed");
    let path = file.path().to_path_buf();

    let mut group = c.benchmark_group("blake3_hash_1mib");
    group.throughput(Throughput::Bytes(ONE_MIB as u64));

    group.bench_function("blake3_file_read_hash", |b| {
        b.iter(|| {
            let bytes = std::fs::read(black_box(&path)).expect("fs::read failed");
            let mut hasher = blake3::Hasher::new();
            hasher.update(&bytes);
            black_box(hasher.finalize());
        });
    });

    group.finish();
}

fn bench_blake3_mem_1mib(c: &mut Criterion) {
    // In-memory variant: isolates hashing throughput from I/O latency.
    let buf: Vec<u8> = vec![0xCD_u8; ONE_MIB];

    let mut group = c.benchmark_group("blake3_hash_mem_1mib");
    group.throughput(Throughput::Bytes(ONE_MIB as u64));

    group.bench_function("blake3_mem", |b| {
        b.iter(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(black_box(&buf));
            black_box(hasher.finalize());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_blake3_file_1mib, bench_blake3_mem_1mib);
criterion_main!(benches);
