use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Default timeout for skill execution (5 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// HTTP timeout padding above the skill timeout (10 seconds for overhead).
const TIMEOUT_PADDING_SECS: u64 = 10;

/// Tool that delegates tasks to ephemeral Elixir-supervised skill agents.
///
/// POSTs to Elixir `/api/skills/invoke`, which spawns an ephemeral AgentServer,
/// runs the task via RustBridge, and returns the result.
pub struct InvokeSkillTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct InvokeResponse {
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

impl InvokeSkillTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    fn elixir_base_url(&self) -> String {
        self.config.elixir.base_url.clone().unwrap_or_else(|| {
            let port = crate::daemon::elixir::resolve_synth_port();
            format!("http://127.0.0.1:{port}")
        })
    }
}

#[async_trait]
impl Tool for InvokeSkillTool {
    fn name(&self) -> &str {
        "invoke_skill"
    }

    fn description(&self) -> &str {
        "Delegate a task to an ephemeral skill agent managed by the Elixir orchestrator. \
         The skill agent runs independently with its own LLM context, executes the task, \
         and returns a summary. Use for tasks that benefit from isolated execution."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill template to invoke (e.g. 'coding-skill', 'research-skill')"
                },
                "task": {
                    "type": "string",
                    "description": "The task description for the skill agent to execute"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context or gist to provide the skill agent"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 300, max: 300)"
                }
            },
            "required": ["skill", "task"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let skill = args
            .get("skill")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'skill' parameter"))?;

        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'task' parameter"))?;

        let context = args.get("context").and_then(|v| v.as_str());

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(DEFAULT_TIMEOUT_SECS);

        let timeout_ms = timeout_secs * 1000;
        let url = format!("{}/api/skills/invoke", self.elixir_base_url());

        let mut body = serde_json::json!({
            "skill": skill,
            "task": task,
            "timeout_ms": timeout_ms,
        });
        if let Some(ctx) = context {
            body["context"] = serde_json::Value::String(ctx.to_string());
        }

        let http_timeout = Duration::from_secs(timeout_secs + TIMEOUT_PADDING_SECS);

        let response = self
            .client
            .post(&url)
            .json(&body)
            .timeout(http_timeout)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<InvokeResponse>().await {
                    Ok(parsed) if parsed.ok => {
                        let output = parsed
                            .result
                            .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                            .unwrap_or_else(|| "Skill completed (no output)".to_string());
                        Ok(ToolResult {
                            success: true,
                            output,
                            error: None,
                        })
                    }
                    Ok(parsed) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(
                            parsed
                                .error
                                .unwrap_or_else(|| format!("Skill failed (HTTP {})", status)),
                        ),
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to parse Elixir response: {e}")),
                    }),
                }
            }
            Err(e) if e.is_timeout() => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Skill '{skill}' timed out after {timeout_secs} seconds"
                )),
            }),
            Err(e) if e.is_connect() => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "Elixir orchestrator is not reachable. Skill delegation unavailable."
                        .to_string(),
                ),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Skill invocation failed: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_tool() -> InvokeSkillTool {
        InvokeSkillTool::new(Arc::new(Config::default()))
    }

    #[test]
    fn name_and_description() {
        let tool = test_tool();
        assert_eq!(tool.name(), "invoke_skill");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let tool = test_tool();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["skill"].is_object());
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["context"].is_object());
        assert!(schema["properties"]["timeout_secs"].is_object());
        assert_eq!(schema["required"], serde_json::json!(["skill", "task"]));
    }

    #[tokio::test]
    async fn missing_skill_param_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(serde_json::json!({"task": "do something"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_task_param_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(serde_json::json!({"skill": "coding-skill"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unreachable_elixir_returns_graceful_error() {
        // Point to a port with nothing listening
        let mut config = Config::default();
        config.elixir.base_url = Some("http://127.0.0.1:1".to_string());
        let tool = InvokeSkillTool::new(Arc::new(config));

        let result = tool
            .execute(serde_json::json!({
                "skill": "test-skill",
                "task": "test task"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(
            result.error.as_deref().unwrap().contains("not reachable")
                || result.error.as_deref().unwrap().contains("failed"),
            "Unexpected error: {:?}",
            result.error
        );
    }

    #[test]
    fn elixir_base_url_uses_config_when_set() {
        let mut config = Config::default();
        config.elixir.base_url = Some("http://custom:9999".to_string());
        let tool = InvokeSkillTool::new(Arc::new(config));
        assert_eq!(tool.elixir_base_url(), "http://custom:9999");
    }

    #[test]
    fn elixir_base_url_falls_back_to_synth_port() {
        let tool = test_tool();
        let url = tool.elixir_base_url();
        assert!(url.starts_with("http://127.0.0.1:"));
    }
}
