use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

const PERSISTENCE_FILE: &str = "session_bridge_bindings.json";

/// Key for the binding table: (channel_name, sender_id).
type BindingKey = (String, String);

/// A single session binding: maps a (channel, sender) pair to a spawned agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBinding {
    pub channel: String,
    pub sender: String,
    pub agent_name: String,
    pub working_dir: PathBuf,
    pub bound_at: DateTime<Utc>,
    /// Marked true on startup — the child process from a previous run is gone.
    #[serde(default)]
    pub stale: bool,
}

/// Thread-safe binding table with disk persistence.
///
/// Keyed on `(channel, sender)`. Persists as JSON in the config directory
/// alongside `daemon_state.json`.
#[derive(Debug)]
pub struct BindingTable {
    inner: Arc<RwLock<HashMap<BindingKey, SessionBinding>>>,
    persistence_path: PathBuf,
}

impl BindingTable {
    /// Load from disk (or start empty). All loaded entries are marked stale
    /// because child processes from a previous daemon run are gone.
    pub async fn load(config_dir: &Path) -> Result<Self> {
        let persistence_path = config_dir.join(PERSISTENCE_FILE);
        let mut entries: HashMap<BindingKey, SessionBinding> = if persistence_path.exists() {
            let data = tokio::fs::read_to_string(&persistence_path)
                .await
                .with_context(|| {
                    format!(
                        "reading session bridge bindings from {}",
                        persistence_path.display()
                    )
                })?;
            let bindings: Vec<SessionBinding> = serde_json::from_str(&data).with_context(|| {
                format!(
                    "parsing session bridge bindings from {}; \
                         file may be corrupt — delete or fix it to recover",
                    persistence_path.display()
                )
            })?;
            bindings
                .into_iter()
                .map(|b| ((b.channel.clone(), b.sender.clone()), b))
                .collect()
        } else {
            HashMap::new()
        };

        // Mark all entries as stale — child processes are gone after restart
        for binding in entries.values_mut() {
            binding.stale = true;
        }

        let table = Self {
            inner: Arc::new(RwLock::new(entries)),
            persistence_path,
        };

        // Persist the stale flags
        if !table.inner.read().await.is_empty() {
            table.save_inner(&*table.inner.read().await).await?;
        }

        Ok(table)
    }

    /// Create a new binding for (channel, sender) → agent.
    /// Returns an error if a non-stale binding already exists for this key.
    pub async fn bind(&self, binding: SessionBinding) -> Result<()> {
        let key = (binding.channel.clone(), binding.sender.clone());
        let mut map = self.inner.write().await;
        if let Some(existing) = map.get(&key) {
            if !existing.stale {
                bail!(
                    "already bound to agent '{}' — /disconnect first",
                    existing.agent_name
                );
            }
        }
        map.insert(key, binding);
        self.save_inner(&map).await
    }

    /// Remove the binding for (channel, sender). Returns the removed binding if it existed.
    pub async fn unbind(&self, channel: &str, sender: &str) -> Result<Option<SessionBinding>> {
        let mut map = self.inner.write().await;
        let removed = map.remove(&(channel.to_string(), sender.to_string()));
        if removed.is_some() {
            self.save_inner(&map).await?;
        }
        Ok(removed)
    }

    /// Count the number of non-stale (active) bindings.
    pub async fn count_active(&self) -> usize {
        let map = self.inner.read().await;
        map.values().filter(|b| !b.stale).count()
    }

    /// Look up the binding for (channel, sender).
    pub async fn lookup(&self, channel: &str, sender: &str) -> Option<SessionBinding> {
        let map = self.inner.read().await;
        map.get(&(channel.to_string(), sender.to_string())).cloned()
    }

    /// Persist current state to disk atomically (temp file + rename).
    /// Caller must hold the lock and pass the map reference to avoid races.
    async fn save_inner(&self, map: &HashMap<BindingKey, SessionBinding>) -> Result<()> {
        let bindings: Vec<&SessionBinding> = map.values().collect();
        let data = serde_json::to_string_pretty(&bindings)
            .context("serializing session bridge bindings")?;

        // Write to temp file first, then atomic rename
        let tmp_path = self.persistence_path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, &data).await.with_context(|| {
            format!("writing session bridge bindings to {}", tmp_path.display())
        })?;
        tokio::fs::rename(&tmp_path, &self.persistence_path)
            .await
            .with_context(|| {
                format!(
                    "renaming {} to {}",
                    tmp_path.display(),
                    self.persistence_path.display()
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_binding(channel: &str, sender: &str) -> SessionBinding {
        SessionBinding {
            channel: channel.to_string(),
            sender: sender.to_string(),
            agent_name: "claude".to_string(),
            working_dir: PathBuf::from("/tmp/test"),
            bound_at: Utc::now(),
            stale: false,
        }
    }

    #[tokio::test]
    async fn bind_lookup_unbind_cycle() {
        let tmp = TempDir::new().unwrap();
        let table = BindingTable::load(tmp.path()).await.unwrap();

        // Initially empty
        assert!(table.lookup("telegram", "alice").await.is_none());

        // Bind
        let binding = sample_binding("telegram", "alice");
        table.bind(binding).await.unwrap();

        // Lookup succeeds
        let found = table.lookup("telegram", "alice").await.unwrap();
        assert_eq!(found.agent_name, "claude");
        assert!(!found.stale);

        // Unbind
        let removed = table.unbind("telegram", "alice").await.unwrap();
        assert!(removed.is_some());
        assert!(table.lookup("telegram", "alice").await.is_none());
    }

    #[tokio::test]
    async fn persistence_and_stale_marking() {
        let tmp = TempDir::new().unwrap();

        // Create and bind
        {
            let table = BindingTable::load(tmp.path()).await.unwrap();
            table.bind(sample_binding("telegram", "bob")).await.unwrap();
        }

        // Reload — should be marked stale
        {
            let table = BindingTable::load(tmp.path()).await.unwrap();
            let found = table.lookup("telegram", "bob").await.unwrap();
            assert!(found.stale);
            assert_eq!(found.agent_name, "claude");
        }
    }

    #[tokio::test]
    async fn duplicate_bind_rejected() {
        let tmp = TempDir::new().unwrap();
        let table = BindingTable::load(tmp.path()).await.unwrap();

        // First bind succeeds
        table
            .bind(sample_binding("telegram", "alice"))
            .await
            .unwrap();

        // Second bind to same key fails
        let result = table.bind(sample_binding("telegram", "alice")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already bound"));
    }

    #[tokio::test]
    async fn stale_binding_allows_rebind() {
        let tmp = TempDir::new().unwrap();

        // Create binding, persist
        {
            let table = BindingTable::load(tmp.path()).await.unwrap();
            table
                .bind(sample_binding("telegram", "alice"))
                .await
                .unwrap();
        }

        // Reload (marks stale), then rebind should succeed
        {
            let table = BindingTable::load(tmp.path()).await.unwrap();
            let found = table.lookup("telegram", "alice").await.unwrap();
            assert!(found.stale);

            // Rebind over stale entry succeeds
            table
                .bind(sample_binding("telegram", "alice"))
                .await
                .unwrap();
            let found = table.lookup("telegram", "alice").await.unwrap();
            assert!(!found.stale);
        }
    }

    #[tokio::test]
    async fn malformed_file_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(PERSISTENCE_FILE);
        tokio::fs::write(&path, "not valid json{{{").await.unwrap();

        let result = BindingTable::load(tmp.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parsing"));
    }

    #[tokio::test]
    async fn unbind_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let table = BindingTable::load(tmp.path()).await.unwrap();
        let removed = table.unbind("telegram", "nobody").await.unwrap();
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn load_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let table = BindingTable::load(tmp.path()).await.unwrap();
        assert!(table.lookup("any", "any").await.is_none());
    }
}
