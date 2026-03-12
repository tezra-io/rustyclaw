use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use rustyclaw::security::sentinel::config::RedactionConfig;
use rustyclaw::security::sentinel::engine::SentinelEngine;
use rustyclaw::security::sentinel::sanitize_config::SanitizationConfig;
use rustyclaw::security::sentinel::sanitizer::SanitizationEngine;

fn bench_redaction(c: &mut Criterion) {
    let engine = SentinelEngine::new(&RedactionConfig::default()).unwrap();

    let mut group = c.benchmark_group("sentinel_redaction");

    // --- Clean ASCII messages at varying sizes ---
    for size in [64, 1024, 4096, 16384] {
        let base = "Hello, this is a completely normal and clean message. ";
        let msg = base.repeat((size / base.len()) + 2);
        let msg = &msg[..size];

        group.bench_with_input(BenchmarkId::new("clean_ascii", size), &msg, |b, input| {
            b.iter(|| {
                let _ = black_box(engine.redact(black_box(input)));
            });
        });
    }

    // --- Message requiring redaction ---
    let msg_with_key =
        "Here's the API key: sk-ant-api03-abc123DEF456_ghi789JKL012mno please use it";
    group.bench_function("single_secret", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(msg_with_key)));
        });
    });

    // --- Multiple secrets ---
    let msg_multi = "Keys: sk-ant-api03-abc123DEF456_ghi789JKL012mno and \
                     AKIAIOSFODNN7EXAMPLE and \
                     eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
    group.bench_function("multiple_secrets", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(msg_multi)));
        });
    });

    // --- Pathological: 64KB of sk-prefixed garbage ---
    let pathological =
        "sk-ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrs ".repeat(64 * 1024 / 51 + 1);
    let pathological = &pathological[..64 * 1024];
    group.bench_function("pathological_64kb", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(pathological)));
        });
    });

    // --- Message with emoji, markdown, code blocks ---
    let msg_with_emoji = "Here's the plan 🎯:\n\
                          1. Build the **engine** 🔧\n\
                          2. Run `cargo test` ✅\n\
                          3. Deploy 🚀\n\
                          \n```rust\nfn main() { println!(\"hello\"); }\n```\n\
                          No secrets here! 😎";
    group.bench_function("emoji_markdown", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(msg_with_emoji)));
        });
    });

    // --- Non-ASCII: Arabic text ---
    let arabic = "مرحبا بالعالم! هذه رسالة اختبار عادية بدون أي أسرار. ".repeat(10);
    group.bench_function("arabic_text", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(&arabic)));
        });
    });

    // --- Non-ASCII: Chinese text ---
    let chinese = "你好世界！这是一条正常的测试消息，没有任何秘密。".repeat(10);
    group.bench_function("chinese_text", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(&chinese)));
        });
    });

    // --- Mixed script content ---
    let mixed = "Hello 你好 مرحبا café 🎉 résumé naïve — normal text without secrets";
    group.bench_function("mixed_script", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(mixed)));
        });
    });

    // --- False positive corpus ---
    let false_positives = "UUID: 550e8400-e29b-41d4-a716-446655440000\n\
                           SHA256: a7f5f35426b927411fc93205c2aa4b058e94e3c4cc3f773e2b6e9b7e4d4c5c5c\n\
                           Base64 image: data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk\n\
                           Hex color: #FF5733\n\
                           Normal date: 2024-01-15T10:30:00Z";
    group.bench_function("false_positive_corpus", |b| {
        b.iter(|| {
            let _ = black_box(engine.redact(black_box(false_positives)));
        });
    });

    group.finish();
}

fn bench_sanitization(c: &mut Criterion) {
    let engine = SanitizationEngine::new(SanitizationConfig::default());

    let mut group = c.benchmark_group("sentinel_sanitization");

    // --- Clean ASCII ---
    let clean = "Normal clean ASCII message without any issues.";
    group.bench_function("clean_ascii", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(clean)));
        });
    });

    // --- Zero-width injection ---
    let zwsp = "hello\u{200B}world\u{200C}test\u{200B}message";
    group.bench_function("zero_width_strip", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(zwsp)));
        });
    });

    // --- NFKC normalization ---
    let fullwidth = "\u{FF21}\u{FF22}\u{FF23}\u{FF24}\u{FF25}\u{FF26}".repeat(10);
    group.bench_function("nfkc_fullwidth", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(&fullwidth)));
        });
    });

    // --- Mixed exploit payload ---
    let mixed_exploit = "\u{FEFF}Hello\u{200B}\u{202E}world\u{E0001}\u{2060}test\u{200D}text";
    group.bench_function("mixed_exploit", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(mixed_exploit)));
        });
    });

    // --- Emoji-heavy content (should preserve) ---
    let emoji = "🎯🔥💡✨ Developer 👨\u{200D}💻 says hello! 👨\u{200D}👩\u{200D}👧\u{200D}👦 Family time! 🚀";
    group.bench_function("emoji_preserve", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(emoji)));
        });
    });

    // --- Arabic/RTL text (should preserve) ---
    let arabic = "مرحبا بالعالم هذه رسالة باللغة العربية";
    group.bench_function("arabic_rtl", |b| {
        b.iter(|| {
            let _ = black_box(engine.sanitize(black_box(arabic)));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_redaction, bench_sanitization);
criterion_main!(benches);
