use crate::providers::ConversationMessage;
use anyhow::Result;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Maximum number of messages to keep after compaction.
const DEFAULT_COMPACTION_KEEP: usize = 40;

/// Persists conversation history to a JSONL file so persistent agents
/// can resume context across daemon restarts and delegations.
pub struct SessionStore {
    path: PathBuf,
    compaction_keep: usize,
}

impl SessionStore {
    /// Create a new session store. The `sessions_dir` is typically
    /// `~/.rustyclaw/agents/<name>/` and the file is `session.jsonl`.
    pub fn new(agent_data_dir: &Path) -> Self {
        Self {
            path: agent_data_dir.join("session.jsonl"),
            compaction_keep: DEFAULT_COMPACTION_KEEP,
        }
    }

    /// Load persisted conversation history. Returns empty vec if no file exists.
    pub fn load(&self) -> Result<Vec<ConversationMessage>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&self.path)?;
        let reader = std::io::BufReader::new(file);
        let mut messages = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<ConversationMessage>(trimmed) {
                Ok(msg) => messages.push(msg),
                Err(e) => {
                    tracing::warn!("Skipping corrupt session line: {e}");
                }
            }
        }

        Ok(messages)
    }

    /// Append new messages to the session file (incremental save).
    pub fn append(&self, messages: &[ConversationMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        for msg in messages {
            let json = serde_json::to_string(msg)?;
            writeln!(file, "{json}")?;
        }

        Ok(())
    }

    /// Overwrite the session file with the given messages (used after compaction).
    pub fn save_all(&self, messages: &[ConversationMessage]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::File::create(&self.path)?;
        for msg in messages {
            let json = serde_json::to_string(msg)?;
            writeln!(file, "{json}")?;
        }

        Ok(())
    }

    /// Compact the session: keep system messages + last N non-system messages.
    /// Returns the compacted message list.
    pub fn compact(&self, messages: &[ConversationMessage]) -> Result<Vec<ConversationMessage>> {
        let mut system_msgs = Vec::new();
        let mut other_msgs = Vec::new();

        for msg in messages {
            match msg {
                ConversationMessage::Chat(chat) if chat.role == "system" => {
                    system_msgs.push(msg.clone());
                }
                _ => other_msgs.push(msg.clone()),
            }
        }

        if other_msgs.len() > self.compaction_keep {
            let drop_count = other_msgs.len() - self.compaction_keep;
            other_msgs.drain(0..drop_count);
        }

        let mut compacted = system_msgs;
        compacted.extend(other_msgs);

        self.save_all(&compacted)?;
        Ok(compacted)
    }

    /// Clear the session file.
    pub fn clear(&self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Check if a session file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;
    use tempfile::TempDir;

    fn chat(role: &str, content: &str) -> ConversationMessage {
        ConversationMessage::Chat(ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        })
    }

    #[test]
    fn load_empty() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());
        let msgs = store.load().unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn save_and_load() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let messages = vec![
            chat("system", "You are helpful."),
            chat("user", "Hello"),
            chat("assistant", "Hi there!"),
        ];

        store.append(&messages).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn append_incremental() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        store.append(&[chat("user", "First")]).unwrap();
        store.append(&[chat("assistant", "Response")]).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn compact_keeps_system_and_recent() {
        let tmp = TempDir::new().unwrap();
        let mut store = SessionStore::new(tmp.path());
        store.compaction_keep = 3;

        let mut messages = vec![chat("system", "System prompt")];
        for i in 0..10 {
            messages.push(chat("user", &format!("msg-{i}")));
        }

        let compacted = store.compact(&messages).unwrap();
        // 1 system + 3 recent
        assert_eq!(compacted.len(), 4);

        // First should be system
        if let ConversationMessage::Chat(c) = &compacted[0] {
            assert_eq!(c.role, "system");
        } else {
            panic!("Expected system message");
        }

        // Last 3 should be the most recent user messages
        if let ConversationMessage::Chat(c) = &compacted[1] {
            assert_eq!(c.content, "msg-7");
        } else {
            panic!("Expected chat message");
        }
    }

    #[test]
    fn compact_noop_when_small() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        let messages = vec![
            chat("system", "System"),
            chat("user", "Hello"),
            chat("assistant", "Hi"),
        ];

        let compacted = store.compact(&messages).unwrap();
        assert_eq!(compacted.len(), 3);
    }

    #[test]
    fn clear_removes_file() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        store.append(&[chat("user", "Hello")]).unwrap();
        assert!(store.exists());

        store.clear().unwrap();
        assert!(!store.exists());
    }

    #[test]
    fn load_skips_corrupt_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");

        let valid = serde_json::to_string(&chat("user", "Hello")).unwrap();
        std::fs::write(&path, format!("{valid}\n{{bad json}}\n{valid}\n")).unwrap();

        let store = SessionStore::new(tmp.path());
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn save_all_overwrites() {
        let tmp = TempDir::new().unwrap();
        let store = SessionStore::new(tmp.path());

        store.append(&[chat("user", "Old")]).unwrap();
        store.save_all(&[chat("user", "New")]).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        if let ConversationMessage::Chat(c) = &loaded[0] {
            assert_eq!(c.content, "New");
        }
    }
}
