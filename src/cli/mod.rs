mod init;

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
    Chat {
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

    /// Show global status dashboard.
    Status,

    /// Run interactive onboarding setup.
    Init,

    /// Manage worker agents.
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },

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

    /// Validate config, API keys, agents, and ledger integrity.
    Doctor,
}

#[derive(Subcommand)]
enum ChannelAction {
    /// Show channel status.
    Status,
    /// Login to a channel.
    Login { channel: String },
}

#[derive(Subcommand)]
enum AgentAction {
    /// List all agent definitions.
    List,
    /// Show detailed status for an agent.
    Status {
        /// Agent name.
        name: String,
    },
    /// Validate agent definitions.
    Validate {
        /// Agent name (or omit for all).
        name: Option<String>,
    },
    /// Show recent logs for an agent.
    Logs {
        /// Agent name.
        name: String,
        /// Filter by log level (error, warn, info, debug).
        #[arg(long)]
        level: Option<String>,
        /// Max lines to show.
        #[arg(long, default_value = "50")]
        lines: usize,
    },
    /// Add a new agent (interactive template selection).
    Add,
    /// Edit an agent definition in $EDITOR.
    Edit {
        /// Agent name.
        name: String,
    },
    /// Remove an agent definition.
    Remove {
        /// Agent name.
        name: String,
    },
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
            Commands::Chat { message, model } => {
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

                // Agent definitions
                let (agents, _warnings) = crate::agent::definition::load_all_agents();
                if agents.is_empty() {
                    println!("\nAgents:    (none defined)");
                } else {
                    println!("\nAgents:");
                    println!("  {:<20} {:<15} DESCRIPTION", "NAME", "MODEL");
                    for agent in &agents {
                        let model = agent.model.as_deref().unwrap_or("inherit");
                        let desc = if agent.description.len() > 50 {
                            format!("{}...", &agent.description[..47])
                        } else {
                            agent.description.clone()
                        };
                        println!("  {:<20} {:<15} {}", agent.name, model, desc);
                    }
                }

                Ok(())
            }

            Commands::Init => init::run_init(),

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

            Commands::Agent { action } => handle_agent_command(action),

            Commands::Doctor => handle_doctor(),
        }
    }
}

/// Handle agent management subcommands.
fn handle_agent_command(action: AgentAction) -> anyhow::Result<()> {
    match action {
        AgentAction::List => {
            let (agents, _warnings) = crate::agent::definition::load_all_agents();
            if agents.is_empty() {
                println!("No agent definitions found.");
                println!("  Global:  ~/.rustyclaw/agents/");
                println!("  Project: .rustyclaw/agents/");
            } else {
                println!(
                    "{:<20} {:<15} {:<10} DESCRIPTION",
                    "NAME", "MODEL", "MEMORY"
                );
                println!("{}", "-".repeat(80));
                for agent in &agents {
                    let model = agent.model.as_deref().unwrap_or("inherit");
                    let memory = match agent.memory_mode {
                        crate::agent::MemoryMode::Isolated => "isolated",
                        crate::agent::MemoryMode::Shared => "shared",
                    };
                    let desc = if agent.description.len() > 35 {
                        format!("{}...", &agent.description[..32])
                    } else {
                        agent.description.clone()
                    };
                    println!("{:<20} {:<15} {:<10} {}", agent.name, model, memory, desc);
                }
                println!("\n{} agent(s) loaded.", agents.len());
            }
            Ok(())
        }

        AgentAction::Status { name } => {
            let (agents, _) = crate::agent::definition::load_all_agents();
            let agent = agents.iter().find(|a| a.name == name);

            match agent {
                Some(a) => {
                    println!("Agent: {}", a.name);
                    println!("Description: {}", a.description);
                    println!(
                        "Model: {}",
                        a.model.as_deref().unwrap_or("inherit (master's model)")
                    );
                    println!(
                        "Memory: {}",
                        match a.memory_mode {
                            crate::agent::MemoryMode::Isolated => "isolated",
                            crate::agent::MemoryMode::Shared => "shared",
                        }
                    );
                    if let Some(ref tools) = a.tools {
                        println!("Tools: {}", tools.join(", "));
                    } else {
                        println!("Tools: (all — inherits master's tools)");
                    }
                    if !a.context_files.is_empty() {
                        println!("Context files: {}", a.context_files.join(", "));
                    }
                    if !a.system_prompt.is_empty() {
                        let preview = if a.system_prompt.len() > 200 {
                            format!("{}...", &a.system_prompt[..197])
                        } else {
                            a.system_prompt.clone()
                        };
                        println!("\nSystem prompt:\n{}", preview);
                    }
                }
                None => {
                    eprintln!("Agent '{}' not found.", name);
                    std::process::exit(1);
                }
            }
            Ok(())
        }

        AgentAction::Validate { name } => {
            let (agents, warnings) = crate::agent::definition::load_all_agents();

            let relevant_warnings: Vec<_> = match &name {
                Some(n) => warnings.iter().filter(|w| w.file.contains(n)).collect(),
                None => warnings.iter().collect(),
            };

            let relevant_agents: Vec<_> = match &name {
                Some(n) => agents.iter().filter(|a| a.name == *n).collect(),
                None => agents.iter().collect(),
            };

            if let Some(n) = &name {
                if relevant_agents.is_empty() && relevant_warnings.is_empty() {
                    eprintln!("Agent '{}' not found.", n);
                    std::process::exit(1);
                }
            }

            let mut has_issues = false;

            for w in &relevant_warnings {
                has_issues = true;
                eprintln!("  WARNING: {}", w);
            }

            for agent in &relevant_agents {
                println!("  OK: {} — {}", agent.name, agent.description);
            }

            if !has_issues && !relevant_agents.is_empty() {
                println!(
                    "\nAll {} agent(s) validated successfully.",
                    relevant_agents.len()
                );
            } else if has_issues {
                println!("\n{} warning(s) found.", relevant_warnings.len());
            }

            Ok(())
        }

        AgentAction::Logs { name, level, lines } => {
            let log_lines = crate::logging::read_agent_logs(&name, lines, level.as_deref());

            if log_lines.is_empty() {
                println!("No log entries found for agent '{}'.", name);
            } else {
                for line in &log_lines {
                    println!("{}", line);
                }
            }
            Ok(())
        }

        AgentAction::Add => init::run_agent_add(),

        AgentAction::Edit { name } => init::run_agent_edit(&name),

        AgentAction::Remove { name } => init::run_agent_remove(&name),
    }
}

/// Run diagnostics: config, API key, agents, ledger integrity.
fn handle_doctor() -> anyhow::Result<()> {
    println!("RustyClaw Doctor\n");
    let mut issues = 0;

    // 1. Config
    let config_path = crate::config::get_config_path();
    if config_path.exists() {
        println!("  [OK] Config: {}", config_path.display());
    } else {
        println!("  [!!] Config not found: {}", config_path.display());
        println!("       Run: rustyclaw onboard");
        issues += 1;
    }

    let config = crate::config::load_config();

    // 2. Workspace
    let workspace = config.workspace_path();
    if workspace.is_dir() {
        println!("  [OK] Workspace: {}", workspace.display());
    } else {
        println!("  [!!] Workspace not found: {}", workspace.display());
        println!("       Run: rustyclaw onboard");
        issues += 1;
    }

    // 3. Provider / API key
    if let Some(name) = config.get_provider_name(None) {
        println!("  [OK] Provider: {} (API key set)", name);
    } else {
        println!("  [!!] No LLM provider configured (no API key)");
        let model = &config.agents.defaults.model;
        if let Some(spec) = find_provider_for_model(model) {
            println!("       Set: export {}=your-key", spec.env_key);
        }
        issues += 1;
    }

    // 4. Agent definitions
    let (agents, warnings) = crate::agent::definition::load_all_agents();
    if agents.is_empty() {
        println!("  [--] No agent definitions found (optional)");
    } else {
        println!("  [OK] {} agent definition(s) loaded", agents.len());
    }
    for w in &warnings {
        println!("  [!!] Agent warning: {}", w);
        issues += 1;
    }

    // 5. Ledger integrity
    let memory_dir = workspace.join("memory");
    if memory_dir.is_dir() {
        match crate::agent::ledger::MemoryLedger::new(memory_dir) {
            Ok(ledger) => match ledger.verify_chain() {
                Ok(crate::agent::ledger::ChainStatus::Ok { entries }) => {
                    println!("  [OK] Master ledger: {} entries, chain intact", entries);
                }
                Ok(crate::agent::ledger::ChainStatus::Broken {
                    at_seq,
                    expected,
                    got,
                }) => {
                    println!("  [!!] Master ledger: chain broken at seq {}", at_seq);
                    println!("       Expected: {}", expected);
                    println!("       Got:      {}", got);
                    issues += 1;
                }
                Err(e) => {
                    println!("  [!!] Master ledger verify error: {}", e);
                    issues += 1;
                }
            },
            Err(e) => {
                println!("  [!!] Cannot open master ledger: {}", e);
                issues += 1;
            }
        }
    } else {
        println!("  [--] No memory ledger yet (will be created on first use)");
    }

    // 6. Log directory
    let log_dir = crate::logging::log_base_dir();
    if log_dir.is_dir() {
        println!("  [OK] Logs: {}", log_dir.display());
    } else {
        println!("  [--] Logs directory not found (will be created on run)");
    }

    println!();
    if issues == 0 {
        println!("All checks passed.");
    } else {
        println!("{} issue(s) found.", issues);
    }

    Ok(())
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

// Workspace template creation moved to cli/init.rs
