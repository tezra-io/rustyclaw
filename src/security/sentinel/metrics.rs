/// Sentinel observability: counters and histograms for redaction/sanitization events.
///
/// Redaction event logging logs category, position, and length — NEVER the secret itself.
/// Fail-open alerts use tracing::error! for high-visibility.
use std::sync::atomic::{AtomicU64, Ordering};

/// Global Sentinel metrics counters.
pub static METRICS: SentinelMetrics = SentinelMetrics::new();

/// Atomic counters for Sentinel events.
pub struct SentinelMetrics {
    /// Total redactions performed, by engine (all categories combined).
    pub redactions_total: AtomicU64,
    /// Redactions of API keys (sk-, ghp_, ghu_, xoxb-, etc.).
    pub redactions_api_key: AtomicU64,
    /// Redactions of JWT tokens.
    pub redactions_jwt: AtomicU64,
    /// Redactions of Bearer tokens.
    pub redactions_bearer: AtomicU64,
    /// Redactions of AWS credentials.
    pub redactions_aws: AtomicU64,
    /// Redactions of connection strings.
    pub redactions_connection_string: AtomicU64,
    /// Redactions of private keys (PEM).
    pub redactions_private_key: AtomicU64,
    /// Redactions of high-entropy strings.
    pub redactions_entropy: AtomicU64,
    /// Redactions of custom patterns.
    pub redactions_custom: AtomicU64,
    /// Total sanitizations performed.
    pub sanitizations_total: AtomicU64,
    /// Sanitization: zero-width characters stripped.
    pub sanitizations_zero_width: AtomicU64,
    /// Sanitization: bidi overrides replaced.
    pub sanitizations_bidi: AtomicU64,
    /// Sanitization: tag characters stripped.
    pub sanitizations_tag_chars: AtomicU64,
    /// Sanitization: NFKC normalization applied.
    pub sanitizations_nfkc: AtomicU64,
    /// Fail-open events (panic caught, original message sent).
    pub fail_open_events: AtomicU64,
    /// Allowlist matches (redaction skipped due to allowlist).
    pub allowlist_matches: AtomicU64,
}

impl SentinelMetrics {
    const fn new() -> Self {
        Self {
            redactions_total: AtomicU64::new(0),
            redactions_api_key: AtomicU64::new(0),
            redactions_jwt: AtomicU64::new(0),
            redactions_bearer: AtomicU64::new(0),
            redactions_aws: AtomicU64::new(0),
            redactions_connection_string: AtomicU64::new(0),
            redactions_private_key: AtomicU64::new(0),
            redactions_entropy: AtomicU64::new(0),
            redactions_custom: AtomicU64::new(0),
            sanitizations_total: AtomicU64::new(0),
            sanitizations_zero_width: AtomicU64::new(0),
            sanitizations_bidi: AtomicU64::new(0),
            sanitizations_tag_chars: AtomicU64::new(0),
            sanitizations_nfkc: AtomicU64::new(0),
            fail_open_events: AtomicU64::new(0),
            allowlist_matches: AtomicU64::new(0),
        }
    }

    /// Record a redaction event by category.
    pub fn record_redaction(&self, category: &str) {
        self.redactions_total.fetch_add(1, Ordering::Relaxed);

        match category {
            "api_key" => self.redactions_api_key.fetch_add(1, Ordering::Relaxed),
            "jwt" => self.redactions_jwt.fetch_add(1, Ordering::Relaxed),
            "bearer" => self.redactions_bearer.fetch_add(1, Ordering::Relaxed),
            "aws_credential" => self.redactions_aws.fetch_add(1, Ordering::Relaxed),
            "connection_string" => self
                .redactions_connection_string
                .fetch_add(1, Ordering::Relaxed),
            "private_key" => self.redactions_private_key.fetch_add(1, Ordering::Relaxed),
            "entropy" => self.redactions_entropy.fetch_add(1, Ordering::Relaxed),
            _ => self.redactions_custom.fetch_add(1, Ordering::Relaxed),
        };

        tracing::info!(category = category, "sentinel: redacted secret");
    }

    /// Record a sanitization event by category.
    pub fn record_sanitization(&self, category: &str, count: usize) {
        self.sanitizations_total.fetch_add(1, Ordering::Relaxed);

        match category {
            "zero_width" => self
                .sanitizations_zero_width
                .fetch_add(count as u64, Ordering::Relaxed),
            "bidi_override" => self
                .sanitizations_bidi
                .fetch_add(count as u64, Ordering::Relaxed),
            "tag_characters" => self
                .sanitizations_tag_chars
                .fetch_add(count as u64, Ordering::Relaxed),
            "nfkc" => self.sanitizations_nfkc.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }

    /// Record a fail-open event (panic caught).
    pub fn record_fail_open(&self, context: &str) {
        self.fail_open_events.fetch_add(1, Ordering::Relaxed);

        tracing::error!(
            context = context,
            "SENTINEL FAIL-OPEN: redaction/sanitization panicked, original message sent"
        );
    }

    /// Record an allowlist match.
    pub fn record_allowlist_match(&self, _candidate: &str) {
        self.allowlist_matches.fetch_add(1, Ordering::Relaxed);

        tracing::debug!("sentinel: allowlist match prevented redaction");
    }

    /// Get a snapshot of all counters for metrics export.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            redactions_total: self.redactions_total.load(Ordering::Relaxed),
            redactions_api_key: self.redactions_api_key.load(Ordering::Relaxed),
            redactions_jwt: self.redactions_jwt.load(Ordering::Relaxed),
            redactions_bearer: self.redactions_bearer.load(Ordering::Relaxed),
            redactions_aws: self.redactions_aws.load(Ordering::Relaxed),
            redactions_connection_string: self.redactions_connection_string.load(Ordering::Relaxed),
            redactions_private_key: self.redactions_private_key.load(Ordering::Relaxed),
            redactions_entropy: self.redactions_entropy.load(Ordering::Relaxed),
            redactions_custom: self.redactions_custom.load(Ordering::Relaxed),
            sanitizations_total: self.sanitizations_total.load(Ordering::Relaxed),
            sanitizations_zero_width: self.sanitizations_zero_width.load(Ordering::Relaxed),
            sanitizations_bidi: self.sanitizations_bidi.load(Ordering::Relaxed),
            sanitizations_tag_chars: self.sanitizations_tag_chars.load(Ordering::Relaxed),
            sanitizations_nfkc: self.sanitizations_nfkc.load(Ordering::Relaxed),
            fail_open_events: self.fail_open_events.load(Ordering::Relaxed),
            allowlist_matches: self.allowlist_matches.load(Ordering::Relaxed),
        }
    }

    /// Format metrics in Prometheus exposition format.
    pub fn to_prometheus(&self) -> String {
        let s = self.snapshot();
        format!(
            "# HELP sentinel_redactions_total Total secrets redacted\n\
             # TYPE sentinel_redactions_total counter\n\
             sentinel_redactions_total {}\n\
             sentinel_redactions_total{{category=\"api_key\"}} {}\n\
             sentinel_redactions_total{{category=\"jwt\"}} {}\n\
             sentinel_redactions_total{{category=\"bearer\"}} {}\n\
             sentinel_redactions_total{{category=\"aws_credential\"}} {}\n\
             sentinel_redactions_total{{category=\"connection_string\"}} {}\n\
             sentinel_redactions_total{{category=\"private_key\"}} {}\n\
             sentinel_redactions_total{{category=\"entropy\"}} {}\n\
             sentinel_redactions_total{{category=\"custom\"}} {}\n\
             # HELP sentinel_sanitizations_total Total messages sanitized\n\
             # TYPE sentinel_sanitizations_total counter\n\
             sentinel_sanitizations_total {}\n\
             # HELP sentinel_fail_open_total Fail-open events (panic caught)\n\
             # TYPE sentinel_fail_open_total counter\n\
             sentinel_fail_open_total {}\n\
             # HELP sentinel_allowlist_matches_total Allowlist matches\n\
             # TYPE sentinel_allowlist_matches_total counter\n\
             sentinel_allowlist_matches_total {}\n",
            s.redactions_total,
            s.redactions_api_key,
            s.redactions_jwt,
            s.redactions_bearer,
            s.redactions_aws,
            s.redactions_connection_string,
            s.redactions_private_key,
            s.redactions_entropy,
            s.redactions_custom,
            s.sanitizations_total,
            s.fail_open_events,
            s.allowlist_matches,
        )
    }
}

/// Snapshot of all metrics counters at a point in time.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub redactions_total: u64,
    pub redactions_api_key: u64,
    pub redactions_jwt: u64,
    pub redactions_bearer: u64,
    pub redactions_aws: u64,
    pub redactions_connection_string: u64,
    pub redactions_private_key: u64,
    pub redactions_entropy: u64,
    pub redactions_custom: u64,
    pub sanitizations_total: u64,
    pub sanitizations_zero_width: u64,
    pub sanitizations_bidi: u64,
    pub sanitizations_tag_chars: u64,
    pub sanitizations_nfkc: u64,
    pub fail_open_events: u64,
    pub allowlist_matches: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_start_at_zero() {
        let m = SentinelMetrics::new();
        let s = m.snapshot();
        assert_eq!(s.redactions_total, 0);
        assert_eq!(s.sanitizations_total, 0);
        assert_eq!(s.fail_open_events, 0);
    }

    #[test]
    fn record_redaction_increments() {
        let m = SentinelMetrics::new();
        m.record_redaction("api_key");
        m.record_redaction("api_key");
        m.record_redaction("jwt");

        let s = m.snapshot();
        assert_eq!(s.redactions_total, 3);
        assert_eq!(s.redactions_api_key, 2);
        assert_eq!(s.redactions_jwt, 1);
    }

    #[test]
    fn record_sanitization_increments() {
        let m = SentinelMetrics::new();
        m.record_sanitization("zero_width", 5);
        m.record_sanitization("bidi_override", 2);

        let s = m.snapshot();
        assert_eq!(s.sanitizations_total, 2);
        assert_eq!(s.sanitizations_zero_width, 5);
        assert_eq!(s.sanitizations_bidi, 2);
    }

    #[test]
    fn record_fail_open_increments() {
        let m = SentinelMetrics::new();
        m.record_fail_open("channel_send");
        m.record_fail_open("tool_execute");

        let s = m.snapshot();
        assert_eq!(s.fail_open_events, 2);
    }

    #[test]
    fn prometheus_format_output() {
        let m = SentinelMetrics::new();
        m.record_redaction("api_key");
        m.record_redaction("jwt");

        let prom = m.to_prometheus();
        assert!(prom.contains("sentinel_redactions_total 2"));
        assert!(prom.contains("sentinel_redactions_total{category=\"api_key\"} 1"));
        assert!(prom.contains("sentinel_redactions_total{category=\"jwt\"} 1"));
        assert!(prom.contains("sentinel_fail_open_total 0"));
    }

    #[test]
    fn allowlist_match_tracking() {
        let m = SentinelMetrics::new();
        m.record_allowlist_match("sk-example");
        m.record_allowlist_match("AKIAEXAMPLE");

        let s = m.snapshot();
        assert_eq!(s.allowlist_matches, 2);
    }
}
