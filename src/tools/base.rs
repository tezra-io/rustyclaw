use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Trait for agent tools.
///
/// Each tool has a name, description, JSON Schema parameters, and an execute method.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String>;

    /// Return the OpenAI-format tool definition.
    fn to_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters(),
            }
        })
    }
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Unregister a tool by name.
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.remove(name)
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with arguments.
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> crate::error::Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| crate::error::RustyClawError::Tool(format!("Unknown tool: {}", name)))?;
        tool.execute(args).await
    }

    /// Get all tool definitions in OpenAI format.
    pub fn definitions(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| t.to_schema()).collect()
    }

    /// List tool names.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Create a scoped registry containing only the named tools (shared via Arc).
    pub fn scoped(&self, allowed: &[&str]) -> ToolRegistry {
        let tools = allowed
            .iter()
            .filter_map(|name| self.tools.get(*name).map(|t| (name.to_string(), t.clone())))
            .collect();
        ToolRegistry { tools }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
