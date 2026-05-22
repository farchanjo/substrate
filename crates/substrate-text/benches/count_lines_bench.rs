//! Criterion benchmark comparing bytecount vs naive newline counting — `substrate-text`.
//!
//! Acceptance per ADR-0043: `bytecount::count` must be >=3x faster than the
//! naive iterator on inputs >= 256 KiB. This bench exercises both paths on a
//! 512 KiB buffer so the baseline can be captured in CI.
#![allow(
    clippy::missing_docs_in_private_items,
    clippy::missing_panics_doc,
    clippy::naive_bytecount,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark harness: naive path is the intentional comparison baseline; docs not required"
)]
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

const BUF_SIZE: usize = 512 * 1024; // 512 KiB

fn make_buf() -> Vec<u8> {
    // Alternate newlines every 64 bytes to create a realistic line density.
    let mut buf = vec![b'x'; BUF_SIZE];
    for i in (63..BUF_SIZE).step_by(64) {
        buf[i] = b'\n';
    }
    buf
}

fn bench_count_lines_bytecount(c: &mut Criterion) {
    let buf = make_buf();

    let mut group = c.benchmark_group("count_lines_512kib");
    group.throughput(Throughput::Bytes(BUF_SIZE as u64));

    group.bench_function("bytecount_simd", |b| {
        b.iter(|| {
            let n = bytecount::count(black_box(&buf), b'\n');
            black_box(n);
        });
    });

    group.bench_function("naive_iter", |b| {
        b.iter(|| {
            let n: usize = black_box(&buf).iter().filter(|&&b| b == b'\n').count();
            black_box(n);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_count_lines_bytecount);
criterion_main!(benches);
