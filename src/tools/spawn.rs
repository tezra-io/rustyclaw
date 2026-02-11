use async_trait::async_trait;

/// Tool for spawning background subagent tasks.
pub struct SpawnTool {
    // Reference to SubagentManager will be added during integration.
    _placeholder: (),
}

impl SpawnTool {
    pub fn new() -> Self {
        Self { _placeholder: () }
    }
}

#[async_trait]
impl super::base::Tool for SpawnTool {
    fn name(&self) -> &str { "spawn" }

    fn description(&self) -> &str {
        "Spawn a background subagent to work on a task asynchronously."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Task description for the subagent" },
                "context": { "type": "string", "description": "Additional context (optional)" }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> crate::error::Result<String> {
        let task = args["task"]
            .as_str()
            .ok_or_else(|| crate::error::NanobotError::Tool("Missing 'task'".into()))?;

        // TODO: Delegate to SubagentManager once integrated.
        Ok(format!("Subagent spawned for task: {}", task))
    }
}
