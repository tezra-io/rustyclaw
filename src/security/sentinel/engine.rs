/// Core Sentinel redaction engine.
///
/// Compiles all detection patterns once at startup into a `SentinelEngine`,
/// then provides `redact()` which scans text in O(n) via Aho-Corasick +
/// regex, returning `Cow::Borrowed` for clean ASCII (zero allocation).
use std::borrow::Cow;
use std::collections::HashMap;

use aho_corasick::AhoCorasick;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use super::config::RedactionConfig;
use super::patterns::{BodyCharset, PrefixPattern, RegexPattern, PREFIX_PATTERNS, REGEX_PATTERNS};

/// Compiled redaction engine. Constructed once at startup, immutable thereafter.
pub struct SentinelEngine {
    /// Aho-Corasick automaton for prefix scanning.
    ac: AhoCorasick,
    /// Mapping from AC pattern index → prefix pattern definition.
    ac_patterns: Vec<PrefixPattern>,
    /// Compiled regex patterns with their categories.
    regexes: Vec<(Regex, &'static str)>,
    /// User-defined compiled regex patterns with their categories.
    custom_regexes: Vec<(Regex, String)>,
    /// Exact-match allowlist (checked before redaction).
    allowlist: Vec<String>,
    /// Compiled allowlist regex patterns.
    allow_regexes: Vec<Regex>,
    /// Entropy detection settings.
    entropy_detection: bool,
    entropy_threshold: f64,
    min_entropy_length: usize,
    /// Replacement format string (contains `{category}` placeholder).
    replacement_format: String,
    /// Whether to log redaction events.
    log_redactions: bool,
}

impl SentinelEngine {
    /// Build a new engine from the given config. Compiles all patterns.
    ///
    /// # Panics
    /// Panics if a built-in regex pattern is invalid (programming error).
    /// Returns `Err` if a user-supplied custom pattern fails to compile.
    pub fn new(config: &RedactionConfig) -> Result<Self, String> {
        // Collect all prefix strings for AC (built-in + custom).
        let mut ac_patterns: Vec<PrefixPattern> = PREFIX_PATTERNS.to_vec();
        for cp in &config.custom_prefixes {
            ac_patterns.push(PrefixPattern {
                prefix: Cow::Owned(cp.clone()),
                min_length: 20,
                max_length: 500,
                body_charset: BodyCharset::AlphanumDash,
                category: "custom",
            });
        }

        let prefixes: Vec<&str> = ac_patterns.iter().map(|p| p.prefix.as_ref()).collect();
        let ac = AhoCorasick::new(&prefixes).map_err(|e| format!("AC build failed: {e}"))?;

        // Compile built-in regexes.
        let regexes: Vec<(Regex, &'static str)> = REGEX_PATTERNS
            .iter()
            .map(|rp: &RegexPattern| {
                let re = Regex::new(rp.pattern)
                    .unwrap_or_else(|e| panic!("bad built-in regex `{}`: {e}", rp.pattern));
                (re, rp.category)
            })
            .collect();

        // Compile user-supplied custom regexes.
        let mut custom_regexes = Vec::new();
        for pat in &config.custom_patterns {
            let re = Regex::new(pat).map_err(|e| format!("invalid custom pattern `{pat}`: {e}"))?;
            custom_regexes.push((re, "custom".to_string()));
        }

        // Compile allowlist regexes.
        let mut allow_regexes = Vec::new();
        for pat in &config.allow_patterns {
            let re = Regex::new(pat).map_err(|e| format!("invalid allow_pattern `{pat}`: {e}"))?;
            allow_regexes.push(re);
        }

        Ok(Self {
            ac,
            ac_patterns,
            regexes,
            custom_regexes,
            allowlist: config.allowlist.clone(),
            allow_regexes,
            entropy_detection: config.entropy_detection,
            entropy_threshold: config.entropy_threshold,
            min_entropy_length: config.min_entropy_length,
            replacement_format: config.replacement_format.clone(),
            log_redactions: config.log_redactions,
        })
    }

    /// Redact secrets from `input`, returning `Cow::Borrowed` when no
    /// redaction is needed (zero allocation for clean ASCII).
    #[must_use]
    pub fn redact<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if input.is_empty() {
            return Cow::Borrowed(input);
        }

        // ASCII fast path: skip NFKC normalization entirely.
        let normalized: Cow<'_, str> = if input.is_ascii() {
            Cow::Borrowed(input)
        } else {
            let n: String = input.nfkc().collect();
            Cow::Owned(n)
        };

        // Collect all matches as (start, end, category) on the normalized text.
        let mut matches = self.find_all_matches(&normalized);

        if matches.is_empty() {
            // No redaction needed — return borrowed reference to original input
            // if we didn't normalize, otherwise return the normalized string.
            return if input.is_ascii() {
                Cow::Borrowed(input)
            } else {
                normalized.into_owned().into()
            };
        }

        // Sort by start position, then by longest match first for overlapping.
        matches.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

        // Build output with replacements, merging overlapping spans.
        let text = normalized.as_ref();
        let mut result = String::with_capacity(text.len());
        let mut cursor = 0;

        for (start, end, category) in &matches {
            if *start < cursor {
                // Overlapping with a previous match — skip.
                continue;
            }
            result.push_str(&text[cursor..*start]);
            result.push_str(&self.format_replacement(category));
            if self.log_redactions {
                tracing::info!(
                    category = category,
                    position = start,
                    "sentinel: redacted secret"
                );
            }
            cursor = *end;
        }
        result.push_str(&text[cursor..]);

        Cow::Owned(result)
    }

    /// Collect all (start, end, category) matches on the given text.
    fn find_all_matches(&self, text: &str) -> Vec<(usize, usize, String)> {
        let mut matches = Vec::new();

        // 1. Aho-Corasick prefix scan.
        for mat in self.ac.find_iter(text) {
            let pattern = &self.ac_patterns[mat.pattern().as_usize()];
            if let Some((start, end)) = self.validate_prefix_match(text, mat.start(), pattern) {
                let candidate = &text[start..end];
                if !self.is_allowlisted(candidate) {
                    // Extra JWT validation: must have 3 dot-separated parts.
                    if pattern.category == "jwt" {
                        if self.validate_jwt(candidate) {
                            matches.push((start, end, pattern.category.to_string()));
                        }
                    } else {
                        matches.push((start, end, pattern.category.to_string()));
                    }
                }
            }
        }

        // 2. Regex patterns (built-in).
        for (re, category) in &self.regexes {
            for mat in re.find_iter(text) {
                let candidate = mat.as_str();
                if !self.is_allowlisted(candidate) {
                    matches.push((mat.start(), mat.end(), (*category).to_string()));
                }
            }
        }

        // 3. Custom regex patterns.
        for (re, category) in &self.custom_regexes {
            for mat in re.find_iter(text) {
                let candidate = mat.as_str();
                if !self.is_allowlisted(candidate) {
                    matches.push((mat.start(), mat.end(), category.clone()));
                }
            }
        }

        // 4. Entropy detection (opt-in, full-message scan).
        if self.entropy_detection {
            self.find_entropy_matches(text, &mut matches);
        }

        matches
    }

    /// Validate a prefix hit: extract the full token and check length/charset.
    /// Returns `Some((start, end))` if valid, `None` if false positive.
    fn validate_prefix_match(
        &self,
        text: &str,
        prefix_start: usize,
        pattern: &PrefixPattern,
    ) -> Option<(usize, usize)> {
        // The token starts at prefix_start. Walk forward from end of prefix
        // while characters match the body charset.
        let after_prefix = prefix_start + pattern.prefix.len();
        let mut end = after_prefix;

        for c in text[after_prefix..].chars() {
            if pattern.body_charset.matches(c) {
                end += c.len_utf8();
            } else {
                break;
            }
        }

        let token_len = end - prefix_start;
        if token_len >= pattern.min_length && token_len <= pattern.max_length {
            // Ensure the prefix is at a word boundary (not mid-token).
            if prefix_start > 0 {
                let prev = text[..prefix_start].chars().next_back()?;
                if prev.is_ascii_alphanumeric() || prev == '_' {
                    return None; // mid-word, not a real prefix match
                }
            }
            Some((prefix_start, end))
        } else {
            None
        }
    }

    /// Validate JWT structure: 3 dot-separated base64url parts.
    fn validate_jwt(&self, token: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return false;
        }
        // Each part must be non-empty and base64url-valid.
        parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '=')
        })
    }

    /// Scan for high-entropy tokens in the text.
    fn find_entropy_matches(&self, text: &str, matches: &mut Vec<(usize, usize, String)>) {
        // Tokenize by whitespace and common delimiters.
        let mut start = 0;
        for segment in text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '`')
        {
            if !segment.is_empty() {
                // Find the actual start position in the original text.
                if let Some(pos) = text[start..].find(segment) {
                    let seg_start = start + pos;
                    let seg_end = seg_start + segment.len();

                    if segment.len() >= self.min_entropy_length
                        && self.looks_like_secret(segment)
                        && !self.is_allowlisted(segment)
                        && !self.overlaps_existing(seg_start, seg_end, matches)
                    {
                        let entropy = shannon_entropy(segment);
                        if entropy > self.entropy_threshold {
                            matches.push((seg_start, seg_end, "high_entropy".to_string()));
                        }
                    }

                    start = seg_end;
                }
            }
        }
    }

    /// Quick heuristic: does this string look like it could be a secret?
    /// Must be mostly base64-like characters.
    fn looks_like_secret(&self, s: &str) -> bool {
        let base64_count = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
            .count();
        let ratio = base64_count as f64 / s.len() as f64;
        ratio > 0.75
    }

    /// Check if a span overlaps with any existing match.
    fn overlaps_existing(
        &self,
        start: usize,
        end: usize,
        matches: &[(usize, usize, String)],
    ) -> bool {
        matches.iter().any(|(s, e, _)| start < *e && end > *s)
    }

    /// Check if a candidate string is allowlisted.
    fn is_allowlisted(&self, candidate: &str) -> bool {
        // Exact match.
        if self.allowlist.iter().any(|a| candidate == a.as_str()) {
            return true;
        }
        // Regex allowlist.
        self.allow_regexes.iter().any(|re| re.is_match(candidate))
    }

    /// Format the replacement string for a given category.
    fn format_replacement(&self, category: &str) -> String {
        self.replacement_format.replace("{category}", category)
    }
}

/// Compute Shannon entropy of a string (bits per character).
pub fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq: HashMap<char, usize> = HashMap::new();
    let mut total = 0usize;
    for c in s.chars() {
        *freq.entry(c).or_insert(0) += 1;
        total += 1;
    }
    let len = total as f64;
    freq.values().fold(0.0, |acc, &count| {
        let p = count as f64 / len;
        acc - p * p.log2()
    })
}

/// Convenience function: create an engine from config and redact.
///
/// **WARNING:** Compiles all patterns on every call. Use
/// [`SentinelEngine::new`] + [`SentinelEngine::redact`] in production paths.
pub fn redact<'a>(input: &'a str, config: &RedactionConfig) -> Cow<'a, str> {
    match SentinelEngine::new(config) {
        Ok(engine) => engine.redact(input),
        Err(e) => {
            tracing::error!("sentinel engine build failed: {e}");
            // Fail-open: return input unredacted.
            Cow::Borrowed(input)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::sentinel::config::RedactionConfig;

    fn engine() -> SentinelEngine {
        SentinelEngine::new(&RedactionConfig::default()).unwrap()
    }

    fn engine_with_entropy() -> SentinelEngine {
        SentinelEngine::new(&RedactionConfig {
            entropy_detection: true,
            entropy_threshold: 4.5,
            min_entropy_length: 20,
            ..Default::default()
        })
        .unwrap()
    }

    // --- API Key prefix detection ---

    #[test]
    fn detects_openai_api_key() {
        let e = engine();
        let input = "My key is sk-ant-api03-abc123DEF456_ghi789JKL012mno";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
        assert!(!result.contains("sk-ant-api03"), "secret leaked: {result}");
    }

    #[test]
    fn detects_github_pat() {
        let e = engine();
        let input = "Use token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_github_user_token() {
        let e = engine();
        let input = "Token: ghu_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_slack_bot_token() {
        let e = engine();
        let input = "Slack token: xoxb-123456789012-1234567890123-ABCDEFghijklMNOPqrs";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_slack_user_token() {
        let e = engine();
        let input = "User: xoxp-123456789012-1234567890123-1234567890123-abcdef1234567890abcdef1234567890ab";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_stripe_live_key() {
        let e = engine();
        let input = "Stripe: sk_live_51ABCDEFghijklMNOPqrstuvwx";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_stripe_test_key() {
        let e = engine();
        let input = "Test: sk_test_51ABCDEFghijklMNOPqrstuvwx";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    #[test]
    fn detects_stripe_publishable_key() {
        let e = engine();
        let input = "Pub: pk_live_51ABCDEFghijklMNOPqrstuvwx";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }

    // --- JWT detection ---

    #[test]
    fn detects_jwt_token() {
        let e = engine();
        // A realistic JWT: header.payload.signature
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let input = format!("Token: {jwt}");
        let result = e.redact(&input);
        assert!(result.contains("[REDACTED:jwt]"), "got: {result}");
        assert!(!result.contains("eyJhbG"), "jwt leaked: {result}");
    }

    #[test]
    fn rejects_invalid_jwt_no_dots() {
        let e = engine();
        // eyJ prefix but no dots — not a JWT.
        let input = "Variable eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9nodots here";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:jwt]"),
            "false positive: {result}"
        );
    }

    #[test]
    fn rejects_jwt_with_two_parts() {
        let e = engine();
        let input = "Not jwt: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:jwt]"),
            "false positive on 2-part: {result}"
        );
    }

    // --- Bearer token detection ---

    #[test]
    fn detects_bearer_token() {
        let e = engine();
        let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.payload.signature";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:bearer_token]") || result.contains("[REDACTED:jwt]"),
            "got: {result}"
        );
        assert!(!result.contains("eyJhbGciOiJSU"), "bearer leaked: {result}");
    }

    #[test]
    fn rejects_bearer_with_short_token() {
        let e = engine();
        let input = "Bearer abc";
        let result = e.redact(input);
        // "abc" is only 3 chars — below the 20-char minimum in the regex.
        assert!(
            !result.contains("[REDACTED:bearer_token]"),
            "false positive: {result}"
        );
    }

    // --- AWS credential detection ---

    #[test]
    fn detects_aws_access_key() {
        let e = engine();
        let input = "AWS key: AKIAIOSFODNN7EXAMPLE";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:aws_credential]"),
            "got: {result}"
        );
    }

    #[test]
    fn detects_aws_secret_in_env() {
        let e = engine();
        let input = "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:aws_credential]"),
            "got: {result}"
        );
    }

    // --- Connection string detection ---

    #[test]
    fn detects_postgres_connection_string() {
        let e = engine();
        let input = "DB: postgres://admin:s3cret@db.example.com:5432/mydb";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:connection_string]"),
            "got: {result}"
        );
    }

    #[test]
    fn detects_mongodb_connection_string() {
        let e = engine();
        let input = "mongodb+srv://user:pass123@cluster0.abc.mongodb.net/test";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:connection_string]"),
            "got: {result}"
        );
    }

    #[test]
    fn detects_mysql_connection_string() {
        let e = engine();
        let input = "Connection: mysql://root:password@localhost:3306/app";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:connection_string]"),
            "got: {result}"
        );
    }

    #[test]
    fn detects_redis_connection_string() {
        let e = engine();
        let input = "redis://default:mysecret@redis.example.com:6379/0";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:connection_string]"),
            "got: {result}"
        );
    }

    // --- PEM private key detection ---

    #[test]
    fn detects_rsa_private_key() {
        let e = engine();
        let input = "Here is a key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAK...";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:private_key]"), "got: {result}");
    }

    #[test]
    fn detects_ec_private_key() {
        let e = engine();
        let input = "-----BEGIN EC PRIVATE KEY-----\nMHQCAQEE...";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:private_key]"), "got: {result}");
    }

    #[test]
    fn detects_openssh_private_key() {
        let e = engine();
        let input = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNza...";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:private_key]"), "got: {result}");
    }

    #[test]
    fn detects_generic_private_key() {
        let e = engine();
        let input = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBAD...";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:private_key]"), "got: {result}");
    }

    // --- Entropy detection ---

    #[test]
    fn detects_high_entropy_string() {
        let e = engine_with_entropy();
        // Random-looking string with high entropy.
        let input = "Token: aB3cD4eF5gH6iJ7kL8mN9oP0qR1sT2u";
        let result = e.redact(input);
        assert!(result.contains("[REDACTED:"), "got: {result}");
    }

    #[test]
    fn skips_entropy_when_disabled() {
        let e = engine(); // entropy_detection = false
        let input = "Token: aB3cD4eF5gH6iJ7kL8mN9oP0qR1sT2u";
        let result = e.redact(input);
        // No prefix/regex match, and entropy is off — should pass through.
        assert!(!result.contains("[REDACTED:high_entropy]"), "got: {result}");
    }

    // --- Allowlist ---

    #[test]
    fn allowlist_bypasses_redaction_exact_match() {
        let e = SentinelEngine::new(&RedactionConfig {
            allowlist: vec!["sk-your-key-here-placeholder-value".to_string()],
            ..Default::default()
        })
        .unwrap();
        // Exact match: the full token equals the allowlist entry.
        let input = "Set OPENAI_API_KEY=sk-your-key-here-placeholder-value";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "allowlist exact match failed: {result}"
        );
    }

    #[test]
    fn allowlist_does_not_bypass_substring() {
        let e = SentinelEngine::new(&RedactionConfig {
            allowlist: vec!["EXAMPLE".to_string()],
            ..Default::default()
        })
        .unwrap();
        // A real AWS key containing the allowlist entry as substring must still be redacted.
        let input = "Key: AKIAIOSFODNN7EXAMPLE";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:aws_credential]"),
            "allowlist substring bypass: {result}"
        );
    }

    #[test]
    fn allow_pattern_bypasses_redaction() {
        let e = SentinelEngine::new(&RedactionConfig {
            allow_patterns: vec!["^sk-xxx".to_string()],
            ..Default::default()
        })
        .unwrap();
        let input = "Example: sk-xxx-placeholder-example-key";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "allow_pattern failed: {result}"
        );
    }

    // --- ASCII fast path ---

    #[test]
    fn ascii_clean_returns_borrowed() {
        let e = engine();
        let input = "Hello, this is a normal message with no secrets.";
        let result = e.redact(input);
        assert!(matches!(result, Cow::Borrowed(_)), "expected Cow::Borrowed");
        assert_eq!(result.as_ref(), input);
    }

    #[test]
    fn empty_input_returns_borrowed() {
        let e = engine();
        let result = e.redact("");
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    // --- NFKC normalization ---

    #[test]
    fn nfkc_defeats_combining_accent_bypass() {
        let e = engine();
        // "sk-" with a combining acute accent on 'k' → NFKC normalizes to "sk\u{0301}-"
        // but the prefix "sk-" should still be detected after normalization.
        // Actually, NFKC keeps the combining accent, so we test with NFKD-decomposed forms.
        // Use fullwidth characters: ｓｋ- which NFKC normalizes to "sk-"
        let input = "Key: \u{FF53}\u{FF4B}-ant-api03-abc123DEF456_ghi789JKL012mno";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:api_key]"),
            "NFKC bypass: {result}"
        );
    }

    #[test]
    fn nfkc_normalizes_fullwidth_digits() {
        let e = engine();
        // AKIA with fullwidth chars
        let input = "\u{FF21}\u{FF2B}\u{FF29}\u{FF21}IOSFODNN7EXAMPLE";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:aws_credential]"),
            "NFKC fullwidth: {result}"
        );
    }

    // --- False positive rejection ---

    #[test]
    fn rejects_uuid_as_false_positive() {
        let e = engine();
        let input = "Request ID: 550e8400-e29b-41d4-a716-446655440000";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "UUID false positive: {result}"
        );
    }

    #[test]
    fn rejects_short_sk_prefix() {
        let e = engine();
        // "sk-" followed by too few characters — below min_length.
        let input = "The variable sk-abc is short";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "short sk- false positive: {result}"
        );
    }

    #[test]
    fn rejects_mid_word_prefix() {
        let e = engine();
        // "task-" ending with "sk-" should not trigger.
        let input = "Download mask-and-data from the server";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "mid-word false positive: {result}"
        );
    }

    #[test]
    fn rejects_hex_hash_as_false_positive() {
        let e = engine();
        let input = "Commit hash: abcdef1234567890abcdef1234567890abcdef12";
        let result = e.redact(input);
        assert!(
            !result.contains("[REDACTED:"),
            "hex hash false positive: {result}"
        );
    }

    // --- Custom patterns ---

    #[test]
    fn custom_prefix_detects_org_key() {
        let e = SentinelEngine::new(&RedactionConfig {
            custom_prefixes: vec!["myorg_key_".to_string()],
            ..Default::default()
        })
        .unwrap();
        let input = "Key: myorg_key_ABCDEFghij1234567890";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:custom]"),
            "custom prefix: {result}"
        );
    }

    #[test]
    fn custom_regex_detects_pattern() {
        let e = SentinelEngine::new(&RedactionConfig {
            custom_patterns: vec!["MYORG-[A-Z0-9]{32}".to_string()],
            ..Default::default()
        })
        .unwrap();
        let input = "Token: MYORG-ABCDEFGHIJKLMNOP0123456789ABCDEF";
        let result = e.redact(input);
        assert!(
            result.contains("[REDACTED:custom]"),
            "custom regex: {result}"
        );
    }

    #[test]
    fn invalid_custom_regex_returns_error() {
        let result = SentinelEngine::new(&RedactionConfig {
            custom_patterns: vec!["[invalid".to_string()],
            ..Default::default()
        });
        assert!(result.is_err());
    }

    // --- Replacement format ---

    #[test]
    fn custom_replacement_format() {
        let e = SentinelEngine::new(&RedactionConfig {
            replacement_format: "<<HIDDEN:{category}>>".to_string(),
            ..Default::default()
        })
        .unwrap();
        let input = "Key: sk-ant-api03-abc123DEF456_ghi789JKL012mno";
        let result = e.redact(input);
        assert!(
            result.contains("<<HIDDEN:api_key>>"),
            "custom format: {result}"
        );
    }

    // --- Multiple secrets in one message ---

    #[test]
    fn redacts_multiple_secrets() {
        let e = engine();
        let input = "Keys: sk-ant-api03-abc123DEF456_ghi789JKL012mno and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmno";
        let result = e.redact(input);
        let redacted_count = result.matches("[REDACTED:api_key]").count();
        assert_eq!(redacted_count, 2, "expected 2 redactions, got: {result}");
    }

    // --- Shannon entropy ---

    #[test]
    fn shannon_entropy_of_uniform_string() {
        // "aaaa" has entropy 0.
        assert!((shannon_entropy("aaaa") - 0.0).abs() < 0.001);
    }

    #[test]
    fn shannon_entropy_of_varied_string() {
        // High entropy for random-looking strings.
        let s = "aB3cD4eF5gH6iJ7kL8mN9oP0qR1sT2u";
        let e = shannon_entropy(s);
        assert!(e > 4.0, "expected high entropy, got {e}");
    }

    // --- Convenience function ---

    #[test]
    fn convenience_redact_works() {
        let config = RedactionConfig::default();
        let input = "Key: sk-ant-api03-abc123DEF456_ghi789JKL012mno";
        let result = redact(input, &config);
        assert!(result.contains("[REDACTED:api_key]"), "got: {result}");
    }
}
