use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Resolve the default Elixir synth API base URL from env or fallback.
fn default_elixir_url() -> String {
    let port = crate::daemon::elixir::resolve_synth_port();
    format!("http://127.0.0.1:{port}")
}
/// TTL for the cached tool list (60 seconds).
const CACHE_TTL: Duration = Duration::from_secs(60);
/// HTTP timeout for execute calls (slightly above Elixir's 30s sandbox timeout).
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(35);
/// HTTP timeout for discovery calls.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// A synthesized tool descriptor received from the Elixir API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthToolInfo {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
    pub status: String,
    #[serde(default)]
    pub invocation_count: u64,
    #[serde(default)]
    pub success_count: u64,
}

/// Response from POST /api/synth/execute.
#[derive(Debug, Deserialize)]
struct ExecuteResponse {
    ok: bool,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Proxy that implements the Tool trait by forwarding execution to the Elixir
/// synth API. One instance per synthesized tool.
pub struct SynthToolProxy {
    name: String,
    description: String,
    schema: serde_json::Value,
    elixir_url: String,
    client: reqwest::Client,
}

impl SynthToolProxy {
    fn new(info: &SynthToolInfo, elixir_url: &str, client: reqwest::Client) -> Self {
        Self {
            name: format!("synth.{}", info.name),
            description: info.description.clone(),
            schema: info.parameters_schema.clone(),
            elixir_url: elixir_url.to_string(),
            client,
        }
    }
}

#[async_trait]
impl Tool for SynthToolProxy {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        // Strip the "synth." prefix for the Elixir API
        let tool_name = self
            .name
            .strip_prefix("synth.")
            .unwrap_or(&self.name)
            .to_string();

        let body = serde_json::json!({
            "tool": tool_name,
            "params": args,
        });

        let resp = self
            .client
            .post(format!("{}/api/synth/execute", self.elixir_url))
            .json(&body)
            .timeout(EXECUTE_TIMEOUT)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("synth API returned HTTP {status}")),
            });
        }

        let parsed: ExecuteResponse = resp.json().await?;
        Ok(ToolResult {
            success: parsed.ok,
            output: parsed.output.unwrap_or_default(),
            error: parsed.error,
        })
    }
}

/// Cached list of synthesized tools with TTL-based expiry.
pub struct SynthToolCache {
    elixir_url: String,
    client: reqwest::Client,
    cache: RwLock<Option<(Instant, Vec<SynthToolInfo>)>>,
}

impl SynthToolCache {
    pub fn new(elixir_url: Option<&str>) -> Self {
        Self {
            elixir_url: elixir_url
                .map(String::from)
                .unwrap_or_else(default_elixir_url),
            client: reqwest::Client::new(),
            cache: RwLock::new(None),
        }
    }

    /// Discover synthesized tools from the Elixir API.
    /// Returns cached results if within TTL; otherwise fetches fresh data.
    /// Returns an empty vec if the Elixir API is not reachable (graceful degradation).
    pub async fn discover(&self) -> Vec<SynthToolInfo> {
        // Check cache
        {
            let guard = self.cache.read().await;
            if let Some((fetched_at, ref tools)) = *guard {
                if fetched_at.elapsed() < CACHE_TTL {
                    return tools.clone();
                }
            }
        }

        // Fetch fresh
        match self.fetch_tools().await {
            Ok(tools) => {
                let mut guard = self.cache.write().await;
                *guard = Some((Instant::now(), tools.clone()));
                tools
            }
            Err(_) => {
                // Graceful degradation: return stale cache or empty
                let guard = self.cache.read().await;
                guard
                    .as_ref()
                    .map(|(_, tools)| tools.clone())
                    .unwrap_or_default()
            }
        }
    }

    /// Build `SynthToolProxy` instances for all active tools.
    pub async fn build_proxies(&self) -> Vec<Box<dyn Tool>> {
        self.discover()
            .await
            .iter()
            .filter(|t| t.status == "probation" || t.status == "promoted")
            .map(|info| -> Box<dyn Tool> {
                Box::new(SynthToolProxy::new(
                    info,
                    &self.elixir_url,
                    self.client.clone(),
                ))
            })
            .collect()
    }

    /// Invalidate the cache so the next call to `discover` fetches fresh data.
    pub async fn invalidate(&self) {
        let mut guard = self.cache.write().await;
        *guard = None;
    }

    async fn fetch_tools(&self) -> anyhow::Result<Vec<SynthToolInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/synth/tools", self.elixir_url))
            .timeout(DISCOVERY_TIMEOUT)
            .send()
            .await?;

        let tools: Vec<SynthToolInfo> = resp.json().await?;
        Ok(tools)
    }
}

/// Convenience: fetch tool list for CLI display.
pub async fn list_synth_tools(elixir_url: Option<&str>) -> anyhow::Result<Vec<SynthToolInfo>> {
    let default = default_elixir_url();
    let url = elixir_url.unwrap_or(&default);
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/api/synth/tools"))
        .timeout(DISCOVERY_TIMEOUT)
        .send()
        .await?;
    let tools: Vec<SynthToolInfo> = resp.json().await?;
    Ok(tools)
}

/// Convenience: approve a tool via CLI.
pub async fn approve_synth_tool(name: &str, elixir_url: Option<&str>) -> anyhow::Result<()> {
    let default = default_elixir_url();
    let url = elixir_url.unwrap_or(&default);
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{url}/api/synth/approve"))
        .json(&serde_json::json!({"name": name}))
        .timeout(DISCOVERY_TIMEOUT)
        .send()
        .await?;
    check_response(resp, "approve").await
}

/// Convenience: suspend a tool via CLI.
pub async fn suspend_synth_tool(name: &str, elixir_url: Option<&str>) -> anyhow::Result<()> {
    let default = default_elixir_url();
    let url = elixir_url.unwrap_or(&default);
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{url}/api/synth/suspend"))
        .json(&serde_json::json!({"name": name}))
        .timeout(DISCOVERY_TIMEOUT)
        .send()
        .await?;
    check_response(resp, "suspend").await
}

/// Convenience: delete a tool via CLI.
pub async fn delete_synth_tool(name: &str, elixir_url: Option<&str>) -> anyhow::Result<()> {
    let default = default_elixir_url();
    let url = elixir_url.unwrap_or(&default);
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("{url}/api/synth/tools/{name}"))
        .timeout(DISCOVERY_TIMEOUT)
        .send()
        .await?;
    check_response(resp, "delete").await
}

async fn check_response(resp: reqwest::Response, action: &str) -> anyhow::Result<()> {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await?;
    if body.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(())
    } else {
        let err = body
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        anyhow::bail!("Failed to {action}: {err} (HTTP {status})")
    }
}

/// Shared cache instance, lazily initialized.
static SYNTH_CACHE: std::sync::OnceLock<Arc<SynthToolCache>> = std::sync::OnceLock::new();

/// Get or create the global SynthToolCache.
pub fn global_cache(elixir_url: Option<&str>) -> Arc<SynthToolCache> {
    SYNTH_CACHE
        .get_or_init(|| Arc::new(SynthToolCache::new(elixir_url)))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_tool_info_deserializes() {
        let json = r#"{
            "name": "csv_parser",
            "description": "Parse CSV data",
            "parameters_schema": {"type": "object"},
            "status": "promoted",
            "invocation_count": 5,
            "success_count": 4
        }"#;
        let info: SynthToolInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "csv_parser");
        assert_eq!(info.status, "promoted");
        assert_eq!(info.invocation_count, 5);
    }

    #[test]
    fn synth_tool_proxy_name_prefixed() {
        let info = SynthToolInfo {
            name: "test_tool".into(),
            description: "A test".into(),
            parameters_schema: serde_json::json!({"type": "object"}),
            status: "promoted".into(),
            invocation_count: 0,
            success_count: 0,
        };
        let proxy = SynthToolProxy::new(&info, "http://localhost:4001", reqwest::Client::new());
        assert_eq!(proxy.name(), "synth.test_tool");
        assert_eq!(proxy.description(), "A test");
    }

    #[test]
    fn execute_response_deserializes_ok() {
        let json = r#"{"ok": true, "output": "hello"}"#;
        let resp: ExecuteResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.output.as_deref(), Some("hello"));
        assert!(resp.error.is_none());
    }

    #[test]
    fn execute_response_deserializes_error() {
        let json = r#"{"ok": false, "error": "boom"}"#;
        let resp: ExecuteResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("boom"));
    }
}
