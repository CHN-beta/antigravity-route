use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";

use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AccountConfig {
    email: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AccountsData {
    version: i32,
    accounts: Vec<AccountConfig>,
    #[serde(rename = "activeIndex")]
    active_index: i32,
}

struct AppState {
    datadir: PathBuf,
    client: Client,
    token_cache: Mutex<Option<(String, String)>>, // (access_token, project_id)
}

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

async fn run_daemon(datadir: PathBuf, port: u16) -> Result<()> {
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
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_auth_cli(daemon_url: &str) -> Result<()> {
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

async fn run_quota_cli(daemon_url: &str) -> Result<()> {
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

async fn auth_url() -> String {
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");
    url.to_string()
}

#[derive(Deserialize)]
struct AuthCallbackReq {
    code: String,
}

async fn auth_callback(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AuthCallbackReq>,
) -> Result<String, (StatusCode, String)> {
    info!("Received auth callback with code");
    let res = state
        .client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("code", &req.code),
            ("redirect_uri", REDIRECT_URI),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let tokens: Value = res
        .json()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let refresh_token = tokens["refresh_token"]
        .as_str()
        .ok_or((StatusCode::BAD_REQUEST, "No refresh token".into()))?;
    let access_token = tokens["access_token"]
        .as_str()
        .ok_or((StatusCode::BAD_REQUEST, "No access token".into()))?;

    // fetch email
    let userinfo_res = state
        .client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let userinfo: Value = userinfo_res
        .json()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let email = userinfo["email"].as_str().unwrap_or("unknown@example.com");

    let account_file = state.datadir.join("antigravity-accounts.json");
    let acc_data = AccountsData {
        version: 4,
        accounts: vec![AccountConfig {
            email: Some(email.to_string()),
            refresh_token: refresh_token.to_string(),
        }],
        active_index: 0,
    };

    fs::write(
        &account_file,
        serde_json::to_string_pretty(&acc_data).unwrap(),
    )
    .map_err(|e| {
        error!(
            "Failed to write accounts data to {}: {}",
            account_file.display(),
            e
        );
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    // clear cache
    *state.token_cache.lock().await = None;

    info!("Successfully authenticated and saved token for {:?}", email);
    Ok("OK".into())
}

async fn get_credentials(state: &Arc<AppState>) -> anyhow::Result<(String, String)> {
    let mut cache = state.token_cache.lock().await;
    if let Some(creds) = &*cache {
        return Ok(creds.clone());
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

    *cache = Some((access_token.clone(), project_id.clone()));
    Ok((access_token, project_id))
}

async fn quota_handler(State(state): State<Arc<AppState>>) -> Result<String, (StatusCode, String)> {
    let (access_token, project_id) = get_credentials(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let res = state
        .client
        .post(format!("{}/v1internal:retrieveUserQuotaSummary", ENDPOINT))
        .bearer_auth(access_token)
        .header("User-Agent", "antigravity/windows/amd64")
        .header(
            "X-Goog-Api-Client",
            "google-cloud-sdk vscode_cloudshelleditor/0.1",
        )
        .header(
            "Client-Metadata",
            r#"{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
        )
        .json(&json!({"project": project_id}))
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let val: Value = res
        .json()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Format output as progress bars
    let mut output = String::new();

    if let Some(groups) = val["groups"].as_array() {
        for group in groups {
            if let Some(display_name) = group["displayName"].as_str() {
                output.push_str(&format!("\n\x1b[1;36m{}\x1b[0m\n", display_name));
            }
            if let Some(desc) = group["description"].as_str() {
                output.push_str(&format!("\x1b[90m{}\x1b[0m\n", desc));
            }

            if let Some(buckets) = group["buckets"].as_array() {
                for bucket in buckets {
                    let bucket_name = bucket["displayName"].as_str().unwrap_or("Unknown Limit");
                    let remaining = bucket["remainingFraction"].as_f64().unwrap_or(0.0);
                    let used_pct = (1.0 - remaining) * 100.0;

                    let bar_width = 40;
                    let filled = (bar_width as f64 * (1.0 - remaining)) as usize;
                    let empty = bar_width - filled;

                    // ANSI colors: Red if used > 90%, Yellow if > 70%, Green otherwise
                    let color = if used_pct > 90.0 {
                        "\x1b[31m"
                    } else if used_pct > 70.0 {
                        "\x1b[33m"
                    } else {
                        "\x1b[32m"
                    };

                    let bar = format!(
                        "{}{}{}\x1b[0m",
                        color,
                        "█".repeat(filled),
                        "░".repeat(empty)
                    );
                    output.push_str(&format!(
                        "  {:<15} [{}] {:>5.1}% used\n",
                        bucket_name, bar, used_pct
                    ));

                    if let Some(desc) = bucket["description"].as_str() {
                        output.push_str(&format!("    \x1b[90m{}\x1b[0m\n", desc));
                    }
                }
            }
        }
        output.push('\n');
    } else {
        // Fallback if structure is unknown
        output = serde_json::to_string_pretty(&val).unwrap();
    }

    Ok(output)
}

fn translate_openai_to_gemini(messages: &Value) -> Value {
    let mut contents = Vec::new();
    let Some(msgs) = messages.as_array() else {
        return serde_json::Value::Array(contents);
    };

    for msg in msgs {
        let mut role = msg["role"].as_str().unwrap_or("user").to_string();
        if role == "assistant" {
            role = "model".to_string();
        } else if role == "system" {
            role = "user".to_string();
        }

        let mut parts = Vec::new();

        if let Some(content_str) = msg["content"].as_str() {
            parts.push(json!({"text": content_str}));
        } else if let Some(content_arr) = msg["content"].as_array() {
            for part in content_arr {
                if part["type"].as_str() == Some("text") {
                    if let Some(text) = part["text"].as_str() {
                        parts.push(json!({"text": text}));
                    }
                } else if part["type"].as_str() == Some("image_url")
                    && let Some(url) = part["image_url"]["url"].as_str()
                    && url.starts_with("data:")
                    && let Some(comma_idx) = url.find(',')
                {
                    let meta = &url[5..comma_idx];
                    let data = &url[comma_idx + 1..];
                    let mut mime_type = "image/jpeg";
                    if let Some(semi_idx) = meta.find(';') {
                        mime_type = &meta[..semi_idx];
                    }
                    parts.push(json!({
                        "inlineData": {
                            "mimeType": mime_type,
                            "data": data
                        }
                    }));
                }
            }
        }

        if parts.is_empty() {
            parts.push(json!({"text": ""}));
        }

        contents.push(json!({
            "role": role,
            "parts": parts
        }));
    }

    // Merge consecutive messages of the same role
    let mut merged: Vec<Value> = Vec::new();
    for content in contents {
        if let Some(last) = merged.last_mut()
            && last["role"] == content["role"]
        {
            let last_parts = last["parts"].as_array_mut().unwrap();
            let mut content_parts = content["parts"].as_array().unwrap().clone();
            last_parts.append(&mut content_parts);
            continue;
        }
        merged.push(content);
    }

    serde_json::Value::Array(merged)
}

// Minimal implementation of chat completions for daemon functionality
async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Value>,
) -> Result<Response, (StatusCode, String)> {
    info!("Handling chat completions request");
    let (access_token, project_id) = get_credentials(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let model = req["model"]
        .as_str()
        .unwrap_or("claude-sonnet-4-6-thinking");
    let mapped_model = match model {
        "claude-3-5-sonnet-latest" => "claude-sonnet-4-6-thinking",
        "claude-3-5-sonnet-20241022" => "claude-sonnet-4-6-thinking",
        "claude-3-5-sonnet-20240620" => "claude-sonnet-4-6",
        "claude-3-opus-20240229" => "claude-opus-4-6-thinking",
        "claude-3-5-haiku-latest" => "claude-sonnet-4-6",
        "gemini-3.5-flash" => "gemini-3.5-flash-low",
        "gemini-3-flash" => "gemini-3-flash",
        "claude-opus-4-6-thinking" => "claude-opus-4-6-thinking",
        "claude-opus-4-6" => "claude-opus-4-6-thinking",
        "claude-sonnet-4-6-thinking" => "claude-sonnet-4-6-thinking",
        "claude-sonnet-4-6" => "claude-sonnet-4-6",
        "gemini-3.1-pro-high" => "gemini-3.1-pro-high",
        "gemini-3.1-pro-low" => "gemini-3.1-pro-low",
        _ => "gemini-3.5-flash-low", // default fallback
    };
    info!(
        "Handling chat request for model {}, mapped to {}, stream: {}",
        model,
        mapped_model,
        req["stream"].as_bool().unwrap_or(false)
    );

    let stream = req["stream"].as_bool().unwrap_or(false);
    let action = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };

    let gemini_contents = translate_openai_to_gemini(&req["messages"]);
    let temperature = req["temperature"].as_f64().unwrap_or(0.7);

    let mut url = format!("{}/v1internal:{}", ENDPOINT, action);
    if stream {
        url.push_str("?alt=sse");
    }
    let request_body = json!({
        "project": project_id,
        "model": mapped_model,
        "request": {
            "contents": gemini_contents,
            "generationConfig": {
                "temperature": temperature
            }
        }
    });

    let res = state
        .client
        .post(url)
        .bearer_auth(access_token)
        .header("User-Agent", "antigravity/windows/amd64")
        .header(
            "X-Goog-Api-Client",
            "google-cloud-sdk vscode_cloudshelleditor/0.1",
        )
        .header(
            "Client-Metadata",
            r#"{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
        )
        .json(&request_body)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if stream {
        let req_model_clone = model.to_string();
        let (tx, rx) = mpsc::channel::<Result<String, String>>(32);

        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut res = res;

            while let Ok(Some(chunk)) = res.chunk().await {
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    buffer.push_str(text);

                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].trim_end().to_string();
                        buffer.drain(..=pos); // Drain exactly up to and including the \n character

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(stripped) = line.strip_prefix("data:") {
                            let data_str = stripped.trim();
                            if data_str.is_empty() {
                                continue;
                            }

                            if let Ok(gemini_data) = serde_json::from_str::<Value>(data_str) {
                                let mut text_content = String::new();
                                let mut finish_reason = None::<String>;

                                // Extract arrays properly, looking into top-level or response obj
                                let candidates_arr = gemini_data["response"]["candidates"]
                                    .as_array()
                                    .or_else(|| gemini_data["candidates"].as_array());

                                if let Some(candidates) = candidates_arr {
                                    if let Some(first) = candidates.first() {
                                        if let Some(parts) = first["content"]["parts"].as_array()
                                            && let Some(part) = parts.first()
                                            && let Some(t) = part["text"].as_str()
                                        {
                                            text_content = t.to_string();
                                        }
                                        if let Some(reason) = first["finishReason"].as_str()
                                            && (reason == "STOP" || !reason.is_empty()) {
                                                finish_reason = Some("stop".to_string());
                                            }
                                    }
                                } else {
                                    // if there are no candidates, it might just be a heartbeat or empty chunk, skip to next.
                                    // But what if it's the very first chunk?
                                    // Let's print for debugging
                                    tracing::debug!("Received unparseable SSE chunk: {}", gemini_data);
                                }

                                if !text_content.is_empty() || finish_reason.is_some() {
                                    let now = SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap()
                                        .as_secs();

                                    let chunk_json = json!({
                                        "id": "chatcmpl-mock",
                                        "object": "chat.completion.chunk",
                                        "created": now,
                                        "model": req_model_clone,
                                        "choices": [
                                            {
                                                "delta": if !text_content.is_empty() { json!({"content": text_content}) } else { json!({}) },
                                                "index": 0,
                                                "finish_reason": finish_reason
                                            }
                                        ]
                                    });

                                    let out = format!("data: {}\n\n", chunk_json);
                                    if tx.send(Ok(out)).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
        });

        let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
        response
            .headers_mut()
            .insert(header::CONNECTION, "keep-alive".parse().unwrap());
        Ok(response)
    } else {
        let text = res
            .text()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let res_data: Value = serde_json::from_str(&text).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to parse JSON: {}, Raw: {}", e, text),
            )
        })?;

        let mut text_content = String::new();
        // The structure seems to vary. Sometimes it's res_data["candidates"] directly.
        let candidates_arr = res_data["response"]["candidates"]
            .as_array()
            .or_else(|| res_data["candidates"].as_array());

        if let Some(candidates) = candidates_arr
            && let Some(first) = candidates.first()
                && let Some(parts) = first["content"]["parts"].as_array()
                    && let Some(part) = parts.first()
                        && let Some(t) = part["text"].as_str() {
                            text_content = t.to_string();
                        }

        let openai_resp = json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 1782210769,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": text_content
                    },
                    "finish_reason": "stop"
                }
            ]
        });

        let mut response = Response::new(Body::from(serde_json::to_string(&openai_resp).unwrap()));
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        Ok(response)
    }
}
