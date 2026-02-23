use super::traits::{Memory, MemoryCategory, MemoryEntry};
use async_trait::async_trait;
use std::sync::Arc;

/// Composite memory: reads from both workspace + agent memory,
/// writes only to agent-specific memory.
///
/// Used for `MemoryIsolation::SharedRead` agents.
pub struct CompositeMemory {
    /// Read-only source (workspace memory)
    read_source: Arc<dyn Memory>,
    /// Read-write target (agent-specific memory)
    write_target: Arc<dyn Memory>,
}

impl CompositeMemory {
    pub fn new(read_source: Arc<dyn Memory>, write_target: Arc<dyn Memory>) -> Self {
        Self {
            read_source,
            write_target,
        }
    }
}

#[async_trait]
impl Memory for CompositeMemory {
    fn name(&self) -> &str {
        "composite"
    }

    /// Store only to agent-specific memory (never write to shared source)
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
    ) -> anyhow::Result<()> {
        self.write_target.store(key, content, category).await
    }

    /// Recall from both sources, merge and deduplicate by key.
    /// Agent-specific entries take priority over workspace entries with same key.
    async fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let (agent_results, workspace_results) = tokio::join!(
            self.write_target.recall(query, limit),
            self.read_source.recall(query, limit),
        );

        let mut results = agent_results.unwrap_or_default();
        let agent_keys: std::collections::HashSet<String> =
            results.iter().map(|e| e.key.clone()).collect();

        // Add workspace results that don't conflict with agent results
        for entry in workspace_results.unwrap_or_default() {
            if !agent_keys.contains(&entry.key) {
                results.push(entry);
            }
        }

        // Sort by score (highest first) if scores exist, otherwise keep order
        results.sort_by(|a, b| {
            b.score
                .unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(limit);
        Ok(results)
    }

    /// Get from agent memory first, fall back to workspace
    async fn get(&self, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        if let Some(entry) = self.write_target.get(key).await? {
            return Ok(Some(entry));
        }
        self.read_source.get(key).await
    }

    /// List from agent memory only (listing workspace would be too noisy)
    async fn list(&self, category: Option<&MemoryCategory>) -> anyhow::Result<Vec<MemoryEntry>> {
        self.write_target.list(category).await
    }

    /// Forget only from agent memory (cannot delete from read-only source)
    async fn forget(&self, key: &str) -> anyhow::Result<bool> {
        self.write_target.forget(key).await
    }

    /// Count agent memory entries only
    async fn count(&self) -> anyhow::Result<usize> {
        self.write_target.count().await
    }

    async fn health_check(&self) -> bool {
        self.write_target.health_check().await && self.read_source.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ephemeral::EphemeralMemory;

    fn make_composite() -> (CompositeMemory, Arc<EphemeralMemory>, Arc<EphemeralMemory>) {
        let workspace = Arc::new(EphemeralMemory::new());
        let agent = Arc::new(EphemeralMemory::new());
        let composite = CompositeMemory::new(workspace.clone(), agent.clone());
        (composite, workspace, agent)
    }

    #[tokio::test]
    async fn store_goes_to_agent_only() {
        let (composite, workspace, agent) = make_composite();
        composite
            .store("key", "value", MemoryCategory::Core)
            .await
            .unwrap();
        assert!(agent.get("key").await.unwrap().is_some());
        assert!(workspace.get("key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn recall_merges_both_sources() {
        let (composite, workspace, agent) = make_composite();
        workspace
            .store("ws_key", "workspace data", MemoryCategory::Core)
            .await
            .unwrap();
        agent
            .store("ag_key", "agent data", MemoryCategory::Core)
            .await
            .unwrap();
        let results = composite.recall("data", 10).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn recall_agent_overrides_workspace() {
        let (composite, workspace, agent) = make_composite();
        workspace
            .store("shared", "old value", MemoryCategory::Core)
            .await
            .unwrap();
        agent
            .store("shared", "new value", MemoryCategory::Core)
            .await
            .unwrap();
        let results = composite.recall("shared", 10).await.unwrap();
        // Should deduplicate — agent version wins
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "new value");
    }

    #[tokio::test]
    async fn forget_only_affects_agent() {
        let (composite, workspace, _agent) = make_composite();
        workspace
            .store("immutable", "can't delete", MemoryCategory::Core)
            .await
            .unwrap();
        let removed = composite.forget("immutable").await.unwrap();
        assert!(!removed); // Can't forget from read-only source
    }

    #[tokio::test]
    async fn get_falls_back_to_workspace() {
        let (composite, workspace, _agent) = make_composite();
        workspace
            .store("only_ws", "from workspace", MemoryCategory::Core)
            .await
            .unwrap();
        let entry = composite.get("only_ws").await.unwrap().unwrap();
        assert_eq!(entry.content, "from workspace");
    }

    #[tokio::test]
    async fn get_prefers_agent() {
        let (composite, workspace, agent) = make_composite();
        workspace
            .store("both", "ws version", MemoryCategory::Core)
            .await
            .unwrap();
        agent
            .store("both", "agent version", MemoryCategory::Core)
            .await
            .unwrap();
        let entry = composite.get("both").await.unwrap().unwrap();
        assert_eq!(entry.content, "agent version");
    }

    #[tokio::test]
    async fn count_is_agent_only() {
        let (composite, workspace, agent) = make_composite();
        workspace
            .store("ws", "1", MemoryCategory::Core)
            .await
            .unwrap();
        agent.store("ag", "2", MemoryCategory::Core).await.unwrap();
        assert_eq!(composite.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn list_is_agent_only() {
        let (composite, workspace, agent) = make_composite();
        workspace
            .store("ws", "1", MemoryCategory::Core)
            .await
            .unwrap();
        agent.store("ag", "2", MemoryCategory::Core).await.unwrap();
        let entries = composite.list(None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "ag");
    }

    #[tokio::test]
    async fn health_check_both_healthy() {
        let (composite, _, _) = make_composite();
        assert!(composite.health_check().await);
    }
}
