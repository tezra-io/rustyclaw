use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread-safe registry mapping agent names to their advertised capabilities.
///
/// Used by `AgentCoordinator` to route tasks to agents based on what they can do
/// rather than by hardcoded name. Capability strings are free-form (e.g.
/// `"web_search"`, `"code_review"`, `"summarization"`).
pub struct CapabilityRegistry {
    /// agent_name → set of capability strings
    inner: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an agent with its capabilities. Replaces any prior registration.
    pub async fn register(&self, agent_name: &str, capabilities: Vec<String>) {
        self.inner
            .write()
            .await
            .insert(agent_name.to_string(), capabilities.into_iter().collect());
    }

    /// Unregister an agent (e.g., on shutdown).
    pub async fn unregister(&self, agent_name: &str) {
        self.inner.write().await.remove(agent_name);
    }

    /// Return names of agents that advertise the given capability.
    pub async fn agents_for_capability(&self, capability: &str) -> Vec<String> {
        self.inner
            .read()
            .await
            .iter()
            .filter_map(|(name, caps)| {
                if caps.contains(capability) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check whether a specific agent has a specific capability registered.
    pub async fn has_capability(&self, agent_name: &str, capability: &str) -> bool {
        self.inner
            .read()
            .await
            .get(agent_name)
            .map(|caps| caps.contains(capability))
            .unwrap_or(false)
    }

    /// Return all registered agent names.
    pub async fn registered_agents(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_lookup_capability() {
        let registry = CapabilityRegistry::new();
        registry
            .register("agent-a", vec!["web_search".into(), "summarization".into()])
            .await;

        let agents = registry.agents_for_capability("web_search").await;
        assert!(agents.contains(&"agent-a".to_string()));
    }

    #[tokio::test]
    async fn lookup_unknown_capability_returns_empty() {
        let registry = CapabilityRegistry::new();
        registry.register("agent-a", vec!["code_review".into()]).await;
        let agents = registry.agents_for_capability("nonexistent").await;
        assert!(agents.is_empty());
    }

    #[tokio::test]
    async fn unregister_removes_agent() {
        let registry = CapabilityRegistry::new();
        registry
            .register("agent-a", vec!["web_search".into()])
            .await;
        registry.unregister("agent-a").await;
        let agents = registry.agents_for_capability("web_search").await;
        assert!(agents.is_empty());
    }

    #[tokio::test]
    async fn has_capability_true_when_registered() {
        let registry = CapabilityRegistry::new();
        registry
            .register("agent-a", vec!["summarization".into()])
            .await;
        assert!(registry.has_capability("agent-a", "summarization").await);
    }

    #[tokio::test]
    async fn has_capability_false_when_not_registered() {
        let registry = CapabilityRegistry::new();
        registry.register("agent-a", vec!["summarization".into()]).await;
        assert!(!registry.has_capability("agent-a", "nonexistent").await);
        assert!(!registry.has_capability("unknown-agent", "summarization").await);
    }

    #[tokio::test]
    async fn multiple_agents_for_same_capability() {
        let registry = CapabilityRegistry::new();
        registry
            .register("agent-a", vec!["web_search".into()])
            .await;
        registry
            .register("agent-b", vec!["web_search".into(), "code_review".into()])
            .await;

        let agents = registry.agents_for_capability("web_search").await;
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&"agent-a".to_string()));
        assert!(agents.contains(&"agent-b".to_string()));
    }

    #[tokio::test]
    async fn register_replaces_prior_capabilities() {
        let registry = CapabilityRegistry::new();
        registry
            .register("agent-a", vec!["web_search".into()])
            .await;
        registry
            .register("agent-a", vec!["code_review".into()])
            .await;

        // Old capability gone
        assert!(!registry.has_capability("agent-a", "web_search").await);
        // New one present
        assert!(registry.has_capability("agent-a", "code_review").await);
    }

    #[tokio::test]
    async fn registered_agents_lists_all() {
        let registry = CapabilityRegistry::new();
        registry.register("agent-a", vec!["x".into()]).await;
        registry.register("agent-b", vec!["y".into()]).await;
        let mut agents = registry.registered_agents().await;
        agents.sort();
        assert_eq!(agents, vec!["agent-a", "agent-b"]);
    }

    #[tokio::test]
    async fn empty_registry_returns_empty() {
        let registry = CapabilityRegistry::new();
        assert!(registry.agents_for_capability("anything").await.is_empty());
        assert!(!registry.has_capability("nobody", "anything").await);
        assert!(registry.registered_agents().await.is_empty());
    }

    #[tokio::test]
    async fn default_creates_empty_registry() {
        let registry = CapabilityRegistry::default();
        assert!(registry.registered_agents().await.is_empty());
    }
}
