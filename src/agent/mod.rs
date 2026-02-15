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
pub mod templates;
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

/// A scheduled entry for an agent (from YAML frontmatter).
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEntry {
    /// Standard 5-field cron expression.
    Cron { expression: String, task: String },
    /// Fixed interval (e.g., "4h", "30m").
    Every {
        interval: std::time::Duration,
        task: String,
    },
}

/// What kind of event triggers an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerEvent {
    GitPush,
    FileChange,
    MessageMatch,
}

/// Event-based trigger configuration for an agent.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerConfig {
    pub on: TriggerEvent,
    pub branches: Option<Vec<String>>,
    pub pattern: Option<String>,
    pub paths: Option<Vec<String>>,
    pub task: String,
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
    pub schedule: Vec<ScheduleEntry>,
    pub trigger: Option<TriggerConfig>,
}
