/// RedactingTool<T> — trait decorator that scans arguments of external-facing
/// tools for secrets before execution, preventing leakage through browser
/// commands, form fills, and external API calls.
///
/// Only wraps tools tagged as `external_facing`. Internal tools (shell, file I/O,
/// memory) bypass scanning entirely for zero overhead.
///
/// Fail-open: if redaction panics, the original arguments are used and an alert
/// is logged (never silently blocks a tool invocation).
use std::sync::Arc;

use async_trait::async_trait;

use crate::tools::traits::{Tool, ToolResult, ToolSpec};

use super::engine::SentinelEngine;

/// A tool wrapper that applies secret redaction to arguments of external-facing tools.
pub struct RedactingTool<T: Tool> {
    inner: T,
    engine: Arc<SentinelEngine>,
}

impl<T: Tool> RedactingTool<T> {
    /// Wrap a tool with argument redaction.
    pub fn new(inner: T, engine: Arc<SentinelEngine>) -> Self {
        Self { inner, engine }
    }

    /// Recursively redact string values in a JSON value.
    fn redact_json(&self, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(s) => {
                let redacted = self.redact_text(s);
                serde_json::Value::String(redacted)
            }
            serde_json::Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    new_map.insert(k.clone(), self.redact_json(v));
                }
                serde_json::Value::Object(new_map)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(|v| self.redact_json(v)).collect())
            }
            // Numbers, bools, null pass through
            other => other.clone(),
        }
    }

    /// Redact text, failing open on panic.
    fn redact_text(&self, text: &str) -> String {
        let engine = self.engine.clone();
        let text_owned = text.to_string();

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.redact(&text_owned).into_owned()
        })) {
            Ok(redacted) => redacted,
            Err(e) => {
                let panic_msg = if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = e.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!(
                    tool = self.inner.name(),
                    error = %panic_msg,
                    "sentinel: tool arg redaction panicked, using original (fail-open)"
                );
                text_owned
            }
        }
    }
}

#[async_trait]
impl<T: Tool> Tool for RedactingTool<T> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let redacted_args = self.redact_json(&args);
        self.inner.execute(redacted_args).await
    }

    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }
}

/// Known external-facing tool names that should be wrapped with redaction.
/// These tools interact with public surfaces where secrets could leak.
pub const EXTERNAL_FACING_TOOLS: &[&str] = &[
    "browser_type",
    "browser_fill",
    "browser_navigate",
    "browser_act",
    "browser",
    "web_fetch",
    "message_send",
];

/// Check if a tool name is external-facing and should have argument redaction.
pub fn is_external_facing(tool_name: &str) -> bool {
    EXTERNAL_FACING_TOOLS.contains(&tool_name)
        || tool_name.starts_with("browser_")
        || tool_name.starts_with("external_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::sentinel::config::RedactionConfig;
    use std::sync::Mutex;

    /// Test tool that records the arguments it receives.
    struct RecordingTool {
        name: String,
        received_args: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    impl RecordingTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                received_args: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Tool for RecordingTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Test tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            self.received_args.lock().unwrap().push(args.clone());
            Ok(ToolResult {
                success: true,
                output: args.to_string(),
                error: None,
            })
        }
    }

    fn test_engine() -> Arc<SentinelEngine> {
        Arc::new(SentinelEngine::new(&RedactionConfig::default()).unwrap())
    }

    #[tokio::test]
    async fn redacts_string_args() {
        let inner = RecordingTool::new("browser_type");
        let received = inner.received_args.clone();
        let tool = RedactingTool::new(inner, test_engine());

        let args = serde_json::json!({
            "text": "My API key is sk-ant-api03-abc123DEF456_ghi789JKL012mno",
            "selector": "#input"
        });

        tool.execute(args).await.unwrap();

        let received_args = received.lock().unwrap();
        let text = received_args[0]["text"].as_str().unwrap();
        assert!(
            text.contains("[REDACTED:api_key]"),
            "secret not redacted: {text}"
        );
        assert!(!text.contains("sk-ant-api03"), "secret leaked: {text}");
        // Selector should be unchanged
        assert_eq!(received_args[0]["selector"], "#input");
    }

    #[tokio::test]
    async fn redacts_nested_json() {
        let inner = RecordingTool::new("external_api");
        let received = inner.received_args.clone();
        let tool = RedactingTool::new(inner, test_engine());

        let args = serde_json::json!({
            "body": {
                "message": "Key: AKIAIOSFODNN7EXAMPLE",
                "metadata": {
                    "notes": "Connection: postgres://admin:s3cret@db.example.com:5432/mydb"
                }
            }
        });

        tool.execute(args).await.unwrap();

        let received_args = received.lock().unwrap();
        let message = received_args[0]["body"]["message"].as_str().unwrap();
        assert!(message.contains("[REDACTED:aws_credential]"));

        let notes = received_args[0]["body"]["metadata"]["notes"]
            .as_str()
            .unwrap();
        assert!(notes.contains("[REDACTED:connection_string]"));
    }

    #[tokio::test]
    async fn redacts_array_values() {
        let inner = RecordingTool::new("browser_fill");
        let received = inner.received_args.clone();
        let tool = RedactingTool::new(inner, test_engine());

        let args = serde_json::json!({
            "fields": [
                "sk-ant-api03-abc123DEF456_ghi789JKL012mno",
                "clean text"
            ]
        });

        tool.execute(args).await.unwrap();

        let received_args = received.lock().unwrap();
        let first = received_args[0]["fields"][0].as_str().unwrap();
        assert!(first.contains("[REDACTED:api_key]"));
        let second = received_args[0]["fields"][1].as_str().unwrap();
        assert_eq!(second, "clean text");
    }

    #[tokio::test]
    async fn clean_args_pass_through() {
        let inner = RecordingTool::new("browser_type");
        let received = inner.received_args.clone();
        let tool = RedactingTool::new(inner, test_engine());

        let args = serde_json::json!({
            "text": "Hello, world!",
            "count": 42,
            "flag": true
        });

        tool.execute(args.clone()).await.unwrap();

        let received_args = received.lock().unwrap();
        assert_eq!(received_args[0], args);
    }

    #[tokio::test]
    async fn name_and_spec_pass_through() {
        let inner = RecordingTool::new("browser_type");
        let tool = RedactingTool::new(inner, test_engine());
        assert_eq!(tool.name(), "browser_type");
        assert_eq!(tool.description(), "Test tool");
    }

    // --- External-facing detection ---

    #[test]
    fn browser_tools_are_external_facing() {
        assert!(is_external_facing("browser_type"));
        assert!(is_external_facing("browser_fill"));
        assert!(is_external_facing("browser_navigate"));
        assert!(is_external_facing("browser_act"));
        assert!(is_external_facing("browser"));
    }

    #[test]
    fn internal_tools_are_not_external_facing() {
        assert!(!is_external_facing("shell"));
        assert!(!is_external_facing("file_read"));
        assert!(!is_external_facing("memory_store"));
        assert!(!is_external_facing("memory_recall"));
    }

    #[test]
    fn custom_external_prefix_detected() {
        assert!(is_external_facing("external_api_post"));
        assert!(is_external_facing("external_webhook"));
    }
}
