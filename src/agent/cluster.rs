use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::agent::context::ContextBuilder;
use crate::agent::definition::load_all_agents;
use crate::agent::router::AgentRouter;
use crate::agent::supervisor::AgentSupervisor;
use crate::agent::AgentDefinition;
use crate::bus::events::{AgentMessage, AgentMessageType, InboundMessage, OutboundMessage};
use crate::bus::queue::MessageBus;
use crate::config::schema::Config;
use crate::providers::base::{ChatMessage, LlmProvider};
use crate::session::SessionManager;
use crate::tools::base::ToolRegistry;

/// Main orchestration struct: master agent + supervised workers.
pub struct AgentCluster {
    config: Arc<Config>,
    bus: Arc<MessageBus>,
    provider: Arc<dyn LlmProvider>,
    sessions: Arc<SessionManager>,
    registry: ToolRegistry,
    supervisor: Arc<AgentSupervisor>,
    router: AgentRouter,
    agents: Vec<AgentDefinition>,
}

impl AgentCluster {
    pub fn new(
        config: Arc<Config>,
        bus: Arc<MessageBus>,
        provider: Arc<dyn LlmProvider>,
        sessions: Arc<SessionManager>,
        registry: ToolRegistry,
    ) -> Self {
        let supervisor = Arc::new(AgentSupervisor::new(bus.clone()));
        let router = AgentRouter::new();

        Self {
            config,
            bus,
            provider,
            sessions,
            registry,
            supervisor,
            router,
            agents: Vec::new(),
        }
    }

    /// Load agent definitions, register workers on the bus, start the cluster.
    pub async fn start(&mut self) {
        // Load agent definitions
        let (agents, warnings) = load_all_agents();
        for w in &warnings {
            warn!("Agent definition warning: {}", w);
        }

        self.agents = agents.clone();

        // Register agents with the router
        for agent in &self.agents {
            self.router.register_agent(agent);
        }

        // Register each worker on the message bus and spawn their loops
        for agent in &self.agents {
            let agent_rx = self.bus.register_agent(&agent.name).await;
            self.spawn_worker(agent.clone(), agent_rx).await;
        }

        info!(
            agent_count = self.agents.len(),
            "AgentCluster started with {} worker agents",
            self.agents.len()
        );

        // Start the supervision loop in the background
        let supervisor = self.supervisor.clone();
        let bus_clone = self.bus.clone();
        let config_clone = self.config.clone();
        let provider_clone = self.provider.clone();
        let sessions_clone = self.sessions.clone();
        let registry = &self.registry;
        let agents_for_spawn = self.agents.clone();

        // Build a spawn function for supervisor restarts
        let spawn_fn = {
            let bus = bus_clone.clone();
            let config = config_clone.clone();
            let provider = provider_clone.clone();
            let sessions = sessions_clone.clone();
            let agents = agents_for_spawn.clone();
            let scoped_tools: std::collections::HashMap<String, ToolRegistry> = agents
                .iter()
                .map(|a| {
                    let scoped = if let Some(ref tools) = a.tools {
                        let tool_refs: Vec<&str> = tools.iter().map(|s| s.as_str()).collect();
                        registry.scoped(&tool_refs)
                    } else {
                        ToolRegistry::new()
                    };
                    (a.name.clone(), scoped)
                })
                .collect();

            Arc::new(move |name: String| {
                let agent = agents.iter().find(|a| a.name == name).cloned();
                let bus = bus.clone();
                let config = config.clone();
                let provider = provider.clone();
                let sessions = sessions.clone();
                // Re-create scoped registry for this agent
                let scoped = scoped_tools
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(ToolRegistry::new);

                tokio::spawn(async move {
                    if let Some(agent_def) = agent {
                        // Re-register on bus to get a new receiver
                        let rx = bus.register_agent(&agent_def.name).await;
                        run_worker(agent_def, rx, config, bus, provider, sessions, scoped).await;
                    }
                })
            })
        };

        tokio::spawn(async move {
            supervisor.supervise(spawn_fn).await;
        });

        // Run the master loop
        self.run_master_loop().await;
    }

    /// Spawn a worker agent task.
    async fn spawn_worker(&self, agent: AgentDefinition, rx: mpsc::Receiver<AgentMessage>) {
        let name = agent.name.clone();
        let config = self.config.clone();
        let bus = self.bus.clone();
        let provider = self.provider.clone();
        let sessions = self.sessions.clone();

        // Scope tools for this worker
        let scoped_registry = if let Some(ref tools) = agent.tools {
            let tool_refs: Vec<&str> = tools.iter().map(|s| s.as_str()).collect();
            self.registry.scoped(&tool_refs)
        } else {
            ToolRegistry::new()
        };

        let handle = tokio::spawn(async move {
            run_worker(agent, rx, config, bus, provider, sessions, scoped_registry).await;
        });

        self.supervisor.register(&name, handle).await;
    }

    /// The master loop: select on user inbound + worker messages.
    async fn run_master_loop(&self) {
        info!(agent = "master", "Master agent loop started");

        loop {
            tokio::select! {
                // User/channel messages
                Some(msg) = self.bus.consume_inbound() => {
                    self.handle_inbound(msg).await;
                }
                // Worker messages to master
                Some(agent_msg) = self.bus.consume_master() => {
                    self.handle_worker_message(agent_msg).await;
                }
            }
        }
    }

    /// Handle an inbound user message: route to agent or handle directly.
    async fn handle_inbound(&self, msg: InboundMessage) {
        // Try routing to an agent
        if let Some(route) = self.router.route(&msg.content, &self.agents) {
            info!(
                agent = "master",
                target = %route.agent_name,
                confidence = route.confidence,
                "Delegating to agent"
            );

            let agent_msg = AgentMessage::new(
                "master",
                &route.agent_name,
                AgentMessageType::Task,
                &msg.content,
            );

            // Store the original message metadata so we can respond on the right channel
            // We use the trace_id to correlate the response back
            let trace_id = agent_msg.trace_id.clone();
            self.sessions
                .append(
                    &format!("delegation:{}", trace_id),
                    "metadata",
                    &serde_json::json!({
                        "channel": msg.channel,
                        "chat_id": msg.chat_id,
                    })
                    .to_string(),
                )
                .await;

            if let Err(e) = self.bus.send_to_agent(&route.agent_name, agent_msg).await {
                error!("Failed to delegate to agent {}: {}", route.agent_name, e);
                // Fallback: handle directly
                self.handle_directly(msg).await;
            }
        } else {
            // No agent match — master handles directly
            self.handle_directly(msg).await;
        }
    }

    /// Master handles the message directly via its own LLM call.
    async fn handle_directly(&self, msg: InboundMessage) {
        let session_key = msg.session_key();
        let history = self.sessions.get_history(&session_key, 50).await;

        let context = ContextBuilder::new(&self.config)
            .with_history(history)
            .build();

        let mut messages = context;
        messages.push(ChatMessage {
            role: "user".into(),
            content: serde_json::Value::String(msg.content.clone()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let tools = self.registry.definitions();
        match self
            .provider
            .chat(
                messages,
                Some(tools),
                None,
                Some(self.config.agents.defaults.temperature),
                Some(self.config.agents.defaults.max_tokens),
            )
            .await
        {
            Ok(response) => {
                if let Some(text) = &response.content {
                    let outbound = OutboundMessage::new(&msg.channel, &msg.chat_id, text);
                    self.bus.publish_outbound(outbound).await;
                }
            }
            Err(e) => {
                error!(agent = "master", "LLM call failed: {}", e);
                let outbound = OutboundMessage::new(
                    &msg.channel,
                    &msg.chat_id,
                    &format!("Sorry, I encountered an error: {}", e),
                );
                self.bus.publish_outbound(outbound).await;
            }
        }

        self.sessions
            .append(&session_key, "user", &msg.content)
            .await;
    }

    /// Handle a message from a worker agent.
    async fn handle_worker_message(&self, msg: AgentMessage) {
        match msg.msg_type {
            AgentMessageType::Result => {
                info!(
                    agent = "master",
                    from = %msg.from,
                    trace_id = %msg.trace_id,
                    "Received result from worker"
                );

                // Look up the original channel/chat_id from delegation metadata
                let delegation_key = format!("delegation:{}", msg.trace_id);
                let metadata = self.sessions.get_history(&delegation_key, 1).await;

                if let Some(meta_msg) = metadata.first() {
                    if let Some(meta_str) = meta_msg.content.as_str() {
                        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_str) {
                            let channel = meta["channel"].as_str().unwrap_or("cli");
                            let chat_id = meta["chat_id"].as_str().unwrap_or("0");

                            let outbound = OutboundMessage::new(channel, chat_id, &msg.content);
                            self.bus.publish_outbound(outbound).await;
                            return;
                        }
                    }
                }

                // Fallback: log if we can't route the response
                warn!(
                    "Cannot route worker result (trace_id {}): no delegation metadata found",
                    msg.trace_id
                );
            }

            AgentMessageType::Alert => {
                warn!(
                    agent = "master",
                    from = %msg.from,
                    "Worker alert: {}", msg.content
                );
            }

            AgentMessageType::Query => {
                info!(
                    agent = "master",
                    from = %msg.from,
                    "Worker query: {}", msg.content
                );
                // For now, reply with a simple acknowledgment
                let reply = msg.reply(AgentMessageType::Result, "Query received by master.");
                if let Err(e) = self.bus.send_to_agent(&msg.from, reply).await {
                    error!("Failed to reply to worker query: {}", e);
                }
            }

            _ => {
                warn!(
                    agent = "master",
                    from = %msg.from,
                    "Unexpected message type from worker: {:?}", msg.msg_type
                );
            }
        }
    }

    /// Graceful shutdown: stop all workers, then master.
    pub async fn shutdown(&self) {
        info!("AgentCluster shutting down...");

        // Send shutdown to all workers via bus
        for agent in &self.agents {
            let shutdown_msg =
                AgentMessage::new("master", &agent.name, AgentMessageType::Shutdown, "");
            if let Err(e) = self.bus.send_to_agent(&agent.name, shutdown_msg).await {
                warn!("Failed to send shutdown to {}: {}", agent.name, e);
            }
        }

        // Give workers a moment to finish
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Force shutdown via supervisor
        self.supervisor.shutdown_all().await;

        info!("AgentCluster shut down complete");
    }

    /// Get a reference to the supervisor for CLI commands.
    pub fn supervisor(&self) -> &Arc<AgentSupervisor> {
        &self.supervisor
    }

    /// Get loaded agent definitions.
    pub fn agents(&self) -> &[AgentDefinition] {
        &self.agents
    }
}

/// Worker agent runtime: receives tasks from master, runs LLM, sends results back.
async fn run_worker(
    agent: AgentDefinition,
    mut rx: mpsc::Receiver<AgentMessage>,
    config: Arc<Config>,
    bus: Arc<MessageBus>,
    provider: Arc<dyn LlmProvider>,
    sessions: Arc<SessionManager>,
    registry: ToolRegistry,
) {
    let name = agent.name.clone();
    info!(agent = %name, "Worker agent started");

    while let Some(msg) = rx.recv().await {
        match msg.msg_type {
            AgentMessageType::Shutdown => {
                info!(agent = %name, "Received shutdown signal");
                break;
            }

            AgentMessageType::Task => {
                info!(
                    agent = %name,
                    trace_id = %msg.trace_id,
                    "Processing task"
                );

                let session_key = format!("agent:{}:{}", name, msg.trace_id);
                let history = sessions.get_history(&session_key, 50).await;

                let context = ContextBuilder::new(&config)
                    .for_agent(&agent)
                    .with_history(history)
                    .build();

                let mut messages = context;
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: serde_json::Value::String(msg.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });

                let max_iterations = config.agents.defaults.max_tool_iterations;
                let mut iterations = 0;
                let mut final_content = String::new();

                loop {
                    if iterations >= max_iterations {
                        warn!(agent = %name, "Max tool iterations reached");
                        break;
                    }

                    let tools = registry.definitions();
                    match provider
                        .chat(
                            messages.clone(),
                            Some(tools),
                            None,
                            Some(config.agents.defaults.temperature),
                            Some(config.agents.defaults.max_tokens),
                        )
                        .await
                    {
                        Ok(response) => {
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
                                    let result =
                                        registry.execute(&tc.name, tc.arguments.clone()).await;
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
                                final_content = response.content.unwrap_or_default();
                                break;
                            }
                        }
                        Err(e) => {
                            error!(agent = %name, "LLM call failed: {}", e);
                            final_content = format!("Error from agent '{}': {}", name, e);
                            break;
                        }
                    }
                }

                // Send result back to master
                let mut result_msg = msg.reply(AgentMessageType::Result, &final_content);
                result_msg.from = name.clone();
                result_msg.to = "master".to_string();

                if let Err(e) = bus.send_to_master(result_msg).await {
                    error!(agent = %name, "Failed to send result to master: {}", e);
                }
            }

            _ => {
                warn!(agent = %name, "Unexpected message type: {:?}", msg.msg_type);
            }
        }
    }

    info!(agent = %name, "Worker agent stopped");
}

#[cfg(test)]
mod tests {
    #[test]
    fn agent_cluster_creates() {
        // Verify struct can be constructed (compile-time check mostly)
        // Full integration tests require mock provider
    }
}
