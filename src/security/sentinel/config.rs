/// Configuration for the Sentinel redaction engine.
/// Controls which patterns are detected, allowlist behavior, and entropy scanning.
#[derive(Debug, Clone)]
pub struct RedactionConfig {
    /// Additional prefix patterns to detect (e.g., `["myorg_key_"]`).
    pub custom_prefixes: Vec<String>,
    /// Additional regex patterns to detect (e.g., `["MYORG-[A-Z0-9]{32}"]`).
    pub custom_patterns: Vec<String>,
    /// Exact strings that skip redaction (e.g., `["sk-your-key-here"]`).
    pub allowlist: Vec<String>,
    /// Regex patterns whose matches skip redaction (e.g., `["^sk-xxx"]`).
    pub allow_patterns: Vec<String>,
    /// Enable high-entropy string detection (off by default — most expensive check).
    pub entropy_detection: bool,
    /// Shannon entropy threshold for flagging a token (default 4.5).
    pub entropy_threshold: f64,
    /// Minimum token length for entropy analysis (default 20).
    pub min_entropy_length: usize,
    /// Format string for replacements. `{category}` is substituted.
    pub replacement_format: String,
    /// Log redaction events (category + position, never the secret).
    pub log_redactions: bool,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            custom_prefixes: Vec::new(),
            custom_patterns: Vec::new(),
            allowlist: Vec::new(),
            allow_patterns: Vec::new(),
            entropy_detection: false,
            entropy_threshold: 4.5,
            min_entropy_length: 20,
            replacement_format: "[REDACTED:{category}]".to_string(),
            log_redactions: true,
        }
    }
}
