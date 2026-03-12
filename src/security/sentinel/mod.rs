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
pub mod patterns;
pub mod sanitize_config;
pub mod sanitizer;

pub use config::RedactionConfig;
#[allow(unused_imports)]
pub use engine::{redact, SentinelEngine};
pub use sanitize_config::SanitizationConfig;
pub use sanitizer::SanitizationEngine;
