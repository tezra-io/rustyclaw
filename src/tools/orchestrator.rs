//! Rust bridge tools for Elixir agent lifecycle management.
//!
//! Four tools that call the Elixir orchestrator HTTP endpoints:
//! - `spawn_agent`  — spawn a new agent via POST /api/agents/spawn
//! - `list_agents`  — list agents via GET /api/agents
//! - `message_agent` — send a message to an agent via POST /api/agents/message
//! - `kill_agent`   — stop an agent via DELETE /api/agents/{name}

use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Timeout for fast operations: spawn, list, kill.
const FAST_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for synchronous message delivery (agent runs LLM loop).
const MESSAGE_SYNC_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve the Elixir orchestrator base URL from config or env fallback.
fn elixir_base_url(config: &Config) -> String {
    config.elixir.base_url.clone().unwrap_or_else(|| {
        let port = crate::daemon::elixir::resolve_synth_port();
        format!("http://127.0.0.1:{port}")
    })
}

/// Read the bridge secret from the environment, if set.
fn bridge_secret() -> Option<String> {
    std::env::var("RUSTYCLAW_BRIDGE_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Standard JSON envelope returned by the Elixir agent API.
#[derive(Debug, Deserialize)]
struct ElixirResponse {
    ok: bool,
    #[serde(flatten)]
    data: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

/// Issue a POST to the Elixir orchestrator and return the parsed response.
async fn elixir_post(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    timeout: Duration,
) -> Result<ElixirResponse, ToolResult> {
    let mut req = client.post(url).json(body).timeout(timeout);
    if let Some(secret) = bridge_secret() {
        req = req.header("x-bridge-secret", secret);
    }
    send_and_parse(req).await
}

/// Issue a GET to the Elixir orchestrator and return the parsed response.
async fn elixir_get(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, &str)],
    timeout: Duration,
) -> Result<ElixirResponse, ToolResult> {
    let mut req = client.get(url).query(query).timeout(timeout);
    if let Some(secret) = bridge_secret() {
        req = req.header("x-bridge-secret", secret);
    }
    send_and_parse(req).await
}

/// Issue a DELETE to the Elixir orchestrator and return the parsed response.
async fn elixir_delete(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Result<ElixirResponse, ToolResult> {
    let mut req = client.delete(url).timeout(timeout);
    if let Some(secret) = bridge_secret() {
        req = req.header("x-bridge-secret", secret);
    }
    send_and_parse(req).await
}

/// Send a prepared request and parse the JSON response.
async fn send_and_parse(req: reqwest::RequestBuilder) -> Result<ElixirResponse, ToolResult> {
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();

            match serde_json::from_str::<ElixirResponse>(&body_text) {
                Ok(parsed) => Ok(parsed),
                Err(_) if !status.is_success() => Err(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Elixir bridge returned HTTP {status}")),
                }),
                Err(e) => Err(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to parse Elixir response: {e}")),
                }),
            }
        }
        Err(e) if e.is_timeout() => Err(ToolResult {
            success: false,
            output: String::new(),
            error: Some("Request to Elixir orchestrator timed out.".to_string()),
        }),
        Err(e) if e.is_connect() => Err(ToolResult {
            success: false,
            output: String::new(),
            error: Some(
                "Elixir orchestrator is not reachable. Agent management unavailable.".to_string(),
            ),
        }),
        Err(e) => Err(ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!("Elixir bridge request failed: {e}")),
        }),
    }
}

/// Convert an `ElixirResponse` into a `ToolResult`.
fn response_to_result(resp: ElixirResponse) -> ToolResult {
    if resp.ok {
        let output = serde_json::to_string_pretty(&resp.data).unwrap_or_default();
        ToolResult {
            success: true,
            output,
            error: None,
        }
    } else {
        ToolResult {
            success: false,
            output: String::new(),
            error: Some(
                resp.error
                    .unwrap_or_else(|| "Elixir reported failure".to_string()),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// SpawnAgentBridgeTool
// ---------------------------------------------------------------------------

/// Spawn a new agent via the Elixir orchestrator.
pub struct SpawnAgentBridgeTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl SpawnAgentBridgeTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for SpawnAgentBridgeTool {
    fn name(&self) -> &str {
        "spawn_agent"
    }

    fn description(&self) -> &str {
        "Spawn a new agent via the Elixir orchestrator. The agent runs as a supervised \
         OTP process with its own LLM context and capabilities."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for the agent"
                },
                "capabilities": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of capability tags (e.g. [\"research\", \"coding\"])"
                },
                "persistent": {
                    "type": "boolean",
                    "description": "Whether the agent survives orchestrator restarts (default false)"
                },
                "parent": {
                    "type": "string",
                    "description": "Name of the parent agent for delegation hierarchy"
                },
                "model": {
                    "type": "string",
                    "description": "LLM model override for this agent"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing or empty 'name' parameter".to_string()),
                });
            }
        };

        let url = format!("{}/api/agents/spawn", elixir_base_url(&self.config));

        // Forward the full args object — Elixir extracts what it needs.
        let mut body = serde_json::Map::new();
        body.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
        if let Some(caps) = args.get("capabilities") {
            body.insert("capabilities".to_string(), caps.clone());
        }
        if let Some(p) = args.get("persistent") {
            body.insert("persistent".to_string(), p.clone());
        }
        if let Some(parent) = args.get("parent") {
            body.insert("parent".to_string(), parent.clone());
        }
        if let Some(model) = args.get("model") {
            body.insert("model".to_string(), model.clone());
        }
        let body = serde_json::Value::Object(body);

        match elixir_post(&self.client, &url, &body, FAST_TIMEOUT).await {
            Ok(resp) => Ok(response_to_result(resp)),
            Err(err) => Ok(err),
        }
    }
}

// ---------------------------------------------------------------------------
// ListAgentsBridgeTool
// ---------------------------------------------------------------------------

/// List agents registered in the Elixir orchestrator.
pub struct ListAgentsBridgeTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl ListAgentsBridgeTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for ListAgentsBridgeTool {
    fn name(&self) -> &str {
        "list_agents"
    }

    fn description(&self) -> &str {
        "List agents managed by the Elixir orchestrator. Optionally filter by \
         capability or status, and request detailed information."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "detailed": {
                    "type": "boolean",
                    "description": "Include full agent state in the response (default false)"
                },
                "capability": {
                    "type": "string",
                    "description": "Filter agents by capability tag"
                },
                "status": {
                    "type": "string",
                    "description": "Filter agents by status (e.g. 'running', 'idle')"
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let base = elixir_base_url(&self.config);
        let url = format!("{base}/api/agents");

        let mut query_params: Vec<(&str, String)> = Vec::new();
        if args
            .get("detailed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            query_params.push(("detailed", "true".to_string()));
        }
        if let Some(cap) = args.get("capability").and_then(|v| v.as_str()) {
            query_params.push(("capability", cap.to_string()));
        }
        if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
            query_params.push(("status", status.to_string()));
        }

        let query_refs: Vec<(&str, &str)> =
            query_params.iter().map(|(k, v)| (*k, v.as_str())).collect();

        match elixir_get(&self.client, &url, &query_refs, FAST_TIMEOUT).await {
            Ok(resp) => Ok(response_to_result(resp)),
            Err(err) => Ok(err),
        }
    }
}

// ---------------------------------------------------------------------------
// MessageAgentBridgeTool
// ---------------------------------------------------------------------------

/// Send a message to a named agent via the Elixir orchestrator.
pub struct MessageAgentBridgeTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl MessageAgentBridgeTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for MessageAgentBridgeTool {
    fn name(&self) -> &str {
        "message_agent"
    }

    fn description(&self) -> &str {
        "Send a message to a running agent. In sync mode (default) the call blocks \
         until the agent produces a response. In async mode it returns immediately \
         after queuing the message."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Name of the agent to message"
                },
                "message": {
                    "type": "string",
                    "description": "The message content to send"
                },
                "mode": {
                    "type": "string",
                    "enum": ["sync", "async"],
                    "description": "Delivery mode: 'sync' waits for response, 'async' returns immediately (default 'sync')"
                }
            },
            "required": ["target", "message"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing or empty 'target' parameter".to_string()),
                });
            }
        };

        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing or empty 'message' parameter".to_string()),
                });
            }
        };

        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("sync");

        let timeout = if mode == "async" {
            FAST_TIMEOUT
        } else {
            MESSAGE_SYNC_TIMEOUT
        };

        let url = format!("{}/api/agents/message", elixir_base_url(&self.config));
        let body = serde_json::json!({
            "target": target,
            "message": message,
            "mode": mode,
        });

        match elixir_post(&self.client, &url, &body, timeout).await {
            Ok(resp) => Ok(response_to_result(resp)),
            Err(err) => Ok(err),
        }
    }
}

// ---------------------------------------------------------------------------
// KillAgentBridgeTool
// ---------------------------------------------------------------------------

/// Stop and remove a named agent via the Elixir orchestrator.
pub struct KillAgentBridgeTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl KillAgentBridgeTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for KillAgentBridgeTool {
    fn name(&self) -> &str {
        "kill_agent"
    }

    fn description(&self) -> &str {
        "Stop and remove a running agent managed by the Elixir orchestrator."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the agent to stop"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing or empty 'name' parameter".to_string()),
                });
            }
        };

        let base = elixir_base_url(&self.config);
        let mut url = reqwest::Url::parse(&format!("{base}/api/agents/"))
            .map_err(|e| anyhow::anyhow!("Invalid base URL: {e}"))?;
        url.path_segments_mut()
            .expect("base URL is not cannot-be-a-base")
            .push(name);
        let url_str = url.to_string();

        match elixir_delete(&self.client, &url_str, FAST_TIMEOUT).await {
            Ok(resp) => Ok(response_to_result(resp)),
            Err(err) => Ok(err),
        }
    }
}

// ---------------------------------------------------------------------------
// DelegateAgentBridgeTool
// ---------------------------------------------------------------------------

/// Delegate a task to agents via the Elixir orchestrator's capability-based coordinator.
pub struct DelegateAgentBridgeTool {
    config: Arc<Config>,
    client: reqwest::Client,
}

impl DelegateAgentBridgeTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

/// Valid routing strategies for delegation.
const VALID_STRATEGIES: &[&str] = &["first_available", "sequential", "fanout"];

#[async_trait]
impl Tool for DelegateAgentBridgeTool {
    fn name(&self) -> &str {
        "delegate_agent"
    }

    fn description(&self) -> &str {
        "Delegate a task to agents via the Elixir orchestrator's capability-based \
         coordinator. Matches agents by capability, routes using the selected strategy."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task to delegate"
                },
                "capabilities": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Capability tags to match agents"
                },
                "from_agent": {
                    "type": "string",
                    "description": "Delegating agent name for ACL"
                },
                "strategy": {
                    "type": "string",
                    "enum": ["first_available", "sequential", "fanout"],
                    "description": "Routing strategy (default 'first_available')"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 300, max 300)"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let task = match args.get("task").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing or empty 'task' parameter".to_string()),
                });
            }
        };

        // Validate capabilities is an array of strings if present.
        if let Some(caps) = args.get("capabilities") {
            match caps.as_array() {
                Some(arr) => {
                    if !arr.iter().all(|v| v.is_string()) {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("'capabilities' must be an array of strings".to_string()),
                        });
                    }
                }
                None => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("'capabilities' must be an array of strings".to_string()),
                    });
                }
            }
        }

        // Validate strategy if present.
        if let Some(strategy) = args.get("strategy").and_then(|v| v.as_str()) {
            if !VALID_STRATEGIES.contains(&strategy) {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Invalid strategy '{strategy}'. Must be one of: {}",
                        VALID_STRATEGIES.join(", ")
                    )),
                });
            }
        }

        // Resolve and clamp timeout.
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_i64())
            .map(|s| s.clamp(1, 300).unsigned_abs())
            .unwrap_or(300);
        let timeout = Duration::from_secs(timeout_secs);

        let url = format!("{}/api/agents/delegate", elixir_base_url(&self.config));

        let mut body = serde_json::Map::new();
        body.insert(
            "task".to_string(),
            serde_json::Value::String(task.to_string()),
        );
        if let Some(caps) = args.get("capabilities") {
            body.insert("capabilities".to_string(), caps.clone());
        }
        if let Some(from) = args.get("from_agent").and_then(|v| v.as_str()) {
            body.insert(
                "from_agent".to_string(),
                serde_json::Value::String(from.to_string()),
            );
        }
        if let Some(strategy) = args.get("strategy").and_then(|v| v.as_str()) {
            body.insert(
                "strategy".to_string(),
                serde_json::Value::String(strategy.to_string()),
            );
        }
        body.insert(
            "timeout_ms".to_string(),
            serde_json::Value::Number((timeout_secs * 1000).into()),
        );
        let body = serde_json::Value::Object(body);

        match elixir_post(&self.client, &url, &body, timeout).await {
            Ok(resp) => Ok(response_to_result(resp)),
            Err(err) => Ok(err),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Arc<Config> {
        let mut config = Config::default();
        config.elixir.base_url = Some("http://127.0.0.1:1".to_string());
        Arc::new(config)
    }

    // -- SpawnAgentBridgeTool ------------------------------------------------

    #[test]
    fn spawn_tool_name_and_schema() {
        let tool = SpawnAgentBridgeTool::new(test_config());
        assert_eq!(tool.name(), "spawn_agent");

        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"name"));
    }

    #[tokio::test]
    async fn spawn_tool_missing_name_returns_error() {
        let tool = SpawnAgentBridgeTool::new(test_config());
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("name"));
    }

    #[tokio::test]
    async fn spawn_tool_unreachable_returns_graceful_error() {
        let tool = SpawnAgentBridgeTool::new(test_config());
        let result = tool
            .execute(serde_json::json!({ "name": "test-agent" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("not reachable")
                || result.error.as_deref().unwrap().contains("failed"),
            "Unexpected error: {:?}",
            result.error
        );
    }

    // -- ListAgentsBridgeTool ------------------------------------------------

    #[test]
    fn list_tool_name_and_schema() {
        let tool = ListAgentsBridgeTool::new(test_config());
        assert_eq!(tool.name(), "list_agents");

        let schema = tool.parameters_schema();
        assert!(schema["properties"]["detailed"].is_object());
        assert!(schema["properties"]["capability"].is_object());
        assert!(schema["properties"]["status"].is_object());
    }

    #[tokio::test]
    async fn list_tool_unreachable_returns_graceful_error() {
        let tool = ListAgentsBridgeTool::new(test_config());
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("not reachable")
                || result.error.as_deref().unwrap().contains("failed"),
            "Unexpected error: {:?}",
            result.error
        );
    }

    // -- MessageAgentBridgeTool ----------------------------------------------

    #[test]
    fn message_tool_name_and_schema() {
        let tool = MessageAgentBridgeTool::new(test_config());
        assert_eq!(tool.name(), "message_agent");

        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"target"));
        assert!(required_strs.contains(&"message"));
    }

    #[tokio::test]
    async fn message_tool_missing_target_returns_error() {
        let tool = MessageAgentBridgeTool::new(test_config());
        let result = tool
            .execute(serde_json::json!({ "message": "hello" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("target"));
    }

    // -- KillAgentBridgeTool -------------------------------------------------

    #[test]
    fn kill_tool_name_and_schema() {
        let tool = KillAgentBridgeTool::new(test_config());
        assert_eq!(tool.name(), "kill_agent");

        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"name"));
    }

    #[tokio::test]
    async fn kill_tool_unreachable_returns_graceful_error() {
        let tool = KillAgentBridgeTool::new(test_config());
        let result = tool
            .execute(serde_json::json!({ "name": "doomed-agent" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("not reachable")
                || result.error.as_deref().unwrap().contains("failed"),
            "Unexpected error: {:?}",
            result.error
        );
    }

    // -- DelegateAgentBridgeTool ------------------------------------------------

    #[test]
    fn delegate_tool_name_and_schema() {
        let tool = DelegateAgentBridgeTool::new(test_config());
        assert_eq!(tool.name(), "delegate_agent");

        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"task"));
    }

    #[tokio::test]
    async fn delegate_tool_missing_task_returns_error() {
        let tool = DelegateAgentBridgeTool::new(test_config());
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("task"));
    }

    #[tokio::test]
    async fn delegate_tool_invalid_capabilities_returns_error() {
        let tool = DelegateAgentBridgeTool::new(test_config());
        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "capabilities": "not-an-array"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("capabilities"));
    }

    #[tokio::test]
    async fn delegate_tool_unreachable_returns_graceful_error() {
        let tool = DelegateAgentBridgeTool::new(test_config());
        let result = tool
            .execute(serde_json::json!({ "task": "research topic X" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result.error.as_deref().unwrap().contains("not reachable")
                || result.error.as_deref().unwrap().contains("failed"),
            "Unexpected error: {:?}",
            result.error
        );
    }
}
