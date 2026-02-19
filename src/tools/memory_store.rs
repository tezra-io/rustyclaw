use super::traits::{Tool, ToolResult};
use crate::memory::{Memory, MemoryCategory};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

/// Let the agent store memories — its own brain writes
pub struct MemoryStoreTool {
    memory: Arc<dyn Memory>,
}

impl MemoryStoreTool {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a fact, preference, or note in long-term memory. Use category 'core' for permanent facts, 'daily' for session notes, 'conversation' for chat context, 'fact' for extracted user facts, 'preference' for user behavioral preferences, 'user_model' for stable user model attributes."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Unique key for this memory (e.g. 'user_lang', 'user_model.response_style')"
                },
                "content": {
                    "type": "string",
                    "description": "The information to remember"
                },
                "category": {
                    "type": "string",
                    "enum": ["core", "daily", "conversation", "fact", "preference", "user_model"],
                    "description": "Memory category: core (permanent), daily (session), conversation (chat), fact (extracted user facts), preference (behavioral preferences), user_model (stable user model attributes)"
                }
            },
            "required": ["key", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;

        let category = match args.get("category").and_then(|v| v.as_str()) {
            Some("daily") => MemoryCategory::Daily,
            Some("conversation") => MemoryCategory::Conversation,
            Some("fact") => MemoryCategory::Fact,
            Some("preference") => MemoryCategory::Preference,
            Some("user_model") => MemoryCategory::UserModel,
            _ => MemoryCategory::Core,
        };

        match self.memory.store(key, content, category).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!("Stored memory: {key}"),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to store memory: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SqliteMemory;
    use tempfile::TempDir;

    fn test_mem() -> (TempDir, Arc<dyn Memory>) {
        let tmp = TempDir::new().unwrap();
        let mem = SqliteMemory::new(tmp.path()).unwrap();
        (tmp, Arc::new(mem))
    }

    #[test]
    fn name_and_schema() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem);
        assert_eq!(tool.name(), "memory_store");
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["key"].is_object());
        assert!(schema["properties"]["content"].is_object());
    }

    #[tokio::test]
    async fn store_core() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem.clone());
        let result = tool
            .execute(json!({"key": "lang", "content": "Prefers Rust"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("lang"));

        let entry = mem.get("lang").await.unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().content, "Prefers Rust");
    }

    #[tokio::test]
    async fn store_with_category() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem.clone());
        let result = tool
            .execute(json!({"key": "note", "content": "Fixed bug", "category": "daily"}))
            .await
            .unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn store_missing_key() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem);
        let result = tool.execute(json!({"content": "no key"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn store_missing_content() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem);
        let result = tool.execute(json!({"key": "no_content"})).await;
        assert!(result.is_err());
    }

    // ── MAPLE Phase 1: new category tests ────────────────────────

    #[tokio::test]
    async fn store_fact_category() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem.clone());
        let result = tool
            .execute(
                json!({"key": "f1", "content": "user is a backend engineer", "category": "fact"}),
            )
            .await
            .unwrap();
        assert!(result.success);
        let entry = mem.get("f1").await.unwrap().unwrap();
        assert_eq!(entry.category, MemoryCategory::Fact);
    }

    #[tokio::test]
    async fn store_preference_category() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem.clone());
        let result = tool
            .execute(json!({"key": "p1", "content": "prefers terse responses", "category": "preference"}))
            .await
            .unwrap();
        assert!(result.success);
        let entry = mem.get("p1").await.unwrap().unwrap();
        assert_eq!(entry.category, MemoryCategory::Preference);
    }

    #[tokio::test]
    async fn store_user_model_category() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem.clone());
        let result = tool
            .execute(json!({"key": "user_model.response_style", "content": "terse", "category": "user_model"}))
            .await
            .unwrap();
        assert!(result.success);
        let entry = mem.get("user_model.response_style").await.unwrap().unwrap();
        assert_eq!(entry.category, MemoryCategory::UserModel);
    }

    #[test]
    fn schema_includes_maple_categories() {
        let (_tmp, mem) = test_mem();
        let tool = MemoryStoreTool::new(mem);
        let schema = tool.parameters_schema();
        let enum_vals = schema["properties"]["category"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(enum_vals.contains(&"fact"));
        assert!(enum_vals.contains(&"preference"));
        assert!(enum_vals.contains(&"user_model"));
    }
}
