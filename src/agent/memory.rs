use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;
use tracing::debug;

use super::ledger::{AsyncMemoryLedger, ChainStatus, MemoryLedger};

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

/// Async-safe memory store that wraps ledger operations in spawn_blocking.
/// Use this from async agent code instead of MemoryStore directly.
pub struct AsyncMemoryStore {
    workspace: PathBuf,
    ledger: AsyncMemoryLedger,
}

impl AsyncMemoryStore {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let ledger_dir = workspace.join("memory");
        let ledger = AsyncMemoryLedger::new(ledger_dir)?;
        Ok(Self { workspace, ledger })
    }

    /// Store a fact in the tamper-proof ledger (async-safe).
    pub async fn store_fact(&self, key: &str, value: &str) -> anyhow::Result<String> {
        self.ledger
            .append("fact", serde_json::json!({"key": key, "value": value}))
            .await
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

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }
}
