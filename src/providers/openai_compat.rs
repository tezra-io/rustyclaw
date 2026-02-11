use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, warn};

use super::base::*;
use super::registry::ProviderSpec;

/// Generic OpenAI-compatible provider.
///
/// Works with any API that follows the OpenAI chat completions format:
/// Anthropic (via proxy), OpenAI, DeepSeek, Groq, vLLM, OpenRouter, etc.
pub struct OpenAiCompatProvider {
    client: Client,
    api_base: String,
    api_key: String,
    default_model: String,
    extra_headers: std::collections::HashMap<String, String>,
}

impl OpenAiCompatProvider {
    pub fn new(
        spec: &ProviderSpec,
        api_key: String,
        api_base: Option<String>,
        model: Option<String>,
        extra_headers: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        let base = api_base
            .unwrap_or_else(|| spec.api_base.to_string())
            .trim_end_matches('/')
            .to_string();

        Self {
            client: Client::new(),
            api_base: base,
            api_key,
            default_model: model.unwrap_or_else(|| spec.default_model.to_string()),
            extra_headers: extra_headers.unwrap_or_default(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn chat(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<serde_json::Value>>,
        model: Option<&str>,
        temperature: Option<f64>,
        max_tokens: Option<u32>,
    ) -> crate::error::Result<LlmResponse> {
        let url = format!("{}/chat/completions", self.api_base);
        let model = model.unwrap_or(&self.default_model);

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
        });

        if let Some(t) = temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if let Some(m) = max_tokens {
            body["max_tokens"] = serde_json::json!(m);
        }
        if let Some(t) = &tools {
            if !t.is_empty() {
                body["tools"] = serde_json::json!(t);
            }
        }

        debug!("POST {} model={}", url, model);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            warn!("LLM API error {}: {}", status, text);
            return Err(crate::error::NanobotError::Provider(format!(
                "API returned {}: {}",
                status, text
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| crate::error::NanobotError::Http(e.to_string()))?;

        parse_response(data)
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

/// Parse an OpenAI-compatible chat completion response.
fn parse_response(data: serde_json::Value) -> crate::error::Result<LlmResponse> {
    let choice = data["choices"]
        .get(0)
        .ok_or_else(|| crate::error::NanobotError::Provider("No choices in response".into()))?;

    let content = choice["message"]["content"].as_str().map(String::from);
    let finish_reason = choice["finish_reason"].as_str().map(String::from);

    let tool_calls = choice["message"]["tool_calls"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    Some(ToolCallRequest {
                        id: tc["id"].as_str()?.to_string(),
                        name: tc["function"]["name"].as_str()?.to_string(),
                        arguments: serde_json::from_str(
                            tc["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(serde_json::Value::Object(Default::default())),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let usage = Usage {
        prompt_tokens: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        completion_tokens: data["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        total_tokens: data["usage"]["total_tokens"].as_u64().unwrap_or(0) as u32,
    };

    Ok(LlmResponse {
        content,
        tool_calls,
        usage,
        finish_reason,
    })
}
