use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::bus::events::InboundMessage;
use crate::config::schema::Config;
use crate::providers::base::LlmProvider;
use crate::tools::base::ToolRegistry;

/// Manages background subagent tasks.
pub struct SubagentManager {
    config: Arc<Config>,
    provider: Arc<dyn LlmProvider>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl SubagentManager {
    pub fn new(
        config: Arc<Config>,
        provider: Arc<dyn LlmProvider>,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self {
            config,
            provider,
            inbound_tx,
        }
    }

    /// Spawn a background subagent task.
    pub fn spawn(&self, task: String, context: Option<String>, origin_session: String) {
        let config = self.config.clone();
        let provider = self.provider.clone();
        let tx = self.inbound_tx.clone();

        tokio::spawn(async move {
            info!("Subagent started: {}", task);

            // Create a minimal tool registry (no message/spawn tools to prevent recursion)
            let registry = ToolRegistry::new();
            // TODO: Register filesystem, shell, web tools with workspace restriction

            let prompt = if let Some(ctx) = context {
                format!("{}\n\nContext: {}", task, ctx)
            } else {
                task.clone()
            };

            // Run the agent loop inline
            // TODO: Use AgentLoop::process_direct or equivalent

            let result = format!("Subagent completed task: {}", task);

            // Announce result back via the inbound bus as a system message
            let announcement = InboundMessage::new("system", "subagent", &origin_session, &result);
            if let Err(e) = tx.send(announcement).await {
                error!("Failed to announce subagent result: {}", e);
            }
        });
    }
}
