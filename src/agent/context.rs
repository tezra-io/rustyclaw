use tracing::debug;

use crate::config::schema::Config;
use crate::providers::base::ChatMessage;

/// Files loaded into the system prompt (from workspace).
const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

/// Builds the message context for an LLM call.
pub struct ContextBuilder<'a> {
    config: &'a Config,
    history: Vec<ChatMessage>,
    extra_system: Vec<String>,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            history: Vec::new(),
            extra_system: Vec::new(),
        }
    }

    /// Add conversation history.
    pub fn with_history(mut self, history: Vec<ChatMessage>) -> Self {
        self.history = history;
        self
    }

    /// Add extra system prompt sections (memory, skills, etc.).
    pub fn with_extra(mut self, section: String) -> Self {
        self.extra_system.push(section);
        self
    }

    /// Build the complete messages list.
    pub fn build(self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System prompt from bootstrap files
        let system_content = self.load_system_prompt();
        if !system_content.is_empty() {
            messages.push(ChatMessage {
                role: "system".into(),
                content: serde_json::Value::String(system_content),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }

        // Conversation history
        messages.extend(self.history);

        messages
    }

    /// Load and concatenate bootstrap files from workspace.
    fn load_system_prompt(&self) -> String {
        let workspace = self.config.workspace_path();
        let mut parts = Vec::new();

        for filename in BOOTSTRAP_FILES {
            let path = workspace.join(filename);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.trim().is_empty() {
                        debug!("Loaded bootstrap file: {}", filename);
                        parts.push(content);
                    }
                }
            }
        }

        // Memory
        let memory = crate::agent::memory::MemoryStore::new(workspace.clone());
        if let Some(mem) = memory.get_context() {
            parts.push(mem);
        }

        // Extra sections
        for section in &self.extra_system {
            parts.push(section.clone());
        }

        parts.join("\n\n---\n\n")
    }
}
