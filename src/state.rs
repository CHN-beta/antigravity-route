use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountConfig {
    pub email: Option<String>,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountsData {
    pub version: i32,
    pub accounts: Vec<AccountConfig>,
    #[serde(rename = "activeIndex")]
    pub active_index: i32,
}

use std::time::Instant;

pub struct AppState {
    pub datadir: PathBuf,
    pub client: Client,
    pub token_cache: Mutex<Option<(String, String, Instant)>>, // (access_token, project_id, timestamp)
}
