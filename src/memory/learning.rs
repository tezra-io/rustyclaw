//! MAPLE Phase 3 — Learning Worker
//!
//! Background task that periodically scans conversation history, calls an LLM
//! extraction prompt, and writes durable `Fact` / `Preference` entries back to
//! memory. Controlled by [`LearningConfig`] and gated behind `enabled = false`
//! by default so users must explicitly opt in.

use crate::config::{Config, LearningConfig};
use crate::memory::{Memory, MemoryCategory, MemoryEntry};
use crate::providers;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::time::{self, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

// ── Key namespaces ────────────────────────────────────────────────────────────

const WATERMARK_KEY: &str = "learning_meta::watermark";
const DAILY_COUNTER_PREFIX: &str = "learning_meta::daily_count";
const LEARNING_META_CAT: &str = "learning_meta";

// Minimum number of conversation characters before triggering extraction.
const MIN_CONTENT_CHARS: usize = 20;
// Minimum LLM confidence score to accept an extraction result.
const MIN_CONFIDENCE: f64 = 0.7;

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the Learning Worker polling loop.
///
/// Creates its own memory and provider instances from `config` so it can be
/// safely spawned inside a `spawn_component_supervisor` closure.
pub async fn run(config: Config) -> Result<()> {
    if !config.learning.enabled {
        info!("Learning Worker disabled by config — sleeping indefinitely");
        crate::health::mark_component_ok("learning");
        loop {
            time::sleep(Duration::from_secs(3600)).await;
        }
    }

    let backend = config.memory.backend.as_str();
    if matches!(backend, "markdown" | "none") {
        info!(
            "Learning Worker: backend '{}' does not support full-history queries — skipping",
            backend
        );
        crate::health::mark_component_ok("learning");
        loop {
            time::sleep(Duration::from_secs(3600)).await;
        }
    }

    let mem: Arc<dyn Memory> = Arc::from(crate::memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);

    let provider_name = config.effective_provider().to_string();
    let model_name = config
        .learning
        .extraction_model
        .clone()
        .unwrap_or_else(|| config.effective_model());

    let provider = providers::create_routed_provider(
        &provider_name,
        config.api_key.as_deref(),
        &config.reliability,
        &config.model_routes,
        &model_name,
    )?;

    let poll_secs = config.learning.poll_interval_secs.max(30);
    let mut interval = time::interval(Duration::from_secs(poll_secs));

    crate::health::mark_component_ok("learning");
    info!(
        "Learning Worker started (poll={}s, model={})",
        poll_secs, model_name
    );

    let worker = LearningWorker {
        mem,
        provider: Arc::from(provider),
        cfg: config.learning.clone(),
        model_name,
    };

    loop {
        interval.tick().await;
        crate::health::mark_component_ok("learning");

        if let Err(e) = worker.run_once().await {
            warn!("Learning Worker cycle error: {e}");
            crate::health::mark_component_error("learning", e.to_string());
        }
    }
}

// ── Worker struct ─────────────────────────────────────────────────────────────

struct LearningWorker {
    mem: Arc<dyn Memory>,
    provider: Arc<dyn crate::providers::Provider>,
    cfg: LearningConfig,
    model_name: String,
}

impl LearningWorker {
    async fn run_once(&self) -> Result<()> {
        // Rate-limit: check daily extraction count.
        if self.daily_count().await >= self.cfg.max_extractions_per_day {
            debug!("Learning Worker: daily extraction limit reached");
            return Ok(());
        }

        // Read watermark.
        let watermark_ts = self.read_watermark().await;

        // Collect conversation entries since watermark.
        let entries = self.collect_entries(watermark_ts.as_ref()).await?;
        if entries.is_empty() {
            debug!("Learning Worker: no new conversation entries since watermark");
            return Ok(());
        }

        info!(
            "Learning Worker: {} new entries, running extraction",
            entries.len()
        );

        // Build transcript and call LLM.
        let transcript = build_transcript(&entries);
        let raw = self
            .provider
            .chat_with_system(Some(EXTRACTION_SYSTEM_PROMPT), &transcript, &self.model_name, 0.2)
            .await?;

        // Parse and store results.
        let parsed = parse_extraction(&raw);
        let stored = self.store_extracted(parsed).await?;

        // Advance watermark.
        let new_watermark = Utc::now();
        self.write_watermark(new_watermark).await;

        // Bump daily counter.
        self.increment_daily_count().await;

        info!("Learning Worker: stored {} fact/preference entries", stored);
        Ok(())
    }

    // ── Watermark ──────────────────────────────────────────────────────────

    async fn read_watermark(&self) -> Option<DateTime<Utc>> {
        let entry = self.mem.get(WATERMARK_KEY).await.ok()??;
        let ts: serde_json::Value = serde_json::from_str(&entry.content).ok()?;
        let s = ts.get("last_processed_ts")?.as_str()?;
        DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
    }

    async fn write_watermark(&self, ts: DateTime<Utc>) {
        let payload = serde_json::json!({
            "last_processed_ts": ts.to_rfc3339(),
            "last_l1_run_ts": Utc::now().to_rfc3339(),
        })
        .to_string();
        let cat = MemoryCategory::Custom(LEARNING_META_CAT.into());
        let _ = self.mem.store(WATERMARK_KEY, &payload, cat).await;
    }

    // ── Daily counter ──────────────────────────────────────────────────────

    fn daily_counter_key() -> String {
        let today = Utc::now().format("%Y-%m-%d");
        format!("{DAILY_COUNTER_PREFIX}::{today}")
    }

    async fn daily_count(&self) -> u32 {
        let key = Self::daily_counter_key();
        match self.mem.get(&key).await {
            Ok(Some(e)) => e.content.parse::<u32>().unwrap_or(0),
            _ => 0,
        }
    }

    async fn increment_daily_count(&self) {
        let key = Self::daily_counter_key();
        let count = self.daily_count().await + 1;
        let cat = MemoryCategory::Custom(LEARNING_META_CAT.into());
        let _ = self.mem.store(&key, &count.to_string(), cat).await;
    }

    // ── Entry collection ───────────────────────────────────────────────────

    async fn collect_entries(
        &self,
        since: Option<&DateTime<Utc>>,
    ) -> Result<Vec<MemoryEntry>> {
        let mut all = self
            .mem
            .list(Some(&MemoryCategory::Conversation))
            .await?;

        // Filter by watermark timestamp if present.
        if let Some(wm) = since {
            all.retain(|e| {
                DateTime::parse_from_rfc3339(&e.timestamp)
                    .map(|dt| dt.with_timezone(&Utc) > *wm)
                    .unwrap_or(false)
            });
        }

        // Drop session-end sentinel keys.
        all.retain(|e| !e.key.starts_with("learning_meta::session_end::"));

        // Drop very short / noisy entries.
        all.retain(|e| e.content.trim().len() >= MIN_CONTENT_CHARS);

        // Respect batch size cap.
        if all.len() > self.cfg.max_conversation_entries_per_batch {
            all.truncate(self.cfg.max_conversation_entries_per_batch);
        }

        Ok(all)
    }

    // ── Storage ────────────────────────────────────────────────────────────

    async fn store_extracted(&self, extracted: ExtractionResult) -> Result<usize> {
        let mut count = 0_usize;

        for fact in extracted.facts {
            if fact.confidence < MIN_CONFIDENCE {
                continue;
            }
            let key = deterministic_key("fact", &fact.text);
            let _ = self
                .mem
                .store(&key, &fact.text, MemoryCategory::Fact)
                .await;
            count += 1;
        }

        for pref in extracted.preferences {
            if pref.confidence < MIN_CONFIDENCE {
                continue;
            }
            // Use stable dimension keys for known preference types.
            let key = stable_preference_key(&pref);
            let _ = self
                .mem
                .store(&key, &pref.text, MemoryCategory::Preference)
                .await;
            count += 1;
        }

        Ok(count)
    }
}

// ── Stable preference key heuristic ──────────────────────────────────────────

fn stable_preference_key(pref: &ExtractedPreference) -> String {
    let text_lower = pref.text.to_lowercase();
    if text_lower.contains("verbose") || text_lower.contains("terse") || text_lower.contains("concise") || text_lower.contains("brief") {
        return "preference::verbosity".to_string();
    }
    if text_lower.contains("markdown") || text_lower.contains("format") || text_lower.contains("plain text") {
        return "preference::format".to_string();
    }
    if text_lower.contains("code block") || text_lower.contains("snippet") || text_lower.contains("code presentation") {
        return "preference::code_presentation".to_string();
    }
    deterministic_key("preference", &pref.text)
}

// ── Deterministic key from content hash ──────────────────────────────────────

fn deterministic_key(prefix: &str, text: &str) -> String {
    let canonical = text.trim().to_lowercase();
    let canonical = canonical.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hash = hasher.finalize();
    let hex16 = hash.iter().take(8).fold(String::with_capacity(16), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!("{prefix}::{hex16}")
}

// ── Transcript builder ────────────────────────────────────────────────────────

fn build_transcript(entries: &[MemoryEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("USER: {}", e.content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Extraction prompt ─────────────────────────────────────────────────────────

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You extract durable user facts and interaction preferences from chat logs.

Only use USER-authored content.
Ignore assistant text, tool outputs, and temporary task instructions.
If uncertain, omit.

Return JSON only, no markdown:
{
  "facts": [
    {"text": "...", "confidence": 0.0}
  ],
  "preferences": [
    {"text": "...", "kind": "explicit", "confidence": 0.0}
  ]
}

Rules:
- confidence must be 0.0 to 1.0
- kind must be "explicit" or "implicit"
- max 8 facts and 8 preferences
- if none: {"facts":[],"preferences":[]}"#;

// ── Extraction result types ───────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ExtractionResult {
    facts: Vec<ExtractedFact>,
    preferences: Vec<ExtractedPreference>,
}

#[derive(Debug)]
struct ExtractedFact {
    text: String,
    confidence: f64,
}

#[derive(Debug)]
struct ExtractedPreference {
    text: String,
    confidence: f64,
}

// ── JSON response parser ──────────────────────────────────────────────────────

fn parse_extraction(raw: &str) -> ExtractionResult {
    // Strip any surrounding markdown code fences.
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let Ok(val) = serde_json::from_str::<serde_json::Value>(cleaned) else {
        warn!("Learning Worker: failed to parse LLM extraction response as JSON");
        return ExtractionResult::default();
    };

    let mut result = ExtractionResult::default();

    if let Some(facts) = val.get("facts").and_then(|v| v.as_array()) {
        for item in facts.iter().take(8) {
            let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            let confidence = item
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let text = sanitize(text);
            if text.is_empty() {
                continue;
            }
            result.facts.push(ExtractedFact { text, confidence });
        }
    }

    if let Some(prefs) = val.get("preferences").and_then(|v| v.as_array()) {
        for item in prefs.iter().take(8) {
            let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            let confidence = item
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let text = sanitize(text);
            if text.is_empty() {
                continue;
            }
            result
                .preferences
                .push(ExtractedPreference { text, confidence });
        }
    }

    result
}

/// Remove control characters and non-printable chars.
fn sanitize(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Write a session-end sentinel into `learning_meta` so the worker can detect
/// session boundaries in the CLI one-shot flow.
pub async fn write_session_end_sentinel(mem: &dyn Memory) {
    let key = format!("learning_meta::session_end::{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "ended_at": Utc::now().to_rfc3339()
    })
    .to_string();
    let _ = mem
        .store(&key, &payload, MemoryCategory::Custom(LEARNING_META_CAT.into()))
        .await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_key_is_stable() {
        let k1 = deterministic_key("fact", "  User is a software engineer  ");
        let k2 = deterministic_key("fact", "user is a software engineer");
        assert_eq!(k1, k2);
        assert!(k1.starts_with("fact::"));
        assert_eq!(k1.len(), "fact::".len() + 16);
    }

    #[test]
    fn deterministic_key_differs_for_different_text() {
        let k1 = deterministic_key("fact", "user likes Rust");
        let k2 = deterministic_key("fact", "user likes Python");
        assert_ne!(k1, k2);
    }

    #[test]
    fn parse_extraction_valid_json() {
        let raw = r#"{"facts":[{"text":"User is a Rust developer","confidence":0.9}],"preferences":[{"text":"prefers terse responses","kind":"explicit","confidence":0.85}]}"#;
        let result = parse_extraction(raw);
        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.preferences.len(), 1);
        assert!((result.facts[0].confidence - 0.9).abs() < 0.001);
        assert!((result.preferences[0].confidence - 0.85).abs() < 0.001);
    }

    #[test]
    fn parse_extraction_strips_markdown_fences() {
        let raw = "```json\n{\"facts\":[],\"preferences\":[]}\n```";
        let result = parse_extraction(raw);
        assert!(result.facts.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn parse_extraction_invalid_json_returns_default() {
        let result = parse_extraction("not json at all");
        assert!(result.facts.is_empty());
        assert!(result.preferences.is_empty());
    }

    #[test]
    fn parse_extraction_clamps_confidence() {
        let raw = r#"{"facts":[{"text":"some fact","confidence":1.5}],"preferences":[]}"#;
        let result = parse_extraction(raw);
        assert_eq!(result.facts.len(), 1);
        assert!((result.facts[0].confidence - 1.0).abs() < 0.001);
    }

    #[test]
    fn parse_extraction_caps_at_8_entries() {
        let facts: Vec<_> = (0..12)
            .map(|i| serde_json::json!({"text": format!("fact {i}"), "confidence": 0.9}))
            .collect();
        let raw = serde_json::json!({"facts": facts, "preferences": []}).to_string();
        let result = parse_extraction(&raw);
        assert_eq!(result.facts.len(), 8);
    }

    #[test]
    fn stable_preference_key_verbosity() {
        let pref = ExtractedPreference {
            text: "User prefers terse responses".into(),
            confidence: 0.9,
        };
        assert_eq!(stable_preference_key(&pref), "preference::verbosity");
    }

    #[test]
    fn stable_preference_key_unknown_uses_hash() {
        let pref = ExtractedPreference {
            text: "User likes dark mode".into(),
            confidence: 0.9,
        };
        let key = stable_preference_key(&pref);
        assert!(key.starts_with("preference::"));
        assert_ne!(key, "preference::verbosity");
        assert_ne!(key, "preference::format");
    }

    #[test]
    fn build_transcript_formats_entries() {
        let entries = vec![
            MemoryEntry {
                id: "1".into(),
                key: "conv_1".into(),
                content: "I am a developer".into(),
                category: MemoryCategory::Conversation,
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            MemoryEntry {
                id: "2".into(),
                key: "conv_2".into(),
                content: "I use Rust".into(),
                category: MemoryCategory::Conversation,
                timestamp: "2026-01-01T00:01:00Z".into(),
                session_id: None,
                score: None,
            },
        ];
        let t = build_transcript(&entries);
        assert!(t.contains("USER: I am a developer"));
        assert!(t.contains("USER: I use Rust"));
    }

    #[test]
    fn sanitize_removes_control_chars() {
        let dirty = "hello\x00world\x1btest";
        let clean = sanitize(dirty);
        assert_eq!(clean, "helloworldtest");
    }

    #[tokio::test]
    async fn write_session_end_sentinel_stores_to_sqlite() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem_cfg = crate::config::MemoryConfig {
            backend: "sqlite".into(),
            ..crate::config::MemoryConfig::default()
        };
        let Ok(mem) = crate::memory::create_memory(&mem_cfg, tmp.path(), None) else {
            return;
        };

        write_session_end_sentinel(mem.as_ref()).await;

        let entries = mem
            .list(Some(&MemoryCategory::Custom("learning_meta".into())))
            .await
            .unwrap_or_default();
        assert!(
            entries
                .iter()
                .any(|e| e.key.starts_with("learning_meta::session_end::")),
            "sentinel key should be written"
        );
    }
}
