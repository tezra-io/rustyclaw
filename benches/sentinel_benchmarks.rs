//! Criterion benchmarks for the Sentinel redaction engine.
//!
//! Run: `cargo bench --bench sentinel_benchmarks`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box;

use rustyclaw::security::sentinel::config::RedactionConfig;
use rustyclaw::security::sentinel::engine::SentinelEngine;

fn clean_ascii_message(size: usize) -> String {
    "Hello, this is a clean message with no secrets. ".repeat((size / 49) + 1)[..size].to_string()
}

fn message_with_secrets() -> String {
    "Here is some text with sk-ant-api03-abc123DEF456_ghi789JKL012mno and also \
     Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U \
     and postgres://admin:s3cret@db.example.com:5432/mydb embedded in a longer message."
        .to_string()
}

fn pathological_64kb() -> String {
    // 64KB of sk-prefixed garbage
    let chunk = "sk-abcdefghijklmnopqrstuvwxyz1234567890ABCDE ";
    chunk.repeat(64 * 1024 / chunk.len())
}

fn bench_clean_ascii(c: &mut Criterion) {
    let config = RedactionConfig {
        log_redactions: false,
        ..Default::default()
    };
    let engine = SentinelEngine::new(&config).unwrap();

    let mut group = c.benchmark_group("sentinel_clean_ascii");
    for size in [64, 1024, 4096, 16384] {
        let msg = clean_ascii_message(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, input| {
            b.iter(|| engine.redact(black_box(input)));
        });
    }
    group.finish();
}

fn bench_with_redaction(c: &mut Criterion) {
    let config = RedactionConfig {
        log_redactions: false,
        ..Default::default()
    };
    let engine = SentinelEngine::new(&config).unwrap();

    let msg = message_with_secrets();
    c.bench_function("sentinel_with_redaction", |b| {
        b.iter(|| engine.redact(black_box(&msg)));
    });
}

fn bench_pathological(c: &mut Criterion) {
    let config = RedactionConfig {
        log_redactions: false,
        ..Default::default()
    };
    let engine = SentinelEngine::new(&config).unwrap();

    let msg = pathological_64kb();
    c.bench_function("sentinel_pathological_64kb", |b| {
        b.iter(|| engine.redact(black_box(&msg)));
    });
}

criterion_group!(
    benches,
    bench_clean_ascii,
    bench_with_redaction,
    bench_pathological
);
criterion_main!(benches);
