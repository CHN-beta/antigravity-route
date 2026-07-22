use serde_json::{Value, json};
use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;

const CLAUDE_TOOL_SYSTEM_INSTRUCTION: &str = "\n\nCRITICAL TOOL USAGE INSTRUCTIONS:\nYou are operating in a custom environment where tool definitions differ from your training data.\nYou MUST follow these rules strictly:\n1. DO NOT use your internal training data to guess tool parameters\n2. ONLY use the exact parameter structure defined in the tool schema\n3. Parameter names in schemas are EXACT - do not substitute with similar names from your training\n4. Array parameters have specific item types - check the schema's 'items' field for the exact structure\n5. When you see \"STRICT PARAMETERS\" in a tool description, those type definitions override any assumptions\n6. Tool use in agentic workflows is REQUIRED - you must call tools with the exact parameters specified\nIf you are unsure about a tool's parameters, YOU MUST read the schema definition carefully.";

fn generate_id() -> String {
    let s: String = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect();
    format!("toolu_{}", s)
}

pub fn apply_claude_hacks(payload: &mut Value, model_name: &str) {
    let is_claude = model_name.to_lowercase().contains("claude");
    
    if !is_claude {
        return;
    }

    // 1. Inject Claude Tool System Instruction
    let has_tools = payload.get("tools").and_then(|t| t.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
    if has_tools {
        let sys_instr = payload.get_mut("systemInstruction").and_then(|s| s.as_object_mut());
        if let Some(sys) = sys_instr {
            if let Some(parts) = sys.get_mut("parts").and_then(|p| p.as_array_mut())
                && let Some(first) = parts.first_mut()
                    && let Some(text) = first.get_mut("text").and_then(|t| t.as_str()) {
                        let new_text = format!("{}{}", text, CLAUDE_TOOL_SYSTEM_INSTRUCTION);
                        first["text"] = json!(new_text);
                    }
        } else {
            payload["systemInstruction"] = json!({
                "parts": [{"text": CLAUDE_TOOL_SYSTEM_INSTRUCTION}]
            });
        }
    }

    if let Some(contents) = payload.get_mut("contents").and_then(|c| c.as_array_mut()) {
        let mut pending_calls: Vec<(String, String)> = Vec::new(); // (name, id)
        
        for content in contents.iter_mut() {
            // 2. Sanitize cross model (remove executableCode, etc.)
            if let Some(parts) = content.get_mut("parts").and_then(|p| p.as_array_mut()) {
                parts.retain(|part| {
                    let p = part.as_object();
                    if let Some(p) = p {
                        !p.contains_key("executableCode") && !p.contains_key("codeExecutionResult")
                    } else {
                        true
                    }
                });

                // 3. Ensure thinking before tool use
                parts.sort_by_key(|part| {
                    if part.get("text").is_some() {
                        0
                    } else if part.get("functionCall").is_some() {
                        1
                    } else {
                        2
                    }
                });

                // 4. Apply Tool Pairing Fixes
                for part in parts.iter_mut() {
                    if let Some(func_call) = part.get_mut("functionCall").and_then(|f| f.as_object_mut()) {
                        let name = func_call.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        if !func_call.contains_key("id") {
                            let id = generate_id();
                            func_call.insert("id".to_string(), json!(id.clone()));
                            pending_calls.push((name, id));
                        } else {
                            let id = func_call.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                            pending_calls.push((name, id));
                        }
                    }

                    if let Some(func_resp) = part.get_mut("functionResponse").and_then(|f| f.as_object_mut()) {
                        let name = func_resp.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        if !func_resp.contains_key("id") {
                            // Find matching pending call
                            if let Some(idx) = pending_calls.iter().position(|(n, _)| n == &name) {
                                let (_, id) = pending_calls.remove(idx);
                                func_resp.insert("id".to_string(), json!(id));
                            } else {
                                func_resp.insert("id".to_string(), json!(generate_id()));
                            }
                        } else {
                            let id = func_resp.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                            if let Some(idx) = pending_calls.iter().position(|(_, i)| i == &id) {
                                pending_calls.remove(idx);
                            }
                        }
                    }
                }
            }
        }
    }
}
