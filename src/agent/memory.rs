use chrono::Utc;
use std::path::PathBuf;
use tracing::debug;

/// Simple file-based memory store.
///
/// - Daily notes: `notes/YYYY-MM-DD.md`
/// - Long-term memory: `MEMORY.md`
pub struct MemoryStore {
    workspace: PathBuf,
}

impl MemoryStore {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
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
