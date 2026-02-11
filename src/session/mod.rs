use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tracing::debug;

use crate::providers::base::ChatMessage;

/// File-based session manager with in-memory cache.
///
/// Sessions are stored as JSONL files: one JSON message per line.
pub struct SessionManager {
    data_dir: PathBuf,
    cache: Mutex<HashMap<String, Vec<ChatMessage>>>,
}

impl SessionManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let sessions_dir = data_dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir).ok();
        Self {
            data_dir: sessions_dir,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn session_path(&self, key: &str) -> PathBuf {
        let safe_key = key.replace([':', '/', '\\'], "_");
        self.data_dir.join(format!("{}.jsonl", safe_key))
    }

    /// Get conversation history for a session.
    pub async fn get_history(&self, key: &str, max_messages: usize) -> Vec<ChatMessage> {
        let mut cache = self.cache.lock().await;

        if let Some(messages) = cache.get(key) {
            let start = messages.len().saturating_sub(max_messages);
            return messages[start..].to_vec();
        }

        // Load from file
        let path = self.session_path(key);
        let messages = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => content
                    .lines()
                    .filter_map(|line| serde_json::from_str::<ChatMessage>(line).ok())
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        cache.insert(key.to_string(), messages.clone());

        let start = messages.len().saturating_sub(max_messages);
        messages[start..].to_vec()
    }

    /// Append a message to a session.
    pub async fn append(&self, key: &str, role: &str, content: &str) {
        let msg = ChatMessage {
            role: role.to_string(),
            content: serde_json::Value::String(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };

        // Update cache
        let mut cache = self.cache.lock().await;
        cache.entry(key.to_string()).or_default().push(msg.clone());

        // Append to file
        let path = self.session_path(key);
        if let Ok(json) = serde_json::to_string(&msg) {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                writeln!(file, "{}", json).ok();
            }
        }
    }

    /// Clear a session's history.
    pub async fn clear(&self, key: &str) {
        let mut cache = self.cache.lock().await;
        cache.remove(key);
        let path = self.session_path(key);
        std::fs::remove_file(&path).ok();
        debug!("Cleared session: {}", key);
    }
}
