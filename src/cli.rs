use anyhow::Result;
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the daemon server
    Daemon {
        /// Data directory to store auth configuration
        #[arg(long)]
        datadir: PathBuf,
        /// Port to run the daemon on
        #[arg(long, default_value_t = 8999)]
        port: u16,
    },
    /// Connect to daemon and authenticate
    Auth {
        /// Daemon URL
        #[arg(long, default_value = "http://127.0.0.1:8999")]
        daemon_url: String,
    },
    /// Query the quota from the daemon
    Quota {
        /// Daemon URL
        #[arg(long, default_value = "http://127.0.0.1:8999")]
        daemon_url: String,
    },
}

pub async fn run_auth_cli(daemon_url: &str) -> Result<()> {
    let client = Client::new();
    let url = format!("{}/v1/auth/url", daemon_url);
    let auth_url: String = client.get(&url).send().await?.text().await?;

    println!(
        "Please open this URL in your browser to log in:\n\n{}\n",
        auth_url
    );
    println!(
        "Complete the login, then paste the full redirect URL or the 'code' parameter value here:"
    );

    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let mut code = code.trim().to_string();

    if code.starts_with("http")
        && let Ok(url) = url::Url::parse(&code)
        && let Some((_, val)) = url.query_pairs().find(|(k, _)| k == "code")
    {
        code = val.into_owned();
    }

    let cb_url = format!("{}/v1/auth/callback", daemon_url);
    let resp = client
        .post(&cb_url)
        .json(&json!({ "code": code }))
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Authentication successful! You can now use the API.");
    } else {
        println!("Authentication failed: {}", resp.text().await?);
    }
    Ok(())
}

pub async fn run_quota_cli(daemon_url: &str) -> Result<()> {
    let client = Client::new();
    let url = format!("{}/v1/dashboard/billing/subscription", daemon_url);
    let resp = client.get(&url).send().await?;

    if resp.status().is_success() {
        let text = resp.text().await?;
        println!("Quota:\n{}", text);
    } else {
        println!("Failed to get quota: {}", resp.text().await?);
    }
    Ok(())
}
