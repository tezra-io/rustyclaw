use super::agent::Agent;
use super::bus::{AgentBus, MessageKind};
use super::definition::{agents_dir_from_config, AgentDefinition, MemoryIsolation};
use super::dispatcher::NativeToolDispatcher;
use super::memory_loader::DefaultMemoryLoader;
use super::prompt::{PromptContext, PromptSection, SystemPromptBuilder};
use crate::config::Config;
use crate::memory::{self, Memory};
use crate::observability::{self, Observer};
use crate::providers::{self, Provider};
use crate::runtime;
use crate::security::SecurityPolicy;
use crate::tools;
use anyhow::Result;
use std::sync::Arc;

const AGENT_MSG_BUFFER: usize = 32;

/// Status of a running agent
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed(String),
}

/// Run a persistent agent with its own memory, tools, and bus receiver.
///
/// This function blocks until the agent receives a Shutdown message or the
/// daemon's shutdown signal fires. It creates a full `Agent` instance with
/// agent-specific memory isolation, filtered tools, and personality prompt.
pub async fn run_persistent_agent(
    definition: AgentDefinition,
    config: Config,
    bus: Arc<AgentBus>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let agents_dir = agents_dir_from_config(&config);
    let agent_name = definition.name.clone();

    // ── Memory (isolated per agent definition) ──
    let memory: Arc<dyn Memory> = match definition.memory {
        MemoryIsolation::Isolated => Arc::from(memory::create_agent_memory(
            &config.memory,
            &agents_dir,
            &agent_name,
            &definition.memory_backend,
            config.api_key.as_deref(),
        )?),
        MemoryIsolation::SharedRead => {
            let workspace_mem = Arc::from(memory::create_memory(
                &config.memory,
                &config.workspace_dir,
                config.api_key.as_deref(),
            )?);
            let agent_mem = Arc::from(memory::create_agent_memory(
                &config.memory,
                &agents_dir,
                &agent_name,
                &definition.memory_backend,
                config.api_key.as_deref(),
            )?);
            Arc::from(memory::CompositeMemory::new(workspace_mem, agent_mem))
        }
        MemoryIsolation::Shared => Arc::from(memory::create_memory(
            &config.memory,
            &config.workspace_dir,
            config.api_key.as_deref(),
        )?),
    };

    // ── Build Agent via existing Agent struct ──
    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime_adapter: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(if let Some(ref agent_autonomy) = definition.autonomy {
        SecurityPolicy::from_agent_config(agent_autonomy, &config.autonomy, &config.workspace_dir)
    } else {
        SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir)
    });

    let composio_key = if config.composio.enabled {
        config.composio.api_key.as_deref()
    } else {
        None
    };
    let composio_entity_id = if config.composio.enabled {
        Some(config.composio.entity_id.as_str())
    } else {
        None
    };

    let all_tools = tools::all_tools_with_runtime(
        &security,
        runtime_adapter,
        memory.clone(),
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        &config,
    );

    // Filter tools by allowed_tools
    let mut tools = filter_tools(all_tools, &definition.allowed_tools);

    // Upgrade DelegateTool with bus access so persistent agents can delegate to each other
    tools::inject_bus_into_delegate_tool(
        &mut tools,
        &config.agents,
        config.api_key.as_deref(),
        Arc::clone(&bus),
    );

    let provider_name = config.effective_provider();
    let effective_model = config.effective_model();
    let model_name = definition
        .model
        .as_deref()
        .or(config.default_model.as_deref())
        .unwrap_or(&effective_model)
        .to_string();
    let temperature = definition.temperature.unwrap_or(config.default_temperature);

    let provider: Box<dyn Provider> = providers::create_routed_provider(
        provider_name,
        config.api_key.as_deref(),
        &config.reliability,
        &config.model_routes,
        &model_name,
    )?;

    // Build prompt with agent personality section
    let mut prompt_builder = SystemPromptBuilder::with_defaults();
    if !definition.personality.is_empty() {
        prompt_builder = prompt_builder.add_section(Box::new(PersonalitySection {
            personality: definition.personality.clone(),
        }));
    }

    let dispatcher_choice = config.agent.tool_dispatcher.as_str();
    let tool_dispatcher: Box<dyn super::dispatcher::ToolDispatcher> = match dispatcher_choice {
        "native" => Box::new(NativeToolDispatcher),
        "xml" => Box::new(super::dispatcher::XmlToolDispatcher),
        _ if provider.supports_native_tools() => Box::new(NativeToolDispatcher),
        _ => Box::new(super::dispatcher::XmlToolDispatcher),
    };

    let mut agent_config = config.agent.clone();
    agent_config.max_tool_iterations = definition.max_tools_per_turn;

    let agent_skills = load_agent_skills(&definition.skills, &config.workspace_dir);

    let mut agent = Agent::builder()
        .provider(provider)
        .tools(tools)
        .memory(memory)
        .observer(observer)
        .tool_dispatcher(tool_dispatcher)
        .memory_loader(Box::new(DefaultMemoryLoader::new(5, config.memory.min_relevance_score)))
        .prompt_builder(prompt_builder)
        .config(agent_config)
        .model_name(model_name)
        .temperature(temperature)
        .workspace_dir(config.workspace_dir.clone())
        .identity_config(config.identity.clone())
        .skills(agent_skills)
        .build()?;

    // ── Register on bus ──
    let mut receiver = bus.register(&agent_name, AGENT_MSG_BUFFER).await;

    crate::health::mark_component_ok(&format!("agent:{agent_name}"));
    tracing::info!(agent = %agent_name, "Persistent agent started");

    // ── Message loop ──
    loop {
        tokio::select! {
            Some(msg) = receiver.recv() => {
                match msg.kind {
                    MessageKind::Delegate => {
                        tracing::info!(
                            agent = %agent_name,
                            from = %msg.from,
                            "Received delegation"
                        );

                        let response = agent.turn(&msg.payload).await;

                        if let Some(tx) = msg.response_tx {
                            let _ = tx.send(
                                response.unwrap_or_else(|e| format!("Error: {e}"))
                            );
                        }

                        // Clear history between delegations for clean context
                        agent.clear_history();
                    }
                    MessageKind::Shutdown => {
                        tracing::info!(agent = %agent_name, "Shutdown requested via bus");
                        break;
                    }
                    MessageKind::Notify => {
                        tracing::info!(
                            agent = %agent_name,
                            from = %msg.from,
                            "Notification: {}",
                            msg.payload
                        );
                    }
                    MessageKind::Result => {
                        tracing::debug!(
                            agent = %agent_name,
                            from = %msg.from,
                            "Result: {}",
                            crate::util::truncate_with_ellipsis(&msg.payload, 100)
                        );
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!(agent = %agent_name, "Daemon shutdown signal received");
                    break;
                }
            }
        }
    }

    bus.unregister(&agent_name).await;
    crate::health::mark_component_error(&format!("agent:{agent_name}"), "stopped");
    tracing::info!(agent = %agent_name, "Persistent agent stopped");
    Ok(())
}

/// Custom prompt section for agent personality
struct PersonalitySection {
    personality: String,
}

impl PromptSection for PersonalitySection {
    fn name(&self) -> &str {
        "personality"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        Ok(format!("## Agent Personality\n\n{}", self.personality))
    }
}

/// Load skills that match the agent's skill list.
pub(crate) fn load_agent_skills(
    skill_names: &[String],
    workspace_dir: &std::path::Path,
) -> Vec<crate::skills::Skill> {
    if skill_names.is_empty() {
        return Vec::new();
    }
    let all_skills = crate::skills::load_skills(workspace_dir);
    all_skills
        .into_iter()
        .filter(|s| skill_names.contains(&s.name))
        .collect()
}

/// Filter tools to only those in the allowed list.
/// Empty `allowed_tools` = all tools permitted.
pub(crate) fn filter_tools(
    all_tools: Vec<Box<dyn tools::Tool>>,
    allowed_tools: &[String],
) -> Vec<Box<dyn tools::Tool>> {
    if allowed_tools.is_empty() {
        return all_tools;
    }
    all_tools
        .into_iter()
        .filter(|t| allowed_tools.iter().any(|a| a == t.name()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_tools_empty_allows_all() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = tools::default_tools(security);
        let count = tools.len();
        let filtered = filter_tools(tools, &[]);
        assert_eq!(filtered.len(), count);
    }

    #[test]
    fn filter_tools_restricts() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = tools::default_tools(security);
        let filtered = filter_tools(tools, &["shell".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name(), "shell");
    }

    #[test]
    fn filter_tools_unknown_name_filters_all() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = tools::default_tools(security);
        let filtered = filter_tools(tools, &["nonexistent_tool".into()]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn load_agent_skills_empty_returns_empty() {
        let skills = load_agent_skills(&[], std::path::Path::new("/tmp"));
        assert!(skills.is_empty());
    }

    #[test]
    fn personality_section_builds() {
        let section = PersonalitySection {
            personality: "Be helpful.".into(),
        };
        let tools: Vec<Box<dyn tools::Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: std::path::Path::new("/tmp"),
            model_name: "test",
            tools: &tools,
            skills: &[],
            identity_config: None,
            dispatcher_instructions: "",
        };
        let output = section.build(&ctx).unwrap();
        assert!(output.contains("Agent Personality"));
        assert!(output.contains("Be helpful."));
    }
}
