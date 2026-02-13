use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::info;

use crate::bus::queue::MessageBus;
use crate::config::schema::Config;
use crate::providers::openai_compat::OpenAiCompatProvider;
use crate::providers::registry::{find_provider_by_name, find_provider_for_model};
use crate::session::SessionManager;
use crate::tools::base::ToolRegistry;

/// RustyClaw — lightweight AI agent framework.
#[derive(Parser)]
#[command(name = "rustyclaw", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a single prompt or start interactive chat.
    Agent {
        /// The prompt to send (omit for interactive mode).
        #[arg(short, long)]
        message: Option<String>,

        /// Model override (e.g., "openrouter/anthropic/claude-sonnet-4-5").
        #[arg(short = 'M', long)]
        model: Option<String>,
    },

    /// Run the agent with all enabled channels.
    Run {
        /// Model override.
        #[arg(short = 'M', long)]
        model: Option<String>,
    },

    /// Show agent status and configuration.
    Status,

    /// Run interactive onboarding setup.
    Onboard,

    /// Manage channels.
    Channels {
        #[command(subcommand)]
        action: ChannelAction,
    },

    /// Manage cron jobs.
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },
}

#[derive(Subcommand)]
enum ChannelAction {
    /// Show channel status.
    Status,
    /// Login to a channel.
    Login { channel: String },
}

#[derive(Subcommand)]
enum CronAction {
    /// List all cron jobs.
    List,
    /// Add a new cron job.
    Add {
        name: String,
        schedule: String,
        prompt: String,
    },
    /// Remove a cron job.
    Remove { name: String },
    /// Run a cron job now.
    Run { name: String },
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        match self.command {
            Commands::Agent { message, model } => {
                let config = crate::config::load_config();

                let agent = build_agent_stack(&config, model.as_deref())?;

                match message {
                    Some(prompt) => {
                        // Single prompt mode
                        let session_key = format!("cli:{}", crate::utils::today_date());
                        let response = agent.process_direct(&prompt, &session_key).await?;
                        println!("{}", response);
                    }
                    None => {
                        // Interactive mode
                        run_interactive(agent).await?;
                    }
                }

                Ok(())
            }

            Commands::Run { model } => {
                let config = crate::config::load_config();
                let config = Arc::new(config);

                let bus = Arc::new(MessageBus::new(256));

                let provider_name =
                    config.get_provider_name(model.as_deref()).ok_or_else(|| {
                        anyhow::anyhow!(
                            "No provider configured. Set an API key via env var or config.json."
                        )
                    })?;
                let provider_config = config
                    .get_provider(model.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("No provider configured"))?;
                let spec = find_provider_by_name(provider_name)
                    .ok_or_else(|| anyhow::anyhow!("Unknown provider: {}", provider_name))?;

                let provider: Arc<dyn crate::providers::LlmProvider> =
                    Arc::new(OpenAiCompatProvider::new(
                        spec,
                        provider_config.api_key.clone(),
                        config.get_api_base(model.as_deref()),
                        model.clone(),
                        provider_config.extra_headers.clone(),
                    ));

                let data_dir = crate::config::get_data_dir();
                let sessions = Arc::new(SessionManager::new(data_dir));

                let registry = build_tool_registry(&config, Some(&bus));

                let agent = Arc::new(crate::agent::AgentLoop::new(
                    config.clone(),
                    bus.clone(),
                    provider.clone(),
                    sessions.clone(),
                    registry,
                ));

                // Start channels
                let channel_mgr = crate::channels::ChannelManager::new(config.clone(), bus.clone());

                info!("RustyClaw running with all enabled channels. Press Ctrl+C to stop.");

                // Run agent loop and channels concurrently
                let agent_handle = {
                    let agent = agent.clone();
                    tokio::spawn(async move { agent.run().await })
                };
                let channel_handle = tokio::spawn(async move { channel_mgr.start_all().await });

                tokio::select! {
                    _ = agent_handle => {}
                    _ = channel_handle => {}
                    _ = tokio::signal::ctrl_c() => {
                        info!("Shutting down...");
                    }
                }

                Ok(())
            }

            Commands::Status => {
                let config = crate::config::load_config();
                println!("RustyClaw v{}", crate::VERSION);
                println!("Model:     {}", config.agents.defaults.model);
                println!("Workspace: {}", config.agents.defaults.workspace);
                println!("Config:    {}", crate::config::get_config_path().display());

                // Provider status
                if let Some(name) = config.get_provider_name(None) {
                    println!("Provider:  {} (configured)", name);
                } else {
                    println!("Provider:  (none configured — set an API key)");
                }

                // Channel status
                println!("\nChannels:");
                println!(
                    "  Telegram: {}",
                    if config.channels.telegram.enabled
                        && !config.channels.telegram.token.is_empty()
                    {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                println!(
                    "  Discord:  {}",
                    if config.channels.discord.enabled && !config.channels.discord.token.is_empty()
                    {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );

                Ok(())
            }

            Commands::Onboard => {
                println!("Welcome to RustyClaw! Let's set things up.\n");

                let mut config = crate::config::load_config();

                // Interactive setup using rustyline
                let mut rl = rustyline::DefaultEditor::new()
                    .map_err(|e| anyhow::anyhow!("Failed to init editor: {}", e))?;

                // Ask for model
                let model = rl
                    .readline(&format!(
                        "Default model [{}]: ",
                        config.agents.defaults.model
                    ))
                    .unwrap_or_default();
                if !model.trim().is_empty() {
                    config.agents.defaults.model = model.trim().to_string();
                }

                // Detect provider from model name (without requiring key to be set)
                if let Some(spec) = find_provider_for_model(&config.agents.defaults.model) {
                    let current_key = config
                        .providers
                        .by_name(spec.name)
                        .map(|p| &p.api_key)
                        .filter(|k| !k.is_empty());
                    let hint = if current_key.is_some() {
                        " (already set, press Enter to keep)"
                    } else {
                        ""
                    };
                    let key = rl
                        .readline(&format!(
                            "{} API key ({}){}: ",
                            spec.name, spec.env_key, hint
                        ))
                        .unwrap_or_default();
                    if !key.trim().is_empty() {
                        set_provider_key(&mut config, spec.name, key.trim());
                    }
                } else {
                    // No provider detected from model — ask for a generic API key
                    println!(
                        "Could not detect provider from model name '{}'.",
                        config.agents.defaults.model
                    );
                    println!("You can set an API key manually in the config file.");
                }

                // Workspace
                let workspace = rl
                    .readline(&format!(
                        "Workspace path [{}]: ",
                        config.agents.defaults.workspace
                    ))
                    .unwrap_or_default();
                if !workspace.trim().is_empty() {
                    config.agents.defaults.workspace = workspace.trim().to_string();
                }

                // Ensure workspace exists
                let ws_path = config.workspace_path();
                std::fs::create_dir_all(&ws_path).ok();

                // Create workspace template files (AGENTS.md, SOUL.md, USER.md, etc.)
                create_workspace_templates(&ws_path);

                // Save
                crate::config::save_config(&config)
                    .map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))?;

                println!(
                    "\nConfiguration saved to {}",
                    crate::config::get_config_path().display()
                );
                println!("\nNext steps:");
                if config.get_provider_name(None).is_none() {
                    if let Some(spec) = find_provider_for_model(&config.agents.defaults.model) {
                        println!("  1. Set your API key: export {}=your-key", spec.env_key);
                    } else {
                        println!("  1. Set an API key in ~/.rustyclaw/config.json");
                    }
                    println!("  2. Test: rustyclaw agent -m 'Hello'");
                } else {
                    println!("  Run: rustyclaw agent -m 'Hello'");
                }

                Ok(())
            }

            Commands::Channels { action } => {
                let config = crate::config::load_config();
                match action {
                    ChannelAction::Status => {
                        println!("Channel status:");
                        println!(
                            "  Telegram: {}",
                            if config.channels.telegram.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        );
                        println!(
                            "  Discord:  {}",
                            if config.channels.discord.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        );
                    }
                    ChannelAction::Login { channel } => {
                        println!("Login flow for {} (not yet implemented)", channel);
                    }
                }
                Ok(())
            }

            Commands::Cron { action } => {
                let data_dir = crate::config::get_data_dir();
                let mut cron_svc = crate::cron::CronService::new(data_dir);

                match action {
                    CronAction::List => {
                        let jobs = cron_svc.list();
                        if jobs.is_empty() {
                            println!("No cron jobs configured.");
                        } else {
                            for job in jobs {
                                println!("  {} — {:?}", job.name, job.schedule);
                            }
                        }
                    }
                    CronAction::Add {
                        name,
                        schedule,
                        prompt,
                    } => {
                        let job = crate::cron::CronJob {
                            name: name.clone(),
                            schedule: crate::cron::CronSchedule::Cron {
                                expression: schedule,
                            },
                            payload: crate::cron::types::CronPayload {
                                prompt,
                                channel: None,
                                chat_id: None,
                            },
                            state: crate::cron::types::CronJobState::default(),
                        };
                        cron_svc.add(job)?;
                        println!("Added cron job: {}", name);
                    }
                    CronAction::Remove { name } => {
                        if cron_svc.remove(&name)? {
                            println!("Removed cron job: {}", name);
                        } else {
                            println!("Cron job not found: {}", name);
                        }
                    }
                    CronAction::Run { name } => {
                        println!("Running cron job: {} (not yet implemented)", name);
                    }
                }
                Ok(())
            }
        }
    }
}

/// Build the full agent stack for CLI usage.
fn build_agent_stack(
    config: &Config,
    model: Option<&str>,
) -> anyhow::Result<crate::agent::AgentLoop> {
    let config = Arc::new(config.clone());

    // Resolve provider
    let provider_name = config.get_provider_name(model).ok_or_else(|| {
        let effective_model = model.unwrap_or(&config.agents.defaults.model);
        let hint = if let Some(spec) = find_provider_for_model(effective_model) {
            format!("Set an API key: export {}=your-key\n", spec.env_key)
        } else {
            "Set an API key for a supported provider.\n".to_string()
        };
        anyhow::anyhow!(
            "No LLM provider configured.\n\
             {}\
             Or run: rustyclaw onboard",
            hint
        )
    })?;

    let provider_config = config.get_provider(model).unwrap();
    let spec = find_provider_by_name(provider_name).unwrap();

    info!(
        "Using provider: {} (model: {})",
        provider_name,
        model.unwrap_or(&config.agents.defaults.model)
    );

    let provider: Arc<dyn crate::providers::LlmProvider> = Arc::new(OpenAiCompatProvider::new(
        spec,
        provider_config.api_key.clone(),
        config.get_api_base(model),
        model.map(String::from),
        provider_config.extra_headers.clone(),
    ));

    let data_dir = crate::config::get_data_dir();
    let sessions = Arc::new(SessionManager::new(data_dir));

    let bus = Arc::new(MessageBus::new(64));
    let registry = build_tool_registry(&config, Some(&bus));

    let agent = crate::agent::AgentLoop::new(config.clone(), bus, provider, sessions, registry);

    Ok(agent)
}

/// Build a tool registry with all Phase 1 tools registered.
fn build_tool_registry(config: &Config, bus: Option<&MessageBus>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    let workspace = if config.tools.restrict_to_workspace {
        Some(config.workspace_path())
    } else {
        None
    };

    // Filesystem tools
    registry.register(Arc::new(crate::tools::filesystem::ReadFileTool {
        allowed_dir: workspace.clone(),
    }));
    registry.register(Arc::new(crate::tools::filesystem::WriteFileTool {
        allowed_dir: workspace.clone(),
    }));
    registry.register(Arc::new(crate::tools::filesystem::EditFileTool {
        allowed_dir: workspace.clone(),
    }));
    registry.register(Arc::new(crate::tools::filesystem::ListDirTool {
        allowed_dir: workspace.clone(),
    }));

    // Shell exec
    registry.register(Arc::new(crate::tools::shell::ExecTool {
        allowed_dir: workspace.clone(),
        timeout_secs: config.tools.exec.timeout,
    }));

    // Web tools
    registry.register(Arc::new(crate::tools::web::WebSearchTool {
        api_key: config.tools.web.search.api_key.clone(),
        max_results: config.tools.web.search.max_results,
    }));
    registry.register(Arc::new(crate::tools::web::WebFetchTool));

    // Message tool (if bus is available)
    if let Some(bus) = bus {
        registry.register(Arc::new(crate::tools::message::MessageTool::new(
            bus.outbound_sender(),
        )));
    }

    registry
}

/// Run interactive chat mode using rustyline.
async fn run_interactive(agent: crate::agent::AgentLoop) -> anyhow::Result<()> {
    let mut rl = rustyline::DefaultEditor::new()
        .map_err(|e| anyhow::anyhow!("Failed to init editor: {}", e))?;

    let session_key = format!("cli:{}", crate::utils::today_date());
    println!("RustyClaw interactive mode. Type 'exit' or Ctrl+D to quit.\n");

    loop {
        match rl.readline("you> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "exit" || line == "quit" {
                    break;
                }
                if line == "/reset" {
                    // Clear session handled by SessionManager
                    println!("(session cleared)");
                    continue;
                }

                rl.add_history_entry(line).ok();

                match agent.process_direct(line, &session_key).await {
                    Ok(response) => {
                        println!("\nassistant> {}\n", response);
                    }
                    Err(e) => {
                        eprintln!("Error: {}\n", e);
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("(Ctrl+C to interrupt, type 'exit' to quit)");
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("Input error: {}", e);
                break;
            }
        }
    }

    println!("Goodbye!");
    Ok(())
}

/// Set a provider API key in the config struct by provider name.
fn set_provider_key(config: &mut Config, name: &str, key: &str) {
    match name {
        "anthropic" => config.providers.anthropic.api_key = key.to_string(),
        "openai" => config.providers.openai.api_key = key.to_string(),
        "openrouter" => config.providers.openrouter.api_key = key.to_string(),
        "deepseek" => config.providers.deepseek.api_key = key.to_string(),
        "groq" => config.providers.groq.api_key = key.to_string(),
        "zhipu" => config.providers.zhipu.api_key = key.to_string(),
        "dashscope" => config.providers.dashscope.api_key = key.to_string(),
        "vllm" => config.providers.vllm.api_key = key.to_string(),
        "gemini" => config.providers.gemini.api_key = key.to_string(),
        "moonshot" => config.providers.moonshot.api_key = key.to_string(),
        "minimax" => config.providers.minimax.api_key = key.to_string(),
        "aihubmix" => config.providers.aihubmix.api_key = key.to_string(),
        _ => {}
    }
}

/// Create default workspace template files matching nanobot's onboarding.
fn create_workspace_templates(workspace: &std::path::Path) {
    let templates: &[(&str, &str)] = &[
        (
            "AGENTS.md",
            "# Agent Instructions\n\n\
             You are a helpful AI assistant. Be concise, accurate, and friendly.\n\n\
             ## Guidelines\n\n\
             - Always explain what you're doing before taking actions\n\
             - Ask for clarification when the request is ambiguous\n\
             - Use tools to help accomplish tasks\n\
             - Remember important information in your memory files\n",
        ),
        (
            "SOUL.md",
            "# Soul\n\n\
             I am RustyClaw, a lightweight AI assistant.\n\n\
             ## Personality\n\n\
             - Helpful and friendly\n\
             - Concise and to the point\n\
             - Curious and eager to learn\n\n\
             ## Values\n\n\
             - Accuracy over speed\n\
             - User privacy and safety\n\
             - Transparency in actions\n",
        ),
        (
            "USER.md",
            "# User\n\n\
             Information about the user goes here.\n\n\
             ## Preferences\n\n\
             - Communication style: (casual/formal)\n\
             - Timezone: (your timezone)\n\
             - Language: (your preferred language)\n",
        ),
    ];

    for (filename, content) in templates {
        let path = workspace.join(filename);
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("  Warning: couldn't create {}: {}", filename, e);
            } else {
                println!("  Created {}", filename);
            }
        }
    }

    // Create memory directory
    let memory_dir = workspace.join("memory");
    std::fs::create_dir_all(&memory_dir).ok();
    let memory_file = memory_dir.join("MEMORY.md");
    if !memory_file.exists() {
        std::fs::write(
            &memory_file,
            "# Long-term Memory\n\n\
             This file stores important information that should persist across sessions.\n",
        )
        .ok();
        println!("  Created memory/MEMORY.md");
    }
}
