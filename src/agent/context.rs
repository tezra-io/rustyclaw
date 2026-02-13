use tracing::debug;

use crate::agent::{AgentDefinition, MemoryMode};
use crate::config::schema::Config;
use crate::providers::base::ChatMessage;

/// Files loaded into the system prompt (from workspace).
const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

/// Builds the message context for an LLM call.
pub struct ContextBuilder<'a> {
    config: &'a Config,
    history: Vec<ChatMessage>,
    extra_system: Vec<String>,
    agent_def: Option<AgentDefinition>,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            history: Vec::new(),
            extra_system: Vec::new(),
            agent_def: None,
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

    /// Configure this builder for a specific agent definition.
    /// Overrides the system prompt with the agent's prompt, loads agent-specific
    /// context files, and uses the agent's memory path.
    pub fn for_agent(mut self, agent: &AgentDefinition) -> Self {
        self.agent_def = Some(agent.clone());
        self
    }

    /// Build the complete messages list.
    pub fn build(self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        let system_content = if self.agent_def.is_some() {
            self.load_agent_prompt()
        } else {
            self.load_system_prompt()
        };

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

    /// Load system prompt for a specific agent definition.
    fn load_agent_prompt(&self) -> String {
        let agent = self.agent_def.as_ref().unwrap();
        let workspace = self.config.workspace_path();
        let mut parts = Vec::new();

        // Agent's own system prompt (markdown body)
        if !agent.system_prompt.is_empty() {
            parts.push(agent.system_prompt.clone());
        }

        // Agent-specific context files
        for file in &agent.context_files {
            let path = workspace.join(file);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.trim().is_empty() {
                        debug!("Loaded agent context file: {}", file);
                        parts.push(content);
                    }
                }
            } else {
                tracing::warn!(
                    "Agent '{}' context file not found: {}",
                    agent.name,
                    path.display()
                );
            }
        }

        // Memory — use agent-isolated path or master's depending on mode
        let memory_dir = match agent.memory_mode {
            MemoryMode::Isolated => workspace.join("memory").join(&agent.name),
            MemoryMode::Shared => workspace.clone(),
        };
        match crate::agent::memory::MemoryStore::new(memory_dir) {
            Ok(memory) => {
                if let Some(mem) = memory.get_context() {
                    parts.push(mem);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize memory store for agent '{}': {}",
                    agent.name,
                    e
                );
            }
        }

        // Extra sections
        for section in &self.extra_system {
            parts.push(section.clone());
        }

        parts.join("\n\n---\n\n")
    }

    /// Load and concatenate bootstrap files from workspace (master agent).
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
        match crate::agent::memory::MemoryStore::new(workspace.clone()) {
            Ok(memory) => {
                if let Some(mem) = memory.get_context() {
                    parts.push(mem);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to initialize memory store: {}", e);
            }
        }

        // Extra sections
        for section in &self.extra_system {
            parts.push(section.clone());
        }

        parts.join("\n\n---\n\n")
    }
}
