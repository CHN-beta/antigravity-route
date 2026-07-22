mod auth;
mod cli;
mod constants;
mod daemon;
mod handlers;
mod state;
mod utils;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, run_auth_cli, run_quota_cli};
use daemon::run_daemon;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt::init();

    match &cli.command {
        Commands::Daemon { datadir, port } => run_daemon(datadir.clone(), *port).await?,
        Commands::Auth { daemon_url } => run_auth_cli(daemon_url).await?,
        Commands::Quota { daemon_url } => run_quota_cli(daemon_url).await?,
    }
    Ok(())
}
