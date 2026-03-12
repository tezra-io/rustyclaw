/// Pattern definitions for the Sentinel redaction engine.
///
/// Each pattern category has a detection method (prefix via Aho-Corasick,
/// regex, or entropy) and validation rules to reduce false positives.
use std::borrow::Cow;

// --- Prefix patterns (Aho-Corasick) ---

/// A prefix pattern detected via Aho-Corasick with post-match validation.
#[derive(Debug, Clone)]
pub struct PrefixPattern {
    /// The prefix string to match (e.g., `"sk-"`).
    pub prefix: Cow<'static, str>,
    /// Minimum total length of the matched token (prefix + body).
    pub min_length: usize,
    /// Maximum total length of the matched token.
    pub max_length: usize,
    /// Allowed charset in the body after the prefix (regex character class fragment).
    pub body_charset: BodyCharset,
    /// Category label used in `[REDACTED:<category>]`.
    pub category: &'static str,
}

/// Allowed character sets for the body portion after a prefix match.
#[derive(Debug, Clone, Copy)]
pub enum BodyCharset {
    /// Alphanumeric plus common token chars: `[A-Za-z0-9_\-]`
    AlphanumDash,
    /// Base64-url safe: `[A-Za-z0-9_\-\.]`
    Base64Url,
    /// Uppercase alphanumeric: `[A-Z0-9]`
    UpperAlphaNum,
    /// Hex: `[A-Fa-f0-9]`
    Hex,
}

impl BodyCharset {
    pub fn matches(self, c: char) -> bool {
        match self {
            Self::AlphanumDash => c.is_ascii_alphanumeric() || c == '_' || c == '-',
            Self::Base64Url => c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.',
            Self::UpperAlphaNum => c.is_ascii_uppercase() || c.is_ascii_digit(),
            Self::Hex => c.is_ascii_hexdigit(),
        }
    }
}

/// Built-in prefix patterns for API key detection.
pub const PREFIX_PATTERNS: &[PrefixPattern] = &[
    // OpenAI / Anthropic style keys
    PrefixPattern {
        prefix: Cow::Borrowed("sk-"),
        min_length: 20,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // GitHub personal access tokens
    PrefixPattern {
        prefix: Cow::Borrowed("ghp_"),
        min_length: 40,
        max_length: 100,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // GitHub user tokens
    PrefixPattern {
        prefix: Cow::Borrowed("ghu_"),
        min_length: 40,
        max_length: 100,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // Slack bot tokens
    PrefixPattern {
        prefix: Cow::Borrowed("xoxb-"),
        min_length: 30,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // Slack user tokens
    PrefixPattern {
        prefix: Cow::Borrowed("xoxp-"),
        min_length: 30,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // AWS access key IDs
    PrefixPattern {
        prefix: Cow::Borrowed("AKIA"),
        min_length: 20,
        max_length: 20,
        body_charset: BodyCharset::UpperAlphaNum,
        category: "aws_credential",
    },
    // Stripe live publishable keys
    PrefixPattern {
        prefix: Cow::Borrowed("pk_live_"),
        min_length: 20,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // Stripe live secret keys
    PrefixPattern {
        prefix: Cow::Borrowed("sk_live_"),
        min_length: 20,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // Stripe test secret keys
    PrefixPattern {
        prefix: Cow::Borrowed("sk_test_"),
        min_length: 20,
        max_length: 200,
        body_charset: BodyCharset::AlphanumDash,
        category: "api_key",
    },
    // JWT tokens
    PrefixPattern {
        prefix: Cow::Borrowed("eyJ"),
        min_length: 20,
        max_length: 8192,
        body_charset: BodyCharset::Base64Url,
        category: "jwt",
    },
];

/// Regex pattern definition for structured secret detection.
#[derive(Debug, Clone)]
pub struct RegexPattern {
    /// The regex pattern string.
    pub pattern: &'static str,
    /// Category label used in `[REDACTED:<category>]`.
    pub category: &'static str,
}

/// Built-in regex patterns for structured secret detection.
pub const REGEX_PATTERNS: &[RegexPattern] = &[
    // Bearer tokens in authorization headers leaked in text
    RegexPattern {
        pattern: r"Bearer [A-Za-z0-9_\-\.]{20,}",
        category: "bearer_token",
    },
    // AWS secret access keys (40 char base64)
    RegexPattern {
        pattern: r"(?:aws_secret_access_key|AWS_SECRET_ACCESS_KEY)\s*[=:]\s*[A-Za-z0-9/+=]{40}",
        category: "aws_credential",
    },
    // Connection strings with credentials
    RegexPattern {
        pattern: r"(?:postgres|postgresql|mysql|mongodb\+srv|mongodb|redis|amqp|amqps)://[^\s@]+:[^\s@]+@[^\s]+",
        category: "connection_string",
    },
    // PEM private keys
    RegexPattern {
        pattern: r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----",
        category: "private_key",
    },
];
