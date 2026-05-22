//! Criterion benchmark for `ignore::WalkBuilder` filesystem traversal — `substrate-fs-query`.
//!
//! Creates a tempdir with 1 000 synthetic files across 10 subdirectories,
//! then measures walk speed. Throughput reported in elements (files enumerated).
#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::significant_drop_tightening,
    missing_docs,
    reason = "benchmark binary: panics are the correct failure mode; missing_docs not required"
)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use ignore::WalkBuilder;
use std::fs;
use tempfile::TempDir;

const FILES_PER_DIR: usize = 100;
const DIRS: usize = 10;
const TOTAL_FILES: usize = FILES_PER_DIR * DIRS;

/// Builds a tempdir with `DIRS` subdirectories, each containing `FILES_PER_DIR`
/// files filled with a short synthetic payload.
fn populate_tempdir() -> TempDir {
    let dir = TempDir::new().expect("TempDir::new failed");
    for d in 0..DIRS {
        let sub = dir.path().join(format!("sub_{d:03}"));
        fs::create_dir_all(&sub).expect("create_dir_all failed");
        for f in 0..FILES_PER_DIR {
            let path = sub.join(format!("file_{f:04}.txt"));
            fs::write(&path, b"synthetic bench payload\n").expect("fs::write failed");
        }
    }
    dir
}

fn bench_ignore_walker_1k_files(c: &mut Criterion) {
    let dir = populate_tempdir();
    let root = dir.path().to_path_buf();

    let mut group = c.benchmark_group("ignore_walker");
    group.throughput(Throughput::Elements(TOTAL_FILES as u64));

    group.bench_with_input(
        BenchmarkId::new("walk_1k_files", TOTAL_FILES),
        &root,
        |b, root| {
            b.iter(|| {
                let count = WalkBuilder::new(black_box(root))
                    .hidden(false)
                    .git_ignore(false)
                    .build()
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
                    .count();
                black_box(count);
            });
        },
    );

    group.finish();

    // Keep `dir` alive until here so the tempdir is not dropped prematurely.
    drop(dir);
}

criterion_group!(benches, bench_ignore_walker_1k_files);
criterion_main!(benches);
