use clap::{Parser, Subcommand};
use tracing::info;

/// Nanobot — lightweight AI agent framework.
#[derive(Parser)]
#[command(name = "nanobot", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the agent with all enabled channels.
    Run,

    /// Interactive CLI chat with the agent.
    Chat {
        /// Model override.
        #[arg(short, long)]
        model: Option<String>,
    },

    /// Send a single prompt and get a response.
    Agent {
        /// The prompt to send.
        prompt: String,

        /// Model override.
        #[arg(short, long)]
        model: Option<String>,
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

    /// Show agent status.
    Status,

    /// Run onboarding setup.
    Onboard,
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
            Commands::Run => {
                info!("Starting nanobot with all enabled channels...");
                // TODO: Initialize config, bus, channels, agent, cron, heartbeat
                // and run them all concurrently with tokio::select!
                println!("nanobot is running. Press Ctrl+C to stop.");
                tokio::signal::ctrl_c().await?;
                Ok(())
            }
            Commands::Chat { model } => {
                info!("Starting interactive chat...");
                // TODO: Use rustyline for interactive prompt
                println!("Interactive chat mode. Type 'exit' to quit.");
                Ok(())
            }
            Commands::Agent { prompt, model } => {
                info!("Processing single prompt...");
                // TODO: Initialize agent and call process_direct
                println!("Agent response would go here.");
                Ok(())
            }
            Commands::Channels { action } => {
                match action {
                    ChannelAction::Status => {
                        let config = crate::config::load_config();
                        println!("Channel status:");
                        println!("  Telegram: {}", if config.channels.telegram.enabled { "enabled" } else { "disabled" });
                        println!("  Discord:  {}", if config.channels.discord.enabled { "enabled" } else { "disabled" });
                    }
                    ChannelAction::Login { channel } => {
                        println!("Login flow for {} (not yet implemented)", channel);
                    }
                }
                Ok(())
            }
            Commands::Cron { action } => {
                match action {
                    CronAction::List => println!("No cron jobs configured."),
                    CronAction::Add { name, schedule, prompt } => {
                        println!("Added cron job: {} ({}) -> {}", name, schedule, prompt);
                    }
                    CronAction::Remove { name } => {
                        println!("Removed cron job: {}", name);
                    }
                    CronAction::Run { name } => {
                        println!("Running cron job: {}", name);
                    }
                }
                Ok(())
            }
            Commands::Status => {
                println!("{}", crate::LOGO);
                println!("Version: {}", crate::VERSION);
                let config = crate::config::load_config();
                println!("Model: {}", config.agents.defaults.model);
                println!("Workspace: {}", config.agents.defaults.workspace);
                Ok(())
            }
            Commands::Onboard => {
                println!("Welcome to nanobot! Let's set things up...");
                // TODO: Interactive onboarding wizard
                Ok(())
            }
        }
    }
}
