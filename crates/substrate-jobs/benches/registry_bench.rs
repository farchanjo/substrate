//! Criterion benchmark for `InMemoryJobRegistry` submit + cancel cycle — `substrate-jobs`.
//!
//! Measures the round-trip latency of submitting a no-op job and immediately
//! cancelling it. Uses a single-threaded tokio runtime so the bench is
//! deterministic and avoids parking overhead from the multi-threaded scheduler.

#![expect(
    clippy::expect_used,
    missing_docs,
    reason = "benchmark harness: expect() panics are acceptable, docs not required for bench fns"
)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use futures::FutureExt as _;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use substrate_domain::jobs::bucket::JobBucket;
use substrate_domain::jobs::config::JobConfig;
use substrate_domain::ports::job_registry::{JobRegistryPort as _, JobSubmitRequest};
use substrate_domain::value_objects::ClientId;
use substrate_jobs::InMemoryJobRegistry;
use substrate_jobs::NoopProgressNotifier;

fn make_registry(cancel: CancellationToken) -> Arc<InMemoryJobRegistry> {
    InMemoryJobRegistry::new(JobConfig::default(), Arc::new(NoopProgressNotifier), cancel)
}

fn bench_submit_cancel(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio Runtime::new failed");

    c.bench_function("registry_submit_cancel", |b| {
        b.iter(|| {
            rt.block_on(async {
                let cancel = CancellationToken::new();
                let registry = make_registry(cancel.clone());

                let client_id = ClientId::parse("bench-client").expect("ClientId::parse failed");
                let request = JobSubmitRequest {
                    client_id: client_id.clone(),
                    tool: "bench.noop".to_owned(),
                    bucket: JobBucket::CAlwaysAsync,
                    idempotency_key: None,
                    args_json: serde_json::Value::Null,
                    execute: async { Ok(serde_json::json!({"ok": true})) }.boxed(),
                };

                let job_id = registry
                    .submit(black_box(request))
                    .await
                    .expect("submit failed");

                let _state = registry
                    .cancel(black_box(&job_id))
                    .await
                    .expect("cancel failed");

                // Signal shutdown so the background GC task terminates cleanly.
                cancel.cancel();
            });
        });
    });
}

criterion_group!(benches, bench_submit_cancel);
criterion_main!(benches);
