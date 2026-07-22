use crate::handlers::{auth_callback, auth_url, gemini_proxy, quota_handler};
use crate::state::AppState;
use anyhow::Result;
use axum::{
    Router,
    routing::{get, post, any},
};
use reqwest::Client;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

pub async fn run_daemon(datadir: PathBuf, port: u16) -> Result<()> {
    info!(
        "Starting daemon on port {} with datadir {:?}",
        port, datadir
    );
    fs::create_dir_all(&datadir)?;
    let state = Arc::new(AppState {
        datadir,
        client: Client::new(),
        token_cache: Mutex::new(None),
    });

    let app = Router::new()
        .route("/v1/auth/url", get(auth_url))
        .route("/v1/auth/callback", post(auth_callback))
        .route("/v1/dashboard/billing/subscription", get(quota_handler))
        .route("/v1beta/{*path}", any(gemini_proxy))
        .route("/v1/{*path}", any(gemini_proxy))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
