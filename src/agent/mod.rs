pub mod cluster;
pub mod context;
pub mod definition;
pub mod ledger;
pub mod r#loop;
pub mod memory;
pub mod router;
pub mod skills;
pub mod subagent;
pub mod supervisor;
pub mod tokens;

pub use r#loop::AgentLoop;

/// How a worker agent's memory is scoped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MemoryMode {
    /// Agent gets its own isolated ledger directory.
    #[default]
    Isolated,
    /// Agent shares the master's ledger.
    Shared,
}

/// Parsed agent definition (from markdown frontmatter + body).
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub context_files: Vec<String>,
    pub memory_mode: MemoryMode,
}
