/// Sentinel configuration schema for config.toml.
///
/// Controls redaction (outbound), sanitization (inbound), and external tool
/// scanning. Loaded at startup; custom patterns are validated immediately.
///
/// # Config Security
///
/// `config.toml` must not be writable by the agent process. Custom patterns
/// are loaded at startup only — no hot reload — to prevent an agent from
/// modifying its own allowlist at runtime.
///
/// Custom regex patterns use the `regex` crate (linear-time, no backreferences
/// or lookahead). Invalid patterns cause a startup error.
use serde::{Deserialize, Serialize};

/// Sentinel fail mode — what happens when the redaction/sanitization engine errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FailMode {
    /// Fail-open: on error, pass the original message through and log an alert.
    /// Default for most deployments.
    #[default]
    Open,
    /// Fail-closed: on error, reject the message entirely.
    /// For high-security deployments where leaking a secret is worse than dropping a message.
    Closed,
}

/// Top-level Sentinel configuration (`[security.sentinel]`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SentinelSchemaConfig {
    /// Master switch — disable all Sentinel processing.
    pub enabled: bool,

    /// Fail mode: `open` (default) or `closed`.
    pub fail_mode: FailMode,

    /// Outbound secret redaction configuration.
    pub redaction: RedactionSchemaConfig,

    /// Inbound unicode sanitization configuration.
    pub sanitization: SanitizationSchemaConfig,

    /// External tool argument scanning.
    pub external_tools: ExternalToolsSchemaConfig,
}

impl Default for SentinelSchemaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fail_mode: FailMode::default(),
            redaction: RedactionSchemaConfig::default(),
            sanitization: SanitizationSchemaConfig::default(),
            external_tools: ExternalToolsSchemaConfig::default(),
        }
    }
}

/// Outbound redaction configuration (`[security.sentinel.redaction]`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct RedactionSchemaConfig {
    /// Enable outbound redaction.
    pub enabled: bool,

    /// Additional prefix patterns to detect (e.g. `["myorg_key_"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_prefixes: Vec<String>,

    /// Additional regex patterns to detect (e.g. `["MYORG-[A-Z0-9]{32}"]`).
    /// Uses `regex` crate (linear-time, no backreferences/lookahead).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_patterns: Vec<String>,

    /// Exact strings that skip redaction (e.g. example keys in docs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowlist: Vec<String>,

    /// Regex patterns whose matches skip redaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_patterns: Vec<String>,

    /// Enable high-entropy string detection (expensive, off by default).
    #[serde(default)]
    pub entropy_detection: bool,

    /// Shannon entropy threshold (default 4.5).
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,

    /// Minimum token length for entropy analysis (default 20).
    #[serde(default = "default_min_entropy_length")]
    pub min_entropy_length: usize,

    /// Format string for replacements. `{category}` is substituted.
    #[serde(default = "default_replacement_format")]
    pub replacement_format: String,

    /// Log redaction events (category + position, never the secret itself).
    #[serde(default = "default_true")]
    pub log_redactions: bool,
}

impl Default for RedactionSchemaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_prefixes: Vec::new(),
            custom_patterns: Vec::new(),
            allowlist: Vec::new(),
            allow_patterns: Vec::new(),
            entropy_detection: false,
            entropy_threshold: default_entropy_threshold(),
            min_entropy_length: default_min_entropy_length(),
            replacement_format: default_replacement_format(),
            log_redactions: true,
        }
    }
}

/// Inbound sanitization configuration (`[security.sentinel.sanitization]`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct SanitizationSchemaConfig {
    /// Enable inbound sanitization.
    pub enabled: bool,

    /// Strip zero-width characters.
    #[serde(default = "default_true")]
    pub strip_zero_width: bool,

    /// Strip tag characters (U+E0001–U+E007F).
    #[serde(default = "default_true")]
    pub strip_tag_characters: bool,

    /// Apply NFKC normalization (collapses homoglyphs). Allocates for non-ASCII.
    #[serde(default = "default_true")]
    pub normalize_unicode: bool,

    /// Strip bidi override control characters.
    #[serde(default = "default_true")]
    pub strip_bidi_overrides: bool,

    /// Preserve ZWJ in emoji sequences.
    #[serde(default = "default_true")]
    pub preserve_emoji_zwj: bool,

    /// Also sanitize metadata fields (sender, reply_target) not just body.
    #[serde(default = "default_true")]
    pub sanitize_metadata_fields: bool,

    /// Log when sanitization modifies a message.
    #[serde(default = "default_true")]
    pub log_sanitizations: bool,
}

impl Default for SanitizationSchemaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strip_zero_width: true,
            strip_tag_characters: true,
            normalize_unicode: true,
            strip_bidi_overrides: true,
            preserve_emoji_zwj: true,
            sanitize_metadata_fields: true,
            log_sanitizations: true,
        }
    }
}

/// External tool scanning configuration (`[security.sentinel.external_tools]`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ExternalToolsSchemaConfig {
    /// Enable argument scanning for external-facing tools.
    pub scan_enabled: bool,
}

impl Default for ExternalToolsSchemaConfig {
    fn default() -> Self {
        Self { scan_enabled: true }
    }
}

// --- Default value helpers ---

fn default_true() -> bool {
    true
}

fn default_entropy_threshold() -> f64 {
    4.5
}

fn default_min_entropy_length() -> usize {
    20
}

fn default_replacement_format() -> String {
    "[REDACTED:{category}]".to_string()
}

// --- Conversion to engine configs ---

impl SentinelSchemaConfig {
    /// Convert the schema config to a `RedactionConfig` for the engine.
    pub fn to_redaction_config(&self) -> super::config::RedactionConfig {
        super::config::RedactionConfig {
            custom_prefixes: self.redaction.custom_prefixes.clone(),
            custom_patterns: self.redaction.custom_patterns.clone(),
            allowlist: self.redaction.allowlist.clone(),
            allow_patterns: self.redaction.allow_patterns.clone(),
            entropy_detection: self.redaction.entropy_detection,
            entropy_threshold: self.redaction.entropy_threshold,
            min_entropy_length: self.redaction.min_entropy_length,
            replacement_format: self.redaction.replacement_format.clone(),
            log_redactions: self.redaction.log_redactions,
        }
    }

    /// Convert the schema config to a `SanitizationConfig` for the engine.
    pub fn to_sanitization_config(&self) -> super::sanitize_config::SanitizationConfig {
        super::sanitize_config::SanitizationConfig {
            strip_zero_width: self.sanitization.strip_zero_width,
            strip_tag_characters: self.sanitization.strip_tag_characters,
            normalize_unicode: self.sanitization.normalize_unicode,
            strip_bidi_overrides: self.sanitization.strip_bidi_overrides,
            preserve_emoji_zwj: self.sanitization.preserve_emoji_zwj,
            sanitize_metadata_fields: self.sanitization.sanitize_metadata_fields,
            log_sanitizations: self.sanitization.log_sanitizations,
        }
    }

    /// Validate custom patterns at startup. Returns errors for invalid regexes.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        for (i, pattern) in self.redaction.custom_patterns.iter().enumerate() {
            if regex::Regex::new(pattern).is_err() {
                errors.push(format!(
                    "sentinel.redaction.custom_patterns[{i}]: invalid regex: {pattern}"
                ));
            }
        }

        for (i, pattern) in self.redaction.allow_patterns.iter().enumerate() {
            if regex::Regex::new(pattern).is_err() {
                errors.push(format!(
                    "sentinel.redaction.allow_patterns[{i}]: invalid regex: {pattern}"
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let config = SentinelSchemaConfig::default();
        assert!(config.enabled);
        assert_eq!(config.fail_mode, FailMode::Open);
        assert!(config.redaction.enabled);
        assert!(config.sanitization.enabled);
        assert!(config.external_tools.scan_enabled);
    }

    #[test]
    fn parses_from_toml_with_defaults() {
        let toml_str = r#"
enabled = true
fail_mode = "open"
"#;
        let config: SentinelSchemaConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert!(config.redaction.enabled);
        assert!(config.sanitization.strip_zero_width);
    }

    #[test]
    fn parses_fail_closed() {
        let toml_str = r#"fail_mode = "closed""#;
        let config: SentinelSchemaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.fail_mode, FailMode::Closed);
    }

    #[test]
    fn parses_custom_patterns() {
        let toml_str = r#"
[redaction]
custom_prefixes = ["myorg_"]
custom_patterns = ["MYORG-[A-Z0-9]{32}"]
allowlist = ["sk-example-placeholder"]
entropy_detection = true
"#;
        let config: SentinelSchemaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.redaction.custom_prefixes, vec!["myorg_"]);
        assert_eq!(config.redaction.custom_patterns, vec!["MYORG-[A-Z0-9]{32}"]);
        assert_eq!(config.redaction.allowlist, vec!["sk-example-placeholder"]);
        assert!(config.redaction.entropy_detection);
    }

    #[test]
    fn validates_valid_patterns() {
        let config = SentinelSchemaConfig {
            redaction: RedactionSchemaConfig {
                custom_patterns: vec!["MYORG-[A-Z0-9]{32}".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_regex_patterns() {
        let config = SentinelSchemaConfig {
            redaction: RedactionSchemaConfig {
                custom_patterns: vec!["[invalid".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("invalid regex"));
    }

    #[test]
    fn converts_to_redaction_config() {
        let schema = SentinelSchemaConfig {
            redaction: RedactionSchemaConfig {
                custom_prefixes: vec!["myorg_".to_string()],
                entropy_detection: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = schema.to_redaction_config();
        assert_eq!(config.custom_prefixes, vec!["myorg_"]);
        assert!(config.entropy_detection);
    }

    #[test]
    fn converts_to_sanitization_config() {
        let schema = SentinelSchemaConfig {
            sanitization: SanitizationSchemaConfig {
                strip_zero_width: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = schema.to_sanitization_config();
        assert!(!config.strip_zero_width);
    }

    #[test]
    fn skip_serializing_empty_vecs() {
        let config = SentinelSchemaConfig::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(
            !toml.contains("custom_prefixes"),
            "empty vec serialized: {toml}"
        );
        assert!(!toml.contains("allowlist"), "empty vec serialized: {toml}");
    }

    #[test]
    fn disabled_sanitization_fields() {
        let toml_str = r#"
[sanitization]
strip_zero_width = false
normalize_unicode = false
"#;
        let config: SentinelSchemaConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.sanitization.strip_zero_width);
        assert!(!config.sanitization.normalize_unicode);
        // Other fields still default to true
        assert!(config.sanitization.strip_bidi_overrides);
    }
}
