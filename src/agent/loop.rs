use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::bus::events::{InboundMessage, OutboundMessage};
use crate::bus::queue::MessageBus;
use crate::config::schema::Config;
use crate::providers::base::{ChatMessage, LlmProvider};
use crate::session::SessionManager;
use crate::tools::base::ToolRegistry;

/// Core agent loop: receives messages → builds context → calls LLM → executes tools → responds.
pub struct AgentLoop {
    agent_name: String,
    config: Arc<Config>,
    bus: Arc<MessageBus>,
    provider: Arc<dyn LlmProvider>,
    sessions: Arc<SessionManager>,
    registry: ToolRegistry,
    max_iterations: u32,
}

impl AgentLoop {
    pub fn new(
        config: Arc<Config>,
        bus: Arc<MessageBus>,
        provider: Arc<dyn LlmProvider>,
        sessions: Arc<SessionManager>,
        registry: ToolRegistry,
    ) -> Self {
        let max_iterations = config.agents.defaults.max_tool_iterations;
        Self {
            agent_name: "master".to_string(),
            config,
            bus,
            provider,
            sessions,
            registry,
            max_iterations,
        }
    }

    /// Set the agent name (for logging and tracing).
    pub fn with_name(mut self, name: &str) -> Self {
        self.agent_name = name.to_string();
        self
    }

    /// Run the main loop, consuming inbound messages from the shared bus.
    /// This is the convenience method used by the master agent.
    pub async fn run(&self) {
        info!(agent = %self.agent_name, "Agent loop started");
        while let Some(msg) = self.bus.consume_inbound().await {
            debug!(agent = %self.agent_name, "Processing message from {}:{}", msg.channel, msg.chat_id);
            if let Err(e) = self.handle_message(msg).await {
                error!(agent = %self.agent_name, "Error handling message: {}", e);
            }
        }
        info!(agent = %self.agent_name, "Agent loop stopped");
    }

    /// Run the loop consuming messages from a dedicated receiver.
    /// Used by worker agents that have their own per-agent channel.
    pub async fn run_with_receiver(&self, mut rx: mpsc::Receiver<InboundMessage>) {
        info!(agent = %self.agent_name, "Agent loop started (dedicated channel)");
        while let Some(msg) = rx.recv().await {
            debug!(agent = %self.agent_name, "Processing message from {}:{}", msg.channel, msg.chat_id);
            if let Err(e) = self.handle_message(msg).await {
                error!(agent = %self.agent_name, "Error handling message: {}", e);
            }
        }
        info!(agent = %self.agent_name, "Agent loop stopped (dedicated channel)");
    }

    /// Handle a single inbound message.
    async fn handle_message(&self, msg: InboundMessage) -> crate::error::Result<()> {
        let session_key = msg.session_key();

        // Get conversation history
        let history = self.sessions.get_history(&session_key, 50).await;

        // Build context
        let context = crate::agent::context::ContextBuilder::new(&self.config)
            .with_history(history)
            .build();

        // Add user message
        let mut messages = context;
        messages.push(ChatMessage {
            role: "user".into(),
            content: serde_json::Value::String(msg.content.clone()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        // Tool loop
        let mut iterations = 0;
        loop {
            if iterations >= self.max_iterations {
                warn!(agent = %self.agent_name, "Max tool iterations ({}) reached", self.max_iterations);
                break;
            }

            let tools = self.registry.definitions();
            let response = self
                .provider
                .chat(
                    messages.clone(),
                    Some(tools),
                    None,
                    Some(self.config.agents.defaults.temperature),
                    Some(self.config.agents.defaults.max_tokens),
                )
                .await?;

            // If there are tool calls, execute them
            if !response.tool_calls.is_empty() {
                // Add assistant message with tool calls
                messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: response
                        .content
                        .clone()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                    tool_calls: Some(response.tool_calls.clone()),
                    tool_call_id: None,
                    name: None,
                });

                // Execute each tool call
                for tc in &response.tool_calls {
                    let result = self.registry.execute(&tc.name, tc.arguments.clone()).await;
                    let result_str = match result {
                        Ok(s) => s,
                        Err(e) => format!("Error: {}", e),
                    };

                    messages.push(ChatMessage {
                        role: "tool".into(),
                        content: serde_json::Value::String(result_str),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.name.clone()),
                    });
                }

                iterations += 1;
            } else {
                // Final text response — send to channel
                if let Some(text) = &response.content {
                    let outbound = OutboundMessage::new(&msg.channel, &msg.chat_id, text);
                    self.bus.publish_outbound(outbound).await;
                }
                break;
            }
        }

        // Save conversation to session
        self.sessions
            .append(&session_key, "user", &msg.content)
            .await;
        // The last assistant response is the final message sent

        Ok(())
    }

    /// Process a direct prompt (for CLI or cron) without the message bus.
    pub async fn process_direct(
        &self,
        prompt: &str,
        session_key: &str,
    ) -> crate::error::Result<String> {
        let history = self.sessions.get_history(session_key, 50).await;
        let mut messages = crate::agent::context::ContextBuilder::new(&self.config)
            .with_history(history)
            .build();

        messages.push(ChatMessage {
            role: "user".into(),
            content: serde_json::Value::String(prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let mut iterations = 0;
        let mut last_content = String::new();

        loop {
            if iterations >= self.max_iterations {
                break;
            }

            let tools = self.registry.definitions();
            let response = self
                .provider
                .chat(messages.clone(), Some(tools), None, None, None)
                .await?;

            if !response.tool_calls.is_empty() {
                messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: response
                        .content
                        .clone()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                    tool_calls: Some(response.tool_calls.clone()),
                    tool_call_id: None,
                    name: None,
                });

                for tc in &response.tool_calls {
                    let result = self.registry.execute(&tc.name, tc.arguments.clone()).await;
                    let result_str = match result {
                        Ok(s) => s,
                        Err(e) => format!("Error: {}", e),
                    };
                    messages.push(ChatMessage {
                        role: "tool".into(),
                        content: serde_json::Value::String(result_str),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.name.clone()),
                    });
                }
                iterations += 1;
            } else {
                last_content = response.content.unwrap_or_default();
                break;
            }
        }

        self.sessions.append(session_key, "user", prompt).await;
        self.sessions
            .append(session_key, "assistant", &last_content)
            .await;

        Ok(last_content)
    }
}
