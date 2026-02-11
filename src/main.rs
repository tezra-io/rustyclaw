use clap::Parser;
use nanobot::cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nanobot=info".into()),
        )
        .init();

    let cli = Cli::parse();
    cli.run().await
}
