use crate::providers::traits::{
    ChatMessage, ChatRequest as ProviderChatRequest, ChatResponse as ProviderChatResponse,
    Provider, ToolCall as ProviderToolCall,
};
use crate::tools::ToolSpec;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

pub struct AnthropicProvider {
    credential: RwLock<Option<String>>,
    base_url: String,
    client: Client,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
    temperature: f64,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

// ── Prompt caching types ────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
}

impl CacheControl {
    fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

/// System prompt: either a plain string or a list of blocks (for caching).
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SystemPrompt {
    String(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Debug, Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

// ── Native API types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct NativeChatRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<SystemPrompt>,
    messages: Vec<NativeMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<NativeToolSpec>>,
}

#[derive(Debug, Serialize)]
struct NativeMessage {
    role: String,
    content: Vec<NativeContentOut>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum NativeContentOut {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Serialize)]
struct NativeToolSpec {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Deserialize)]
struct NativeChatResponse {
    #[serde(default)]
    content: Vec<NativeContentIn>,
}

#[derive(Debug, Deserialize)]
struct NativeContentIn {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

impl AnthropicProvider {
    pub fn new(api_key: Option<&str>) -> Self {
        Self::with_base_url(api_key, None)
    }

    pub fn with_base_url(api_key: Option<&str>, base_url: Option<&str>) -> Self {
        let base_url = base_url
            .map(|u| u.trim_end_matches('/'))
            .unwrap_or("https://api.anthropic.com")
            .to_string();
        Self {
            credential: RwLock::new(
                api_key
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(ToString::to_string),
            ),
            base_url,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn is_setup_token(token: &str) -> bool {
        token.starts_with("sk-ant-oat01-")
    }

    /// Get the current credential, returning an error if not set.
    fn get_credential(&self) -> anyhow::Result<String> {
        self.credential
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Anthropic credentials not set. Set ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN (setup-token)."
                )
            })
    }

    fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        credential: &str,
    ) -> reqwest::RequestBuilder {
        if Self::is_setup_token(credential) {
            request
                .header("Authorization", format!("Bearer {credential}"))
                .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            request.header("x-api-key", credential)
        }
    }

    /// Try to read a fresh OAT token from Claude Code's macOS keychain entry.
    /// Returns `Some(token)` if found and valid, `None` otherwise.
    #[cfg(target_os = "macos")]
    fn try_refresh_from_keychain() -> Option<String> {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8(output.stdout).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(json_str.trim()).ok()?;
        let token = parsed
            .get("claudeAiOauth")?
            .get("accessToken")?
            .as_str()?;

        if Self::is_setup_token(token) {
            tracing::info!("Refreshed OAT token from Claude Code keychain");
            Some(token.to_string())
        } else {
            None
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn try_refresh_from_keychain() -> Option<String> {
        None
    }

    /// Attempt to refresh the stored credential on 401. Returns true if refreshed.
    fn try_refresh_credential(&self) -> bool {
        let current = self.credential.read().unwrap().clone();
        // Only attempt refresh for OAT tokens
        if !current.as_deref().is_some_and(Self::is_setup_token) {
            return false;
        }

        if let Some(fresh_token) = Self::try_refresh_from_keychain() {
            if current.as_deref() != Some(&fresh_token) {
                *self.credential.write().unwrap() = Some(fresh_token);
                return true;
            }
        }

        tracing::warn!(
            "OAuth token expired and could not be refreshed. \
             On macOS, install Claude Code and authenticate. \
             Otherwise, set a fresh token via `rustyclaw onboard`."
        );
        false
    }

    // ── Prompt caching helpers ──────────────────────────────────

    /// Cache system prompts longer than 3 KB (3072 bytes).
    fn should_cache_system(text: &str) -> bool {
        text.len() > 3072
    }

    /// Cache conversation context after more than 4 non-system turns.
    fn should_cache_conversation(messages: &[ChatMessage]) -> bool {
        messages.iter().filter(|m| m.role != "system").count() > 4
    }

    /// Set `cache_control` on the last content block of the last message.
    fn apply_cache_to_last_message(messages: &mut [NativeMessage]) {
        if let Some(last_msg) = messages.last_mut() {
            if let Some(last_content) = last_msg.content.last_mut() {
                let cache_control = match last_content {
                    NativeContentOut::Text { cache_control, .. }
                    | NativeContentOut::ToolResult { cache_control, .. }
                    | NativeContentOut::ToolUse { cache_control, .. } => cache_control,
                };
                *cache_control = Some(CacheControl::ephemeral());
            }
        }
    }

    fn convert_tools(tools: Option<&[ToolSpec]>) -> Option<Vec<NativeToolSpec>> {
        let items = tools?;
        if items.is_empty() {
            return None;
        }
        let mut native_tools: Vec<NativeToolSpec> = items
            .iter()
            .map(|tool| NativeToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.parameters.clone(),
                cache_control: None,
            })
            .collect();
        // Cache all tools up to the last one by marking the last tool.
        if let Some(last) = native_tools.last_mut() {
            last.cache_control = Some(CacheControl::ephemeral());
        }
        Some(native_tools)
    }

    fn parse_assistant_tool_call_message(content: &str) -> Option<Vec<NativeContentOut>> {
        let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
        let tool_calls = value
            .get("tool_calls")
            .and_then(|v| serde_json::from_value::<Vec<ProviderToolCall>>(v.clone()).ok())?;

        let mut blocks = Vec::new();
        if let Some(text) = value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            blocks.push(NativeContentOut::Text {
                text: text.to_string(),
                cache_control: None,
            });
        }
        for call in tool_calls {
            let input = serde_json::from_str::<serde_json::Value>(&call.arguments)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            blocks.push(NativeContentOut::ToolUse {
                id: call.id,
                name: call.name,
                input,
                cache_control: None,
            });
        }
        Some(blocks)
    }

    fn parse_tool_result_message(content: &str) -> Option<NativeMessage> {
        let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
        let tool_use_id = value
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let result = value
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Some(NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::ToolResult {
                tool_use_id,
                content: result,
                cache_control: None,
            }],
        })
    }

    fn convert_messages(
        messages: &[ChatMessage],
    ) -> (Option<SystemPrompt>, Vec<NativeMessage>) {
        let mut system_text = None;
        let mut native_messages = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    if system_text.is_none() {
                        system_text = Some(msg.content.clone());
                    }
                }
                "assistant" => {
                    if let Some(blocks) = Self::parse_assistant_tool_call_message(&msg.content) {
                        native_messages.push(NativeMessage {
                            role: "assistant".to_string(),
                            content: blocks,
                        });
                    } else {
                        native_messages.push(NativeMessage {
                            role: "assistant".to_string(),
                            content: vec![NativeContentOut::Text {
                                text: msg.content.clone(),
                                cache_control: None,
                            }],
                        });
                    }
                }
                "tool" => {
                    if let Some(tool_result) = Self::parse_tool_result_message(&msg.content) {
                        native_messages.push(tool_result);
                    } else {
                        native_messages.push(NativeMessage {
                            role: "user".to_string(),
                            content: vec![NativeContentOut::Text {
                                text: msg.content.clone(),
                                cache_control: None,
                            }],
                        });
                    }
                }
                _ => {
                    native_messages.push(NativeMessage {
                        role: "user".to_string(),
                        content: vec![NativeContentOut::Text {
                            text: msg.content.clone(),
                            cache_control: None,
                        }],
                    });
                }
            }
        }

        let system_prompt = system_text.map(|text| {
            if Self::should_cache_system(&text) {
                SystemPrompt::Blocks(vec![SystemBlock {
                    block_type: "text".to_string(),
                    text,
                    cache_control: Some(CacheControl::ephemeral()),
                }])
            } else {
                SystemPrompt::String(text)
            }
        });

        (system_prompt, native_messages)
    }

    fn parse_text_response(response: ChatResponse) -> anyhow::Result<String> {
        response
            .content
            .into_iter()
            .find(|c| c.kind == "text")
            .and_then(|c| c.text)
            .ok_or_else(|| anyhow::anyhow!("No response from Anthropic"))
    }

    fn parse_native_response(response: NativeChatResponse) -> ProviderChatResponse {
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in response.content {
            match block.kind.as_str() {
                "text" => {
                    if let Some(text) = block.text.map(|t| t.trim().to_string()) {
                        if !text.is_empty() {
                            text_parts.push(text);
                        }
                    }
                }
                "tool_use" => {
                    let name = block.name.unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let arguments = block
                        .input
                        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                    tool_calls.push(ProviderToolCall {
                        id: block.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        name,
                        arguments: arguments.to_string(),
                    });
                }
                _ => {}
            }
        }

        ProviderChatResponse {
            text: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            },
            tool_calls,
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let credential = self.get_credential()?;

        let body = ChatRequest {
            model: model.to_string(),
            max_tokens: 4096,
            system: system_prompt.map(ToString::to_string),
            messages: vec![Message {
                role: "user".to_string(),
                content: message.to_string(),
            }],
            temperature,
        };

        let request = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body);

        let response = self.apply_auth(request, &credential).send().await?;

        // On 401 with OAT token, try keychain refresh and retry once
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && Self::is_setup_token(&credential)
            && self.try_refresh_credential()
        {
            let fresh = self.get_credential()?;
            let retry = self
                .client
                .post(format!("{}/v1/messages", self.base_url))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body);
            let response = self.apply_auth(retry, &fresh).send().await?;
            if !response.status().is_success() {
                return Err(super::api_error("Anthropic", response).await);
            }
            let chat_response: ChatResponse = response.json().await?;
            return Self::parse_text_response(chat_response);
        }

        if !response.status().is_success() {
            return Err(super::api_error("Anthropic", response).await);
        }

        let chat_response: ChatResponse = response.json().await?;
        Self::parse_text_response(chat_response)
    }

    async fn chat(
        &self,
        request: ProviderChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        let credential = self.get_credential()?;

        let (system_prompt, mut messages) = Self::convert_messages(request.messages);

        // Apply conversation-level caching for long contexts
        if Self::should_cache_conversation(request.messages) {
            Self::apply_cache_to_last_message(&mut messages);
        }

        let native_request = NativeChatRequest {
            model: model.to_string(),
            max_tokens: 4096,
            system: system_prompt,
            messages,
            temperature,
            tools: Self::convert_tools(request.tools),
        };

        let req = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&native_request);

        let response = self.apply_auth(req, &credential).send().await?;

        // On 401 with OAT token, try keychain refresh and retry once
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && Self::is_setup_token(&credential)
            && self.try_refresh_credential()
        {
            let fresh = self.get_credential()?;
            let retry = self
                .client
                .post(format!("{}/v1/messages", self.base_url))
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&native_request);
            let response = self.apply_auth(retry, &fresh).send().await?;
            if !response.status().is_success() {
                return Err(super::api_error("Anthropic", response).await);
            }
            let native_response: NativeChatResponse = response.json().await?;
            return Ok(Self::parse_native_response(native_response));
        }

        if !response.status().is_success() {
            return Err(super::api_error("Anthropic", response).await);
        }

        let native_response: NativeChatResponse = response.json().await?;
        Ok(Self::parse_native_response(native_response))
    }

    fn supports_native_tools(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_with_key() {
        let p = AnthropicProvider::new(Some("sk-ant-test123"));
        assert!(p.credential.read().unwrap().is_some());
        assert_eq!(p.credential.read().unwrap().as_deref(), Some("sk-ant-test123"));
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn creates_without_key() {
        let p = AnthropicProvider::new(None);
        assert!(p.credential.read().unwrap().is_none());
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn creates_with_empty_key() {
        let p = AnthropicProvider::new(Some(""));
        assert!(p.credential.read().unwrap().is_none());
    }

    #[test]
    fn creates_with_whitespace_key() {
        let p = AnthropicProvider::new(Some("  sk-ant-test123  "));
        assert!(p.credential.read().unwrap().is_some());
        assert_eq!(p.credential.read().unwrap().as_deref(), Some("sk-ant-test123"));
    }

    #[test]
    fn creates_with_custom_base_url() {
        let p =
            AnthropicProvider::with_base_url(Some("sk-ant-test"), Some("https://api.example.com"));
        assert_eq!(p.base_url, "https://api.example.com");
        assert_eq!(p.credential.read().unwrap().as_deref(), Some("sk-ant-test"));
    }

    #[test]
    fn custom_base_url_trims_trailing_slash() {
        let p = AnthropicProvider::with_base_url(None, Some("https://api.example.com/"));
        assert_eq!(p.base_url, "https://api.example.com");
    }

    #[test]
    fn default_base_url_when_none_provided() {
        let p = AnthropicProvider::with_base_url(None, None);
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[tokio::test]
    async fn chat_fails_without_key() {
        let p = AnthropicProvider::new(None);
        let result = p
            .chat_with_system(None, "hello", "claude-3-opus", 0.7)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("credentials not set"),
            "Expected key error, got: {err}"
        );
    }

    #[test]
    fn setup_token_detection_works() {
        assert!(AnthropicProvider::is_setup_token("sk-ant-oat01-abcdef"));
        assert!(!AnthropicProvider::is_setup_token("sk-ant-api-key"));
    }

    #[tokio::test]
    async fn chat_with_system_fails_without_key() {
        let p = AnthropicProvider::new(None);
        let result = p
            .chat_with_system(Some("You are RustyClaw"), "hello", "claude-3-opus", 0.7)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn chat_request_serializes_without_system() {
        let req = ChatRequest {
            model: "claude-3-opus".to_string(),
            max_tokens: 4096,
            system: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            temperature: 0.7,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains("system"),
            "system field should be skipped when None"
        );
        assert!(json.contains("claude-3-opus"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn chat_request_serializes_with_system() {
        let req = ChatRequest {
            model: "claude-3-opus".to_string(),
            max_tokens: 4096,
            system: Some("You are RustyClaw".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            temperature: 0.7,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"system\":\"You are RustyClaw\""));
    }

    #[test]
    fn chat_response_deserializes() {
        let json = r#"{"content":[{"type":"text","text":"Hello there!"}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].kind, "text");
        assert_eq!(resp.content[0].text.as_deref(), Some("Hello there!"));
    }

    #[test]
    fn chat_response_empty_content() {
        let json = r#"{"content":[]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert!(resp.content.is_empty());
    }

    #[test]
    fn chat_response_multiple_blocks() {
        let json =
            r#"{"content":[{"type":"text","text":"First"},{"type":"text","text":"Second"}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.content[0].text.as_deref(), Some("First"));
        assert_eq!(resp.content[1].text.as_deref(), Some("Second"));
    }

    #[test]
    fn temperature_range_serializes() {
        for temp in [0.0, 0.5, 1.0, 2.0] {
            let req = ChatRequest {
                model: "claude-3-opus".to_string(),
                max_tokens: 4096,
                system: None,
                messages: vec![],
                temperature: temp,
            };
            let json = serde_json::to_string(&req).unwrap();
            assert!(json.contains(&format!("{temp}")));
        }
    }

    // ── Prompt caching tests ────────────────────────────────────

    #[test]
    fn cache_control_serializes_correctly() {
        let cc = CacheControl::ephemeral();
        let json = serde_json::to_string(&cc).unwrap();
        assert_eq!(json, r#"{"type":"ephemeral"}"#);
    }

    #[test]
    fn system_prompt_string_variant_no_cache_control_field() {
        let sp = SystemPrompt::String("Hello".to_string());
        let json = serde_json::to_string(&sp).unwrap();
        assert_eq!(json, r#""Hello""#);
    }

    #[test]
    fn system_prompt_blocks_variant_serializes() {
        let sp = SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: "You are helpful.".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        }]);
        let json = serde_json::to_string(&sp).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"cache_control\":{\"type\":\"ephemeral\"}"));
        assert!(json.contains("You are helpful."));
    }

    #[test]
    fn should_cache_system_boundary() {
        let short = "x".repeat(3072);
        let long = "x".repeat(3073);
        assert!(!AnthropicProvider::should_cache_system(&short));
        assert!(AnthropicProvider::should_cache_system(&long));
    }

    #[test]
    fn should_cache_conversation_boundary() {
        use crate::providers::traits::ChatMessage;
        let make_msg = |role: &str| ChatMessage {
            role: role.to_string(),
            content: "hi".to_string(),
        };
        let four = vec![
            make_msg("user"),
            make_msg("assistant"),
            make_msg("user"),
            make_msg("assistant"),
        ];
        let five = {
            let mut v = four.clone();
            v.push(make_msg("user"));
            v
        };
        assert!(!AnthropicProvider::should_cache_conversation(&four));
        assert!(AnthropicProvider::should_cache_conversation(&five));
    }

    #[test]
    fn convert_tools_sets_cache_on_last_only() {
        use crate::tools::ToolSpec;
        let tools = vec![
            ToolSpec {
                name: "tool_a".to_string(),
                description: "A".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            ToolSpec {
                name: "tool_b".to_string(),
                description: "B".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ];
        let native = AnthropicProvider::convert_tools(Some(&tools)).unwrap();
        assert!(native[0].cache_control.is_none());
        assert!(native[1].cache_control.is_some());
    }

    #[test]
    fn convert_tools_empty_returns_none() {
        let result = AnthropicProvider::convert_tools(Some(&[]));
        assert!(result.is_none());
    }

    #[test]
    fn convert_messages_small_system_returns_string() {
        use crate::providers::traits::ChatMessage;
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: "short prompt".to_string(),
        }];
        let (sys, _) = AnthropicProvider::convert_messages(&messages);
        let sys = sys.unwrap();
        let json = serde_json::to_string(&sys).unwrap();
        assert_eq!(json, r#""short prompt""#);
    }

    #[test]
    fn convert_messages_large_system_returns_blocks_with_cache() {
        use crate::providers::traits::ChatMessage;
        let large_text = "x".repeat(3073);
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: large_text.clone(),
        }];
        let (sys, _) = AnthropicProvider::convert_messages(&messages);
        let sys = sys.unwrap();
        let json = serde_json::to_string(&sys).unwrap();
        assert!(json.contains("\"cache_control\":{\"type\":\"ephemeral\"}"));
    }

    #[test]
    fn native_content_out_text_no_cache_serializes_without_cache_field() {
        let block = NativeContentOut::Text {
            text: "hello".to_string(),
            cache_control: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(!json.contains("cache_control"));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn native_content_out_tool_result_with_cache_serializes() {
        let block = NativeContentOut::ToolResult {
            tool_use_id: "id-1".to_string(),
            content: "result".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"cache_control\":{\"type\":\"ephemeral\"}"));
    }

    #[test]
    fn apply_cache_to_last_message_on_text() {
        let mut messages = vec![NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        }];
        AnthropicProvider::apply_cache_to_last_message(&mut messages);
        match &messages[0].content[0] {
            NativeContentOut::Text { cache_control, .. } => {
                assert!(cache_control.is_some());
            }
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn apply_cache_to_last_message_on_tool_result() {
        let mut messages = vec![NativeMessage {
            role: "user".to_string(),
            content: vec![NativeContentOut::ToolResult {
                tool_use_id: "id-1".to_string(),
                content: "result".to_string(),
                cache_control: None,
            }],
        }];
        AnthropicProvider::apply_cache_to_last_message(&mut messages);
        match &messages[0].content[0] {
            NativeContentOut::ToolResult { cache_control, .. } => {
                assert!(cache_control.is_some());
            }
            _ => panic!("Expected ToolResult variant"),
        }
    }

    #[test]
    fn apply_cache_to_last_message_empty_noop() {
        let mut messages: Vec<NativeMessage> = Vec::new();
        // Should not panic
        AnthropicProvider::apply_cache_to_last_message(&mut messages);
    }

    #[test]
    fn native_tool_spec_no_cache_backward_compat() {
        let spec = NativeToolSpec {
            name: "my_tool".to_string(),
            description: "does stuff".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(!json.contains("cache_control"));
    }

    // ── OAuth / apply_auth tests ────────────────────────────────

    #[test]
    fn apply_auth_oat_token_uses_bearer_and_beta_header() {
        let provider = AnthropicProvider::new(Some("sk-ant-oat01-abc123"));
        let client = reqwest::Client::new();
        let request = client.post("https://api.anthropic.com/v1/messages");
        let request = provider.apply_auth(request, "sk-ant-oat01-abc123");
        let built = request.build().unwrap();
        let headers = built.headers();

        assert_eq!(
            headers.get("Authorization").unwrap(),
            "Bearer sk-ant-oat01-abc123"
        );
        assert_eq!(
            headers.get("anthropic-beta").unwrap(),
            "oauth-2025-04-20"
        );
        assert!(
            headers.get("x-api-key").is_none(),
            "OAT tokens should not use x-api-key"
        );
    }

    #[test]
    fn apply_auth_regular_key_uses_x_api_key_no_beta() {
        let provider = AnthropicProvider::new(Some("sk-ant-api03-regularkey"));
        let client = reqwest::Client::new();
        let request = client.post("https://api.anthropic.com/v1/messages");
        let request = provider.apply_auth(request, "sk-ant-api03-regularkey");
        let built = request.build().unwrap();
        let headers = built.headers();

        assert_eq!(
            headers.get("x-api-key").unwrap(),
            "sk-ant-api03-regularkey"
        );
        assert!(
            headers.get("Authorization").is_none(),
            "Regular API keys should not use Bearer auth"
        );
        assert!(
            headers.get("anthropic-beta").is_none(),
            "Regular API keys should not include oauth beta header"
        );
    }

    #[test]
    fn try_refresh_credential_skips_regular_api_keys() {
        let provider = AnthropicProvider::new(Some("sk-ant-api03-regularkey"));
        // Should not attempt refresh for non-OAT tokens
        assert!(!provider.try_refresh_credential());
        // Credential should remain unchanged
        assert_eq!(
            provider.credential.read().unwrap().as_deref(),
            Some("sk-ant-api03-regularkey")
        );
    }

    #[test]
    fn try_refresh_credential_skips_when_no_credential() {
        let provider = AnthropicProvider::new(None);
        assert!(!provider.try_refresh_credential());
    }

    #[test]
    fn get_credential_returns_stored_value() {
        let provider = AnthropicProvider::new(Some("sk-ant-oat01-test"));
        assert_eq!(provider.get_credential().unwrap(), "sk-ant-oat01-test");
    }

    #[test]
    fn get_credential_errors_when_none() {
        let provider = AnthropicProvider::new(None);
        assert!(provider.get_credential().is_err());
    }
}
