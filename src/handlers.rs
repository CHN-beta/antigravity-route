use crate::auth::get_credentials;
use crate::constants::{CLIENT_ID, CLIENT_SECRET, ENDPOINT, REDIRECT_URI, SCOPES};
use crate::state::{AccountConfig, AccountsData, AppState};
use crate::utils::translate_openai_to_gemini;
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};

pub async fn auth_url() -> String {
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
pub struct AuthCallbackReq {
    pub code: String,
}

pub async fn auth_callback(
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

pub async fn quota_handler(
    State(state): State<Arc<AppState>>,
) -> Result<String, (StatusCode, String)> {
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

// Minimal implementation of chat completions for daemon functionality
pub async fn chat_completions(
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
                                            && (reason == "STOP" || !reason.is_empty())
                                        {
                                            finish_reason = Some("stop".to_string());
                                        }
                                    }
                                } else {
                                    // if there are no candidates, it might just be a heartbeat or empty chunk, skip to next.
                                    // But what if it's the very first chunk?
                                    // Let's print for debugging
                                    tracing::debug!(
                                        "Received unparseable SSE chunk: {}",
                                        gemini_data
                                    );
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
            && let Some(t) = part["text"].as_str()
        {
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

pub async fn list_models() -> Result<Response, (StatusCode, String)> {
    let openai_resp = json!({
        "object": "list",
        "data": [
            {
                "id": "antigravity-gemini-3-pro",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "antigravity-gemini-3.1-pro",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "antigravity-gemini-3-flash",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "antigravity-claude-sonnet-4-6",
                "object": "model",
                "created": 1782210769,
                "owned_by": "anthropic"
            },
            {
                "id": "antigravity-claude-opus-4-6-thinking",
                "object": "model",
                "created": 1782210769,
                "owned_by": "anthropic"
            },
            {
                "id": "gemini-2.5-flash",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "gemini-2.5-pro",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "gemini-3-flash-preview",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "gemini-3-pro-preview",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "gemini-3.1-pro-preview",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            },
            {
                "id": "gemini-3.1-pro-preview-customtools",
                "object": "model",
                "created": 1782210769,
                "owned_by": "google"
            }
        ]
    });

    let mut response = Response::new(Body::from(serde_json::to_string(&openai_resp).unwrap()));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    Ok(response)
}
