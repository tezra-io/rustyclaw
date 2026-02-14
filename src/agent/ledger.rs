use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

const SEGMENT_MAX_ENTRIES: u64 = 10_000;
const GENESIS_PREV_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub seq: u64,
    pub timestamp: String,
    pub entry_type: String,
    pub content: serde_json::Value,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChainStatus {
    Ok {
        entries: u64,
    },
    Broken {
        at_seq: u64,
        expected: String,
        got: String,
    },
}

pub struct MemoryLedger {
    dir: PathBuf,
    next_seq: u64,
    last_hash: String,
    /// Latest value for each fact key (built from all "fact" entries).
    index: HashMap<String, String>,
}

impl MemoryLedger {
    /// Open (or create) a ledger in `dir`. Rebuilds the in-memory index.
    pub fn new(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).context("create ledger directory")?;

        let mut ledger = Self {
            dir,
            next_seq: 0,
            last_hash: GENESIS_PREV_HASH.to_string(),
            index: HashMap::new(),
        };
        ledger.rebuild_index()?;
        Ok(ledger)
    }

    /// Append an entry to the ledger. Returns the entry's hash.
    pub fn append(&mut self, entry_type: &str, content: serde_json::Value) -> Result<String> {
        let seq = self.next_seq;
        let timestamp = chrono::Utc::now().to_rfc3339();
        let prev_hash = self.last_hash.clone();
        let hash = compute_hash(seq, &timestamp, entry_type, &content, &prev_hash);

        let entry = LedgerEntry {
            seq,
            timestamp,
            entry_type: entry_type.to_string(),
            content: content.clone(),
            prev_hash,
            hash: hash.clone(),
        };

        let segment = segment_index(seq);
        let path = self.segment_path(segment);
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .with_context(|| format!("open segment {}", path.display()))?;

        let mut line = serde_json::to_string(&entry).context("serialize ledger entry")?;
        line.push('\n');
        file.write_all(line.as_bytes())
            .context("write ledger entry")?;

        // Update in-memory index for fact entries.
        if entry_type == "fact" {
            if let Some(key) = entry.content.get("key").and_then(|v| v.as_str()) {
                let value = entry
                    .content
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.index.insert(key.to_string(), value);
            }
        } else if entry_type == "tombstone" {
            if let Some(ref_key) = entry.content.get("ref_key").and_then(|v| v.as_str()) {
                self.index.remove(ref_key);
            }
        }

        self.next_seq = seq + 1;
        self.last_hash = hash.clone();
        debug!(seq, %hash, "ledger append");
        Ok(hash)
    }

    /// Walk every segment and verify the hash chain.
    pub fn verify_chain(&self) -> Result<ChainStatus> {
        let mut expected_prev = GENESIS_PREV_HASH.to_string();
        let mut count: u64 = 0;

        for seg in 0.. {
            let path = self.segment_path(seg);
            if !path.exists() {
                break;
            }
            let file = fs::File::open(&path)
                .with_context(|| format!("open segment {}", path.display()))?;
            for line_result in BufReader::new(file).lines() {
                let line = line_result.context("read ledger line")?;
                if line.is_empty() {
                    continue;
                }
                let entry: LedgerEntry =
                    serde_json::from_str(&line).context("parse ledger entry")?;

                if entry.prev_hash != expected_prev {
                    return Ok(ChainStatus::Broken {
                        at_seq: entry.seq,
                        expected: expected_prev,
                        got: entry.prev_hash,
                    });
                }

                let recomputed = compute_hash(
                    entry.seq,
                    &entry.timestamp,
                    &entry.entry_type,
                    &entry.content,
                    &entry.prev_hash,
                );
                if entry.hash != recomputed {
                    return Ok(ChainStatus::Broken {
                        at_seq: entry.seq,
                        expected: recomputed,
                        got: entry.hash,
                    });
                }

                expected_prev = entry.hash;
                count += 1;
            }
        }

        Ok(ChainStatus::Ok { entries: count })
    }

    /// Look up the latest value for a fact key.
    pub fn get_latest_fact(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|s| s.as_str())
    }

    /// Return all current facts as (key, value) pairs.
    pub fn all_facts(&self) -> Vec<(String, String)> {
        self.index
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Rebuild the in-memory index (and sequence state) from all segments.
    pub fn rebuild_index(&mut self) -> Result<()> {
        self.index.clear();
        self.next_seq = 0;
        self.last_hash = GENESIS_PREV_HASH.to_string();

        for seg in 0.. {
            let path = self.segment_path(seg);
            if !path.exists() {
                break;
            }
            let file = fs::File::open(&path)
                .with_context(|| format!("open segment {}", path.display()))?;
            for line_result in BufReader::new(file).lines() {
                let line = line_result.context("read ledger line")?;
                if line.is_empty() {
                    continue;
                }
                let entry: LedgerEntry =
                    serde_json::from_str(&line).context("parse ledger entry")?;

                if entry.entry_type == "fact" {
                    if let Some(key) = entry.content.get("key").and_then(|v| v.as_str()) {
                        let value = entry
                            .content
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        self.index.insert(key.to_string(), value);
                    }
                } else if entry.entry_type == "tombstone" {
                    if let Some(ref_key) = entry.content.get("ref_key").and_then(|v| v.as_str()) {
                        self.index.remove(ref_key);
                    }
                }

                self.next_seq = entry.seq + 1;
                self.last_hash = entry.hash;
            }
        }

        debug!(
            entries = self.next_seq,
            facts = self.index.len(),
            "ledger index rebuilt"
        );
        Ok(())
    }

    fn segment_path(&self, segment: u64) -> PathBuf {
        self.dir.join(format!("ledger.{:04}.jsonl", segment))
    }
}

fn segment_index(seq: u64) -> u64 {
    seq / SEGMENT_MAX_ENTRIES
}

fn compute_hash(
    seq: u64,
    timestamp: &str,
    entry_type: &str,
    content: &serde_json::Value,
    prev_hash: &str,
) -> String {
    let content_json = serde_json::to_string(content).unwrap_or_default();
    let input = format!(
        "{}{}{}{}{}",
        seq, timestamp, entry_type, content_json, prev_hash
    );
    let digest = Sha256::digest(input.as_bytes());
    hex::encode(&digest)
}

/// Encode bytes as lowercase hex (no extra crate needed).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

/// Async-safe wrapper around MemoryLedger.
/// All blocking file I/O is dispatched via `tokio::task::spawn_blocking`
/// so it does not stall the async executor.
pub struct AsyncMemoryLedger {
    inner: Arc<Mutex<MemoryLedger>>,
}

impl AsyncMemoryLedger {
    /// Create a new async ledger, opening (or creating) the underlying ledger.
    pub fn new(dir: PathBuf) -> Result<Self> {
        let ledger = MemoryLedger::new(dir)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(ledger)),
        })
    }

    /// Wrap an existing MemoryLedger.
    pub fn from_sync(ledger: MemoryLedger) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ledger)),
        }
    }

    /// Append an entry to the ledger. Returns the entry's hash.
    pub async fn append(
        &self,
        entry_type: &str,
        content: serde_json::Value,
    ) -> anyhow::Result<String> {
        let ledger = self.inner.clone();
        let et = entry_type.to_string();
        tokio::task::spawn_blocking(move || {
            ledger
                .lock()
                .map_err(|e| anyhow::anyhow!("ledger lock poisoned: {}", e))?
                .append(&et, content)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Verify the hash chain integrity.
    pub async fn verify_chain(&self) -> anyhow::Result<ChainStatus> {
        let ledger = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            ledger
                .lock()
                .map_err(|e| anyhow::anyhow!("ledger lock poisoned: {}", e))?
                .verify_chain()
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Look up the latest value for a fact key.
    pub async fn get_latest_fact(&self, key: &str) -> Option<String> {
        let ledger = self.inner.clone();
        let k = key.to_string();
        tokio::task::spawn_blocking(move || {
            ledger
                .lock()
                .ok()
                .and_then(|l| l.get_latest_fact(&k).map(|s| s.to_string()))
        })
        .await
        .ok()
        .flatten()
    }

    /// Return all current facts as (key, value) pairs.
    pub async fn get_all_facts(&self) -> Vec<(String, String)> {
        let ledger = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            ledger
                .lock()
                .ok()
                .map(|l| l.all_facts())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    /// Rebuild the in-memory index from all segments on disk.
    pub async fn rebuild_index(&self) -> anyhow::Result<()> {
        let ledger = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            ledger
                .lock()
                .map_err(|e| anyhow::anyhow!("ledger lock poisoned: {}", e))?
                .rebuild_index()
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_ledger() -> (TempDir, MemoryLedger) {
        let tmp = TempDir::new().unwrap();
        let ledger = MemoryLedger::new(tmp.path().to_path_buf()).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn append_and_verify() {
        let (_tmp, mut ledger) = make_ledger();

        ledger
            .append("fact", serde_json::json!({"key": "name", "value": "Alice"}))
            .unwrap();
        ledger
            .append("fact", serde_json::json!({"key": "city", "value": "NYC"}))
            .unwrap();
        ledger
            .append("note", serde_json::json!({"text": "hello"}))
            .unwrap();

        let status = ledger.verify_chain().unwrap();
        assert_eq!(status, ChainStatus::Ok { entries: 3 });
    }

    #[test]
    fn tamper_detection() {
        let (tmp, mut ledger) = make_ledger();

        ledger
            .append("fact", serde_json::json!({"key": "a", "value": "1"}))
            .unwrap();
        ledger
            .append("fact", serde_json::json!({"key": "b", "value": "2"}))
            .unwrap();
        ledger
            .append("fact", serde_json::json!({"key": "c", "value": "3"}))
            .unwrap();

        // Tamper with the second entry: change content in the file.
        let path = tmp.path().join("ledger.0000.jsonl");
        let contents = fs::read_to_string(&path).unwrap();
        let tampered = contents.replacen("\"value\":\"2\"", "\"value\":\"HACKED\"", 1);
        fs::write(&path, tampered).unwrap();

        let status = ledger.verify_chain().unwrap();
        match status {
            ChainStatus::Broken { at_seq, .. } => assert_eq!(at_seq, 1),
            _ => panic!("expected broken chain"),
        }
    }

    #[test]
    fn auto_segmentation() {
        let (_tmp, mut ledger) = make_ledger();

        // Append exactly SEGMENT_MAX_ENTRIES + 1 entries.
        for i in 0..=SEGMENT_MAX_ENTRIES {
            ledger.append("note", serde_json::json!({"i": i})).unwrap();
        }

        // First segment should exist.
        assert!(ledger.segment_path(0).exists());
        // Second segment should exist (entry 10000 goes into segment 1).
        assert!(ledger.segment_path(1).exists());

        let status = ledger.verify_chain().unwrap();
        assert_eq!(
            status,
            ChainStatus::Ok {
                entries: SEGMENT_MAX_ENTRIES + 1
            }
        );
    }

    #[test]
    fn index_rebuild() {
        let (tmp, mut ledger) = make_ledger();

        ledger
            .append("fact", serde_json::json!({"key": "color", "value": "blue"}))
            .unwrap();
        ledger
            .append("fact", serde_json::json!({"key": "color", "value": "red"}))
            .unwrap();
        ledger
            .append("fact", serde_json::json!({"key": "pet", "value": "cat"}))
            .unwrap();

        // Rebuild from disk.
        let mut fresh = MemoryLedger::new(tmp.path().to_path_buf()).unwrap();
        fresh.rebuild_index().unwrap();

        assert_eq!(fresh.get_latest_fact("color"), Some("red"));
        assert_eq!(fresh.get_latest_fact("pet"), Some("cat"));
        assert_eq!(fresh.get_latest_fact("missing"), None);
    }

    #[test]
    fn tombstone_redact() {
        let (_tmp, mut ledger) = make_ledger();

        ledger
            .append(
                "fact",
                serde_json::json!({"key": "secret", "value": "password123"}),
            )
            .unwrap();
        assert_eq!(ledger.get_latest_fact("secret"), Some("password123"));

        ledger
            .append(
                "tombstone",
                serde_json::json!({"ref_key": "secret", "reason": "user requested"}),
            )
            .unwrap();
        assert_eq!(ledger.get_latest_fact("secret"), None);

        // Chain still valid.
        let status = ledger.verify_chain().unwrap();
        assert_eq!(status, ChainStatus::Ok { entries: 2 });
    }

    #[test]
    fn hash_determinism() {
        let content = serde_json::json!({"key": "x", "value": "y"});
        let h1 = compute_hash(
            0,
            "2026-01-01T00:00:00Z",
            "fact",
            &content,
            GENESIS_PREV_HASH,
        );
        let h2 = compute_hash(
            0,
            "2026-01-01T00:00:00Z",
            "fact",
            &content,
            GENESIS_PREV_HASH,
        );
        assert_eq!(h1, h2);

        // Different seq → different hash.
        let h3 = compute_hash(
            1,
            "2026-01-01T00:00:00Z",
            "fact",
            &content,
            GENESIS_PREV_HASH,
        );
        assert_ne!(h1, h3);
    }
}
