use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Response from an LLM provider.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCallRequest>,
    pub usage: Usage,
    pub finish_reason: Option<String>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Trait for LLM providers (OpenAI-compatible API).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request.
    async fn chat(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<serde_json::Value>>,
        model: Option<&str>,
        temperature: Option<f64>,
        max_tokens: Option<u32>,
    ) -> crate::error::Result<LlmResponse>;

    /// Get the default model for this provider.
    fn default_model(&self) -> &str;
}
