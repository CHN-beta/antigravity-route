use crate::auth::get_credentials;
use crate::constants::{CLIENT_ID, CLIENT_SECRET, ENDPOINT, REDIRECT_URI, SCOPES};
use crate::model_resolver::resolve_model_for_antigravity;
use crate::claude_hacks::apply_claude_hacks;
use crate::state::{AccountConfig, AccountsData, AppState};
use axum::{
    Json,
    body::Body,
    extract::{State, Request},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};
use serde_json::Map;

pub fn clean_schema(schema: Value) -> Value {
    if !schema.is_object() {
        return schema;
    }
    
    let mut obj = schema.as_object().unwrap().clone();
    
    // Remove unsupported keys
    let bad_keys = [
        "$schema", "$ref", "anyOf", "oneOf", "allOf", "const", 
        "pattern", "maxLength", "minLength", "maxItems", "minItems", 
        "maximum", "minimum", "exclusiveMaximum", "exclusiveMinimum",
        "multipleOf", "additionalProperties", "default", "format"
    ];
    
    for key in bad_keys.iter() {
        obj.remove(*key);
    }
    
    // Fix type array -> string
    if let Some(typ) = obj.get_mut("type")
        && typ.is_array() {
            let mut new_type = json!("string");
            for t in typ.as_array().unwrap() {
                if t.as_str() != Some("null") {
                    new_type = t.clone();
                    break;
                }
            }
            *typ = new_type;
        }
    
    // Process properties
    if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for (_, v) in props.iter_mut() {
            *v = clean_schema(v.clone());
        }
        
        // If properties is empty, Claude might complain.
        if props.is_empty() {
            props.insert("_placeholder".to_string(), json!({
                "type": "boolean",
                "description": "Placeholder. Always pass true."
            }));
            if let Some(req) = obj.get_mut("required").and_then(|r| r.as_array_mut()) {
                req.push(json!("_placeholder"));
            } else {
                obj.insert("required".to_string(), json!(["_placeholder"]));
            }
        }
    } else if obj.get("type").and_then(|t| t.as_str()) == Some("object") {
        // Type is object but no properties
        let mut props = Map::new();
        props.insert("_placeholder".to_string(), json!({
            "type": "boolean",
            "description": "Placeholder. Always pass true."
        }));
        obj.insert("properties".to_string(), Value::Object(props));
        obj.insert("required".to_string(), json!(["_placeholder"]));
    }
    
    // Process items
    if let Some(items) = obj.get_mut("items") {
        *items = clean_schema(items.clone());
    }
    
    Value::Object(obj)
}

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
        output = serde_json::to_string_pretty(&val).unwrap();
    }

    Ok(output)
}

pub async fn gemini_proxy(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, (StatusCode, String)> {
    let (access_token, project_id) = get_credentials(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path();
    let query = uri.query().unwrap_or("");

    // Extract model and action from the path.
    // Example: /v1beta/models/gemini-1.5-pro:generateContent
    let Some(colon_idx) = path.rfind(':') else {
        // Not a standard model generate endpoint, just proxy it to ENDPOINT as is
        let mut new_url = format!("{}{}", ENDPOINT, path);
        if !query.is_empty() {
            new_url.push('?');
            new_url.push_str(query);
        }

        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        
        let res = state
            .client
            .request(method, new_url)
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
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut builder = Response::builder().status(res.status());
        for (name, value) in res.headers() {
            if name != reqwest::header::TRANSFER_ENCODING {
                builder = builder.header(name, value);
            }
        }
        return Ok(builder.body(Body::from_stream(res.bytes_stream())).unwrap());
    };

    let model_part = &path[..colon_idx];
    let action = &path[colon_idx + 1..]; // e.g. "generateContent" or "streamGenerateContent"

    let Some(requested_model) = model_part.split('/').next_back() else {
        return Err((StatusCode::BAD_REQUEST, "Missing model name".into()));
    };

    let resolved_model = resolve_model_for_antigravity(requested_model);
    let model_name = &resolved_model.actual_model;

    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    let mut request_payload: Value = serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}));
    
    // Check if it's already wrapped (sometimes opencode might wrap it)
    let is_wrapped = request_payload.get("project").is_some() && request_payload.get("request").is_some();
    
    // Apply Claude hacks (Tool ID pairing, sorting parts, system instruction, etc.)
    if is_wrapped {
        if let Some(inner) = request_payload.get_mut("request") {
            apply_claude_hacks(inner, model_name);
        }
    } else {
        apply_claude_hacks(&mut request_payload, model_name);
    }
    
    let mut wrapped_body = if is_wrapped {
        request_payload["model"] = json!(model_name);
        request_payload
    } else {
        json!({
            "project": project_id,
            "model": model_name,
            "request": request_payload
        })
    };

    // Apply thinking config if applicable
    if (resolved_model.thinking_budget.is_some() || resolved_model.thinking_level.is_some())
        && let Some(req_obj) = wrapped_body.get_mut("request").and_then(|r| r.as_object_mut()) {
            let gen_config = req_obj
                .entry("generationConfig")
                .or_insert_with(|| json!({}));
            
            if let Some(gen_obj) = gen_config.as_object_mut() {
                let thinking_config = gen_obj
                    .entry("thinkingConfig")
                    .or_insert_with(|| json!({}));
                
                if let Some(think_obj) = thinking_config.as_object_mut() {
                    let is_claude = model_name.to_lowercase().contains("claude");
                    let is_gemini3 = model_name.to_lowercase().contains("gemini-3");
                    
                    if is_claude {
                        if let Some(budget) = resolved_model.thinking_budget {
                            think_obj.insert("thinking_budget".to_string(), json!(budget));
                        }
                    } else if is_gemini3 {
                        if let Some(ref level) = resolved_model.thinking_level {
                            think_obj.insert("thinkingLevel".to_string(), json!(level));
                        }
                    } else {
                        if let Some(budget) = resolved_model.thinking_budget {
                            think_obj.insert("thinkingBudget".to_string(), json!(budget));
                        }
                    }
                }
            }
        }

    // Clean JSON schema for tool calls if needed
    if let Some(req_obj) = wrapped_body.get_mut("request").and_then(|r| r.as_object_mut())
        && let Some(tools) = req_obj.get_mut("tools").and_then(|t| t.as_array_mut()) {
            for tool in tools.iter_mut() {
                if let Some(funcs) = tool.get_mut("functionDeclarations").and_then(|f| f.as_array_mut()) {
                    for func in funcs.iter_mut() {
                        if let Some(params) = func.get_mut("parameters") {
                            *params = clean_schema(params.clone());
                        }
                    }
                }
            }
        }

    // Construct the Antigravity URL
    // e.g. https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:generateContent
    let mut new_url = format!("{}/v1internal:{}", ENDPOINT, action);
    if action == "streamGenerateContent" {
        new_url.push_str("?alt=sse");
    }

    info!("Proxying {} to {}", path, new_url);

    let res = state
        .client
        .post(new_url)
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
        .json(&wrapped_body)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if action == "streamGenerateContent" {
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

                            if let Ok(mut gemini_data) = serde_json::from_str::<Value>(data_str) {
                                // Unwrap the "response" property if it exists
                                if let Some(response_obj) = gemini_data.get("response") {
                                    gemini_data = response_obj.clone();
                                }
                                
                                let out = format!("data: {}\n\n", gemini_data);
                                if tx.send(Ok(out)).await.is_err() {
                                    return;
                                }
                            } else {
                                // Send as is if parsing fails
                                let out = format!("data: {}\n\n", data_str);
                                if tx.send(Ok(out)).await.is_err() {
                                    return;
                                }
                            }
                        } else {
                            // Forward non-data lines? SSE chunks start with data:
                        }
                    }
                }
            }
            let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
        });

        let mut builder = Response::builder().status(StatusCode::OK);
        builder = builder.header(reqwest::header::CONTENT_TYPE, "text/event-stream");
        builder = builder.header(reqwest::header::CACHE_CONTROL, "no-cache");
        builder = builder.header(reqwest::header::CONNECTION, "keep-alive");
        
        let body = Body::from_stream(ReceiverStream::new(rx));
        Ok(builder.body(body).unwrap())
    } else {
        let text = res
            .text()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            
        let mut res_data: Value = serde_json::from_str(&text).unwrap_or(json!({}));
        
        // Unwrap the "response" property if it exists
        if let Some(response_obj) = res_data.get("response") {
            res_data = response_obj.clone();
        }
        
        let mut builder = Response::builder().status(StatusCode::OK);
        builder = builder.header(reqwest::header::CONTENT_TYPE, "application/json");
        
        let body = Body::from(serde_json::to_string(&res_data).unwrap());
        Ok(builder.body(body).unwrap())
    }
}
