use crate::constants::{CLIENT_ID, CLIENT_SECRET, ENDPOINT};
use crate::state::{AccountsData, AppState};
use anyhow::Context;
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;

pub async fn get_credentials(state: &Arc<AppState>) -> anyhow::Result<(String, String)> {
    let mut cache = state.token_cache.lock().await;
    if let Some((access_token, project_id, timestamp)) = &*cache
        && timestamp.elapsed().as_secs() < 3000 {
            return Ok((access_token.clone(), project_id.clone()));
        }

    let account_file = state.datadir.join("antigravity-accounts.json");
    let acc_data: AccountsData = serde_json::from_str(&fs::read_to_string(&account_file)?)?;
    let acc = acc_data.accounts.first().context("No account configured")?;

    let res = state
        .client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("refresh_token", &acc.refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;
    let tokens: Value = res.json().await?;
    let access_token = tokens["access_token"]
        .as_str()
        .context("No access token")?
        .to_string();

    let proj_res = state
        .client
        .post(format!("{}/v1internal:loadCodeAssist", ENDPOINT))
        .bearer_auth(&access_token)
        .header("User-Agent", "google-api-nodejs-client/9.15.1")
        .header(
            "X-Goog-Api-Client",
            "google-cloud-sdk vscode_cloudshelleditor/0.1",
        )
        .header(
            "Client-Metadata",
            r#"{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
        )
        .json(&json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        }))
        .send()
        .await?;

    let proj_data: Value = proj_res.json().await?;
    let project_id = if let Some(obj) = proj_data["cloudaicompanionProject"].as_object() {
        obj["id"].as_str().unwrap_or("unknown").to_string()
    } else {
        proj_data["cloudaicompanionProject"]
            .as_str()
            .unwrap_or("unknown")
            .to_string()
    };

    *cache = Some((access_token.clone(), project_id.clone(), std::time::Instant::now()));
    Ok((access_token, project_id))
}
