use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// How a memory was created — affects default confidence and display.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Agent explicitly called memory_store tool
    #[default]
    Explicit,
    /// Auto-saved from user input
    AutoSave,
    /// Extracted by Hermes LLM analysis
    Extracted,
    /// Restored from MEMORY_SNAPSHOT.md
    Hydrated,
}

impl std::fmt::Display for MemorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Explicit => write!(f, "explicit"),
            Self::AutoSave => write!(f, "auto_save"),
            Self::Extracted => write!(f, "extracted"),
            Self::Hydrated => write!(f, "hydrated"),
        }
    }
}

/// A single memory entry
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
    /// Confidence score (0.0–1.0). Higher = more trustworthy.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// How this memory was created.
    #[serde(default)]
    pub source: MemorySource,
    /// Last time this memory was returned by recall().
    #[serde(default)]
    pub last_recalled_at: Option<String>,
}

fn default_confidence() -> f64 {
    1.0
}

impl std::fmt::Debug for MemoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryEntry")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("content", &self.content)
            .field("category", &self.category)
            .field("timestamp", &self.timestamp)
            .field("score", &self.score)
            .field("confidence", &self.confidence)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// Memory categories for organization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    /// Long-term facts, preferences, decisions
    Core,
    /// Daily session logs
    Daily,
    /// Conversation context
    Conversation,
    /// User-defined custom category
    Custom(String),
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(name) => write!(f, "{name}"),
        }
    }
}

/// Core memory trait — implement for any persistence backend
#[async_trait]
pub trait Memory: Send + Sync {
    /// Backend name
    fn name(&self) -> &str;

    /// Store a memory entry, optionally scoped to a session
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Recall memories matching a query (keyword search), optionally scoped to a session
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Get a specific memory by key
    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// List all memory keys, optionally filtered by category and/or session
    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Remove a memory by key
    async fn forget(&self, key: &str) -> anyhow::Result<bool>;

    /// Count total memories
    async fn count(&self) -> anyhow::Result<usize>;

    /// Health check
    async fn health_check(&self) -> bool;

    /// Store with explicit confidence and source metadata.
    /// Default: delegates to store() (backwards compatible — ignores metadata).
    async fn store_with_metadata(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        _confidence: f64,
        _source: MemorySource,
    ) -> anyhow::Result<()> {
        self.store(key, content, category, session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_category_display_outputs_expected_values() {
        assert_eq!(MemoryCategory::Core.to_string(), "core");
        assert_eq!(MemoryCategory::Daily.to_string(), "daily");
        assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
        assert_eq!(
            MemoryCategory::Custom("project_notes".into()).to_string(),
            "project_notes"
        );
    }

    #[test]
    fn memory_category_serde_uses_snake_case() {
        let core = serde_json::to_string(&MemoryCategory::Core).unwrap();
        let daily = serde_json::to_string(&MemoryCategory::Daily).unwrap();
        let conversation = serde_json::to_string(&MemoryCategory::Conversation).unwrap();

        assert_eq!(core, "\"core\"");
        assert_eq!(daily, "\"daily\"");
        assert_eq!(conversation, "\"conversation\"");
    }

    #[test]
    fn memory_entry_roundtrip_preserves_optional_fields() {
        let entry = MemoryEntry {
            id: "id-1".into(),
            key: "favorite_language".into(),
            content: "Rust".into(),
            category: MemoryCategory::Core,
            timestamp: "2026-02-16T00:00:00Z".into(),
            session_id: Some("session-abc".into()),
            score: Some(0.98),
            confidence: 0.85,
            source: MemorySource::Extracted,
            last_recalled_at: Some("2026-02-16T01:00:00Z".into()),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "id-1");
        assert_eq!(parsed.key, "favorite_language");
        assert_eq!(parsed.content, "Rust");
        assert_eq!(parsed.category, MemoryCategory::Core);
        assert_eq!(parsed.session_id.as_deref(), Some("session-abc"));
        assert_eq!(parsed.score, Some(0.98));
        assert!((parsed.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(parsed.source, MemorySource::Extracted);
        assert_eq!(
            parsed.last_recalled_at.as_deref(),
            Some("2026-02-16T01:00:00Z")
        );
    }

    #[test]
    fn memory_entry_defaults_when_fields_missing() {
        // Simulate deserializing an old-format entry without confidence/source
        let json = r#"{
            "id": "id-2",
            "key": "test",
            "content": "hello",
            "category": "core",
            "timestamp": "2026-01-01T00:00:00Z",
            "session_id": null,
            "score": null
        }"#;
        let parsed: MemoryEntry = serde_json::from_str(json).unwrap();
        assert!((parsed.confidence - 1.0).abs() < f64::EPSILON);
        assert_eq!(parsed.source, MemorySource::Explicit);
        assert!(parsed.last_recalled_at.is_none());
    }

    #[test]
    fn memory_source_serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&MemorySource::Explicit).unwrap(),
            "\"explicit\""
        );
        assert_eq!(
            serde_json::to_string(&MemorySource::AutoSave).unwrap(),
            "\"auto_save\""
        );
        assert_eq!(
            serde_json::to_string(&MemorySource::Extracted).unwrap(),
            "\"extracted\""
        );
        assert_eq!(
            serde_json::to_string(&MemorySource::Hydrated).unwrap(),
            "\"hydrated\""
        );

        let parsed: MemorySource = serde_json::from_str("\"extracted\"").unwrap();
        assert_eq!(parsed, MemorySource::Extracted);
    }
}
