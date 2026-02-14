use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, warn};

use super::ledger::{AsyncMemoryLedger, ChainStatus, MemoryLedger};
use crate::embeddings::{cosine_similarity, EmbeddingProvider};

/// Simple file-based memory store.
///
/// - Daily notes: `notes/YYYY-MM-DD.md`
/// - Long-term memory: `MEMORY.md`
/// - Tamper-proof ledger: `memory/ledger.*.jsonl`
pub struct MemoryStore {
    workspace: PathBuf,
    ledger: MemoryLedger,
}

impl MemoryStore {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let ledger_dir = workspace.join("memory");
        let ledger = MemoryLedger::new(ledger_dir)?;
        Ok(Self { workspace, ledger })
    }

    /// Get the path to today's daily note.
    pub fn today_path(&self) -> PathBuf {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        self.workspace.join("notes").join(format!("{}.md", date))
    }

    /// Get the path to long-term memory.
    pub fn memory_path(&self) -> PathBuf {
        self.workspace.join("MEMORY.md")
    }

    /// Read today's daily note.
    pub fn today_note(&self) -> Option<String> {
        let path = self.today_path();
        std::fs::read_to_string(&path)
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Read long-term memory.
    pub fn long_term(&self) -> Option<String> {
        let path = self.memory_path();
        std::fs::read_to_string(&path)
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Store a fact in the tamper-proof ledger (append-only, no overwrite).
    pub fn store_fact(&mut self, key: &str, value: &str) -> Result<String> {
        self.ledger
            .append("fact", serde_json::json!({"key": key, "value": value}))
    }

    /// Redact a previous entry by appending a tombstone (no delete).
    pub fn redact(&mut self, seq: u64) -> Result<String> {
        self.ledger.append(
            "tombstone",
            serde_json::json!({"ref_seq": seq, "reason": "redacted"}),
        )
    }

    /// Verify the integrity of the ledger hash chain.
    pub fn verify(&self) -> Result<ChainStatus> {
        self.ledger.verify_chain()
    }

    /// Look up the latest value for a fact key from the ledger.
    pub fn get_fact(&self, key: &str) -> Option<&str> {
        self.ledger.get_latest_fact(key)
    }

    /// Get memory context for the system prompt.
    pub fn get_context(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(mem) = self.long_term() {
            parts.push(format!("## Long-term Memory\n{}", mem));
        }

        if let Some(note) = self.today_note() {
            parts.push(format!("## Today's Notes\n{}", note));
        }

        // Recent daily notes (last 3 days)
        for days_ago in 1..=3 {
            let date = (Utc::now() - chrono::Duration::days(days_ago))
                .format("%Y-%m-%d")
                .to_string();
            let path = self.workspace.join("notes").join(format!("{}.md", date));
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.is_empty() {
                    parts.push(format!("## Notes from {}\n{}", date, content));
                }
            }
        }

        if parts.is_empty() {
            None
        } else {
            debug!("Loaded {} memory sections", parts.len());
            Some(parts.join("\n\n"))
        }
    }
}

/// In-memory vector index entry for semantic search.
struct VectorEntry {
    /// The fact key this embedding corresponds to.
    key: String,
    /// The fact value.
    value: String,
    /// Dense vector embedding of "{key}: {value}".
    embedding: Vec<f32>,
}

/// Async-safe memory store that wraps ledger operations in spawn_blocking.
///
/// Optionally integrates with an [`EmbeddingProvider`] to maintain an
/// in-memory vector index for semantic similarity search over stored facts.
pub struct AsyncMemoryStore {
    workspace: PathBuf,
    ledger: AsyncMemoryLedger,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    vector_index: tokio::sync::Mutex<Vec<VectorEntry>>,
}

impl AsyncMemoryStore {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let ledger_dir = workspace.join("memory");
        let ledger = AsyncMemoryLedger::new(ledger_dir)?;
        Ok(Self {
            workspace,
            ledger,
            embedding_provider: None,
            vector_index: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    /// Attach an embedding provider and rebuild the vector index from
    /// all existing facts in the ledger.
    pub async fn with_embeddings(mut self, provider: Arc<dyn EmbeddingProvider>) -> Result<Self> {
        self.embedding_provider = Some(provider);
        self.rebuild_vector_index().await;
        Ok(self)
    }

    /// Store a fact in the tamper-proof ledger (async-safe).
    ///
    /// If an embedding provider is attached, the fact is also indexed
    /// in the in-memory vector store for semantic search.
    pub async fn store_fact(&self, key: &str, value: &str) -> anyhow::Result<String> {
        let hash = self
            .ledger
            .append("fact", serde_json::json!({"key": key, "value": value}))
            .await?;

        // Index the new fact for semantic search
        if let Some(ref provider) = self.embedding_provider {
            let text = format!("{}: {}", key, value);
            match provider.embed(&[text]).await {
                Ok(mut embeddings) if !embeddings.is_empty() => {
                    let embedding = embeddings.remove(0);
                    let mut index = self.vector_index.lock().await;
                    // Remove any existing entry for this key
                    index.retain(|e| e.key != key);
                    index.push(VectorEntry {
                        key: key.to_string(),
                        value: value.to_string(),
                        embedding,
                    });
                    debug!(key, "fact indexed for semantic search");
                }
                Ok(_) => {
                    warn!(key, "embedding returned empty result");
                }
                Err(e) => {
                    warn!(key, error = %e, "failed to embed fact, skipping index");
                }
            }
        }

        Ok(hash)
    }

    /// Redact a previous entry by appending a tombstone (async-safe).
    pub async fn redact(&self, seq: u64) -> anyhow::Result<String> {
        self.ledger
            .append(
                "tombstone",
                serde_json::json!({"ref_seq": seq, "reason": "redacted"}),
            )
            .await
    }

    /// Verify the integrity of the ledger hash chain (async-safe).
    pub async fn verify(&self) -> anyhow::Result<ChainStatus> {
        self.ledger.verify_chain().await
    }

    /// Look up the latest value for a fact key (async-safe).
    pub async fn get_fact(&self, key: &str) -> Option<String> {
        self.ledger.get_latest_fact(key).await
    }

    /// Search for facts semantically similar to the query.
    ///
    /// Returns up to `top_k` results as `(key, value, similarity_score)` tuples,
    /// sorted by descending similarity. Returns an empty vec if no embedding
    /// provider is attached.
    pub async fn search_similar(&self, query: &str, top_k: usize) -> Vec<(String, String, f32)> {
        let provider = match &self.embedding_provider {
            Some(p) => p,
            None => return Vec::new(),
        };

        let query_embedding = match provider.embed(&[query.to_string()]).await {
            Ok(mut embeddings) if !embeddings.is_empty() => embeddings.remove(0),
            _ => return Vec::new(),
        };

        let index = self.vector_index.lock().await;
        let mut scored: Vec<(String, String, f32)> = index
            .iter()
            .map(|entry| {
                let score = cosine_similarity(&query_embedding, &entry.embedding);
                (entry.key.clone(), entry.value.clone(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Rebuild the in-memory vector index from all facts in the ledger.
    async fn rebuild_vector_index(&self) {
        let provider = match &self.embedding_provider {
            Some(p) => p,
            None => return,
        };

        let facts = self.ledger.get_all_facts().await;
        if facts.is_empty() {
            return;
        }

        let texts: Vec<String> = facts.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();

        match provider.embed(&texts).await {
            Ok(embeddings) => {
                let mut index = self.vector_index.lock().await;
                index.clear();
                for ((key, value), embedding) in facts.into_iter().zip(embeddings) {
                    index.push(VectorEntry {
                        key,
                        value,
                        embedding,
                    });
                }
                debug!(count = index.len(), "vector index rebuilt from ledger");
            }
            Err(e) => {
                warn!(error = %e, "failed to rebuild vector index");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RustyClawError;
    use async_trait::async_trait;
    use tempfile::TempDir;

    struct MockEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(
            &self,
            texts: &[String],
        ) -> std::result::Result<Vec<Vec<f32>>, RustyClawError> {
            // Return a deterministic embedding based on text length.
            // Different texts will get different embeddings.
            Ok(texts
                .iter()
                .map(|t| {
                    let len = t.len() as f32;
                    vec![len / 100.0, (len * 2.0) / 100.0, (len * 3.0) / 100.0]
                })
                .collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn search_similar_with_mock_provider() {
        let tmp = TempDir::new().unwrap();
        let store = AsyncMemoryStore::new(tmp.path().to_path_buf()).unwrap();
        let store = store
            .with_embeddings(Arc::new(MockEmbeddingProvider))
            .await
            .unwrap();

        // Store some facts
        store.store_fact("name", "Alice").await.unwrap();
        store.store_fact("city", "New York").await.unwrap();
        store.store_fact("pet", "cat named Whiskers").await.unwrap();

        // Search for something
        let results = store.search_similar("pet animal", 2).await;
        assert_eq!(results.len(), 2);

        // All results should have similarity scores
        for (_key, _value, score) in &results {
            assert!(*score >= -1.0 && *score <= 1.0);
        }
    }

    #[tokio::test]
    async fn search_similar_without_provider() {
        let tmp = TempDir::new().unwrap();
        let store = AsyncMemoryStore::new(tmp.path().to_path_buf()).unwrap();

        let results = store.search_similar("anything", 5).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn store_fact_updates_vector_index() {
        let tmp = TempDir::new().unwrap();
        let store = AsyncMemoryStore::new(tmp.path().to_path_buf()).unwrap();
        let store = store
            .with_embeddings(Arc::new(MockEmbeddingProvider))
            .await
            .unwrap();

        store.store_fact("color", "blue").await.unwrap();
        let index = store.vector_index.lock().await;
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].key, "color");
        assert_eq!(index[0].value, "blue");
        drop(index);

        // Updating same key should replace
        store.store_fact("color", "red").await.unwrap();
        let index = store.vector_index.lock().await;
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].value, "red");
    }
}
