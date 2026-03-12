//! Sentinel: visibility-boundary secret redaction & unicode sanitization engine.
//!
//! Scans outbound messages for secrets (API keys, JWTs, connection strings,
//! private keys, etc.) and replaces them with `[REDACTED:<category>]`.
//! Sanitizes inbound messages of dangerous unicode (zero-width chars, bidi
//! overrides, tag characters, homoglyphs).
//!
//! See `docs/sentinel-gateway-redaction-design.md` for the full design.

pub mod config;
pub mod engine;
pub mod middleware;
pub mod patterns;
pub mod redacting_channel;
pub mod redacting_tool;
pub mod sanitize_config;
pub mod sanitizer;
pub mod schema;

pub use config::RedactionConfig;
#[allow(unused_imports)]
pub use engine::{redact, SentinelEngine};
#[allow(unused_imports)]
pub use redacting_channel::RedactingChannel;
pub use sanitize_config::SanitizationConfig;
pub use sanitizer::SanitizationEngine;
