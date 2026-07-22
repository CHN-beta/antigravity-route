use serde_json::{Value, json};

pub fn translate_openai_to_gemini(messages: &Value) -> Value {
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
