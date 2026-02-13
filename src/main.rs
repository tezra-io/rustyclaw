use clap::Parser;
use rustyclaw::cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustyclaw::logging::init_logging();

    let cli = Cli::parse();
    cli.run().await
}
