use super::traits::{Memory, MemoryCategory, MemoryEntry};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// In-memory storage for ephemeral agents. Dies when the agent exits.
/// No persistence, no SQLite, no files. Just a HashMap.
pub struct EphemeralMemory {
    entries: Mutex<HashMap<String, MemoryEntry>>,
}

impl EphemeralMemory {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for EphemeralMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Memory for EphemeralMemory {
    fn name(&self) -> &str {
        "ephemeral"
    }

    async fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
    ) -> anyhow::Result<()> {
        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            key: key.to_string(),
            content: content.to_string(),
            category,
            timestamp: Utc::now().to_rfc3339(),
            session_id: None,
            score: None,
        };
        self.entries
            .lock()
            .expect("ephemeral memory lock poisoned")
            .insert(key.to_string(), entry);
        Ok(())
    }

    async fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let entries = self.entries.lock().expect("ephemeral memory lock poisoned");
        let query_lower = query.to_lowercase();
        let mut results: Vec<MemoryEntry> = entries
            .values()
            .filter(|e| {
                e.key.to_lowercase().contains(&query_lower)
                    || e.content.to_lowercase().contains(&query_lower)
            })
            .cloned()
            .collect();
        results.truncate(limit);
        Ok(results)
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral memory lock poisoned")
            .get(key)
            .cloned())
    }

    async fn list(
        &self,
        category: Option<&MemoryCategory>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let entries = self.entries.lock().expect("ephemeral memory lock poisoned");
        Ok(entries
            .values()
            .filter(|e| category.map_or(true, |c| c == &e.category))
            .cloned()
            .collect())
    }

    async fn forget(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral memory lock poisoned")
            .remove(key)
            .is_some())
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(self
            .entries
            .lock()
            .expect("ephemeral memory lock poisoned")
            .len())
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn store_and_recall() {
        let mem = EphemeralMemory::new();
        mem.store("test", "hello world", MemoryCategory::Core)
            .await
            .unwrap();
        let results = mem.recall("hello", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello world");
    }

    #[tokio::test]
    async fn recall_case_insensitive() {
        let mem = EphemeralMemory::new();
        mem.store("test", "Hello World", MemoryCategory::Core)
            .await
            .unwrap();
        let results = mem.recall("hello", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn recall_no_match() {
        let mem = EphemeralMemory::new();
        mem.store("test", "hello", MemoryCategory::Core)
            .await
            .unwrap();
        let results = mem.recall("xyz", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn recall_respects_limit() {
        let mem = EphemeralMemory::new();
        for i in 0..10 {
            mem.store(&format!("key-{i}"), &format!("data {i}"), MemoryCategory::Core)
                .await
                .unwrap();
        }
        let results = mem.recall("data", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn get_existing() {
        let mem = EphemeralMemory::new();
        mem.store("k", "v", MemoryCategory::Core).await.unwrap();
        let entry = mem.get("k").await.unwrap().unwrap();
        assert_eq!(entry.content, "v");
    }

    #[tokio::test]
    async fn get_nonexistent() {
        let mem = EphemeralMemory::new();
        assert!(mem.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn forget_removes_entry() {
        let mem = EphemeralMemory::new();
        mem.store("k", "v", MemoryCategory::Core).await.unwrap();
        assert!(mem.forget("k").await.unwrap());
        assert!(mem.get("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn forget_nonexistent() {
        let mem = EphemeralMemory::new();
        assert!(!mem.forget("nope").await.unwrap());
    }

    #[tokio::test]
    async fn count() {
        let mem = EphemeralMemory::new();
        assert_eq!(mem.count().await.unwrap(), 0);
        mem.store("a", "1", MemoryCategory::Core).await.unwrap();
        mem.store("b", "2", MemoryCategory::Core).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn list_with_category_filter() {
        let mem = EphemeralMemory::new();
        mem.store("a", "1", MemoryCategory::Core).await.unwrap();
        mem.store("b", "2", MemoryCategory::Daily).await.unwrap();
        let core = mem.list(Some(&MemoryCategory::Core)).await.unwrap();
        assert_eq!(core.len(), 1);
        assert_eq!(core[0].key, "a");
    }

    #[tokio::test]
    async fn list_all() {
        let mem = EphemeralMemory::new();
        mem.store("a", "1", MemoryCategory::Core).await.unwrap();
        mem.store("b", "2", MemoryCategory::Daily).await.unwrap();
        let all = mem.list(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn health_check() {
        let mem = EphemeralMemory::new();
        assert!(mem.health_check().await);
    }

    #[tokio::test]
    async fn store_overwrites_same_key() {
        let mem = EphemeralMemory::new();
        mem.store("k", "old", MemoryCategory::Core).await.unwrap();
        mem.store("k", "new", MemoryCategory::Core).await.unwrap();
        let entry = mem.get("k").await.unwrap().unwrap();
        assert_eq!(entry.content, "new");
        assert_eq!(mem.count().await.unwrap(), 1);
    }
}
