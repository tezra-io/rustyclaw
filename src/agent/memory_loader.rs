use crate::memory::Memory;
use async_trait::async_trait;

#[async_trait]
pub trait MemoryLoader: Send + Sync {
    async fn load_context(&self, memory: &dyn Memory, user_message: &str)
        -> anyhow::Result<String>;
}

pub struct DefaultMemoryLoader {
    limit: usize,
    min_relevance_score: f64,
}

impl Default for DefaultMemoryLoader {
    fn default() -> Self {
        Self {
            limit: 5,
            min_relevance_score: 0.4,
        }
    }
}

impl DefaultMemoryLoader {
    pub fn new(limit: usize, min_relevance_score: f64) -> Self {
        Self {
            limit: limit.max(1),
            min_relevance_score,
        }
    }
}

#[async_trait]
impl MemoryLoader for DefaultMemoryLoader {
    /// Build category-structured memory context for persistent agents.
    ///
    /// Prepends UserModel entries unconditionally (session-start injection),
    /// then appends query-relevant memories ordered by category:
    ///   [User preferences] → [Known facts about user] → [Relevant memory]
    ///
    /// The `limit` field is not used here — the personalization engine fetches
    /// 10 entries via `recall()` and enforces its own char budget.
    async fn load_context(
        &self,
        memory: &dyn Memory,
        user_message: &str,
    ) -> anyhow::Result<String> {
        let user_model_ctx =
            crate::agent::personalization::build_user_model_context(memory).await;
        let mem_ctx = crate::agent::personalization::build_context(
            memory,
            user_message,
            self.min_relevance_score,
        )
        .await;
        Ok(format!("{user_model_ctx}{mem_ctx}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};

    struct MockMemory;

    #[async_trait]
    impl Memory for MockMemory {
        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(&self, _query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
            if limit == 0 {
                return Ok(vec![]);
            }
            Ok(vec![MemoryEntry {
                id: "1".into(),
                key: "k".into(),
                content: "v".into(),
                category: MemoryCategory::Conversation,
                timestamp: "now".into(),
                session_id: None,
                score: None,
            }])
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn default_loader_formats_context_with_relevant_memory_section() {
        let loader = DefaultMemoryLoader::default();
        let context = loader.load_context(&MockMemory, "hello").await.unwrap();
        // Conversation entries appear in the [Relevant memory] section
        assert!(context.contains("[Relevant memory]"));
        assert!(context.contains("- k: v"));
    }

    #[tokio::test]
    async fn default_loader_empty_when_no_memories() {
        struct EmptyMemory;

        #[async_trait]
        impl Memory for EmptyMemory {
            fn name(&self) -> &str {
                "empty"
            }
            async fn store(&self, _: &str, _: &str, _: MemoryCategory) -> anyhow::Result<()> {
                Ok(())
            }
            async fn recall(&self, _: &str, _: usize) -> anyhow::Result<Vec<MemoryEntry>> {
                Ok(vec![])
            }
            async fn get(&self, _: &str) -> anyhow::Result<Option<MemoryEntry>> {
                Ok(None)
            }
            async fn list(&self, _: Option<&MemoryCategory>) -> anyhow::Result<Vec<MemoryEntry>> {
                Ok(vec![])
            }
            async fn forget(&self, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
            async fn count(&self) -> anyhow::Result<usize> {
                Ok(0)
            }
            async fn health_check(&self) -> bool {
                true
            }
        }

        let loader = DefaultMemoryLoader::default();
        let context = loader.load_context(&EmptyMemory, "hello").await.unwrap();
        assert!(context.is_empty());
    }

    #[tokio::test]
    async fn new_loader_filters_low_relevance_entries() {
        struct ScoredMemory;

        #[async_trait]
        impl Memory for ScoredMemory {
            fn name(&self) -> &str {
                "scored"
            }
            async fn store(&self, _: &str, _: &str, _: MemoryCategory) -> anyhow::Result<()> {
                Ok(())
            }
            async fn recall(
                &self,
                _: &str,
                _limit: usize,
            ) -> anyhow::Result<Vec<MemoryEntry>> {
                Ok(vec![
                    MemoryEntry {
                        id: "1".into(),
                        key: "high".into(),
                        content: "relevant content".into(),
                        category: MemoryCategory::Conversation,
                        timestamp: "now".into(),
                        session_id: None,
                        score: Some(0.8),
                    },
                    MemoryEntry {
                        id: "2".into(),
                        key: "low".into(),
                        content: "unrelated content".into(),
                        category: MemoryCategory::Conversation,
                        timestamp: "now".into(),
                        session_id: None,
                        score: Some(0.1),
                    },
                ])
            }
            async fn get(&self, _: &str) -> anyhow::Result<Option<MemoryEntry>> {
                Ok(None)
            }
            async fn list(
                &self,
                _: Option<&MemoryCategory>,
            ) -> anyhow::Result<Vec<MemoryEntry>> {
                Ok(vec![])
            }
            async fn forget(&self, _: &str) -> anyhow::Result<bool> {
                Ok(false)
            }
            async fn count(&self) -> anyhow::Result<usize> {
                Ok(0)
            }
            async fn health_check(&self) -> bool {
                true
            }
        }

        // With threshold 0.4, low-relevance entry (score 0.1) should be filtered out.
        let loader = DefaultMemoryLoader::new(5, 0.4);
        let context = loader.load_context(&ScoredMemory, "hello").await.unwrap();
        assert!(context.contains("relevant content"));
        assert!(!context.contains("unrelated content"));
    }
}
