//! Sentinel: visibility-boundary secret redaction engine.
//!
//! Scans outbound messages for secrets (API keys, JWTs, connection strings,
//! private keys, etc.) and replaces them with `[REDACTED:<category>]`.
//! Uses Aho-Corasick for O(n) multi-pattern prefix matching and regex for
//! structural patterns. Returns `Cow::Borrowed` for clean ASCII messages
//! (zero allocation on the hot path).
//!
//! See `docs/sentinel-gateway-redaction-design.md` for the full design.

pub mod config;
pub mod engine;
pub mod patterns;

pub use config::RedactionConfig;
#[allow(unused_imports)]
pub use engine::{redact, SentinelEngine};
