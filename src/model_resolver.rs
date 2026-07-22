use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;

lazy_static! {
    static ref TIER_REGEX: Regex = Regex::new(r"-(minimal|low|medium|high)$").unwrap();
    static ref QUOTA_PREFIX_REGEX: Regex = Regex::new(r"(?i)^antigravity-").unwrap();
    static ref GEMINI_3_PRO_REGEX: Regex = Regex::new(r"(?i)^gemini-3(?:\.\d+)?-pro").unwrap();
    static ref GEMINI_3_FLASH_REGEX: Regex = Regex::new(r"(?i)^gemini-3(?:\.\d+)?-flash").unwrap();
    static ref IMAGE_GENERATION_MODELS: Regex = Regex::new(r"(?i)image|imagen").unwrap();
    
    static ref MODEL_ALIASES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("gemini-3-pro-low", "gemini-3-pro");
        m.insert("gemini-3-pro-high", "gemini-3-pro");
        m.insert("gemini-3.1-pro-low", "gemini-3.1-pro");
        m.insert("gemini-3.1-pro-high", "gemini-3.1-pro");
        m.insert("gemini-3-flash-low", "gemini-3-flash");
        m.insert("gemini-3-flash-medium", "gemini-3-flash");
        m.insert("gemini-3-flash-high", "gemini-3-flash");
        m.insert("gemini-claude-opus-4-6-thinking-low", "claude-opus-4-6-thinking");
        m.insert("gemini-claude-opus-4-6-thinking-medium", "claude-opus-4-6-thinking");
        m.insert("gemini-claude-opus-4-6-thinking-high", "claude-opus-4-6-thinking");
        m.insert("gemini-claude-sonnet-4-6", "claude-sonnet-4-6");
        m
    };
}

pub struct ResolvedModel {
    pub actual_model: String,
    pub thinking_budget: Option<u32>,
    pub thinking_level: Option<String>,
}

fn extract_thinking_tier(model: &str) -> Option<String> {
    let lower = model.to_lowercase();
    let supports = lower.contains("gemini-3") || 
                   lower.contains("gemini-2.5") || 
                   (lower.contains("claude") && lower.contains("thinking"));
    
    if !supports {
        return None;
    }
    
    TIER_REGEX.captures(model).map(|caps| caps[1].to_string())
}

fn get_budget_for_tier(family: &str, tier: &str) -> u32 {
    match family {
        "claude" => match tier {
            "low" => 8192,
            "medium" => 16384,
            _ => 32768,
        },
        "gemini-2.5-pro" => match tier {
            "low" => 8192,
            "medium" => 16384,
            _ => 32768,
        },
        "gemini-2.5-flash" => match tier {
            "low" => 6144,
            "medium" => 12288,
            _ => 24576,
        },
        _ => match tier {
            "low" => 4096,
            "medium" => 8192,
            _ => 16384,
        },
    }
}

pub fn resolve_model_with_tier(requested_model: &str) -> ResolvedModel {
    let is_antigravity = QUOTA_PREFIX_REGEX.is_match(requested_model);
    let model_without_quota = QUOTA_PREFIX_REGEX.replace(requested_model, "").to_string();
    
    let tier = extract_thinking_tier(&model_without_quota);
    let base_name = if let Some(ref _t) = tier {
        TIER_REGEX.replace(&model_without_quota, "").to_string()
    } else {
        model_without_quota.clone()
    };
    
    let is_image = IMAGE_GENERATION_MODELS.is_match(&model_without_quota);
    let is_gemini3 = model_without_quota.to_lowercase().starts_with("gemini-3");
    let skip_alias = is_antigravity && is_gemini3;
    
    let is_gemini3_pro = GEMINI_3_PRO_REGEX.is_match(&model_without_quota);
    let is_gemini3_flash = GEMINI_3_FLASH_REGEX.is_match(&model_without_quota);
    
    let mut antigravity_model = model_without_quota.clone();
    if skip_alias {
        if is_gemini3_pro && tier.is_none() && !is_image {
            antigravity_model = format!("{}-low", model_without_quota);
        } else if is_gemini3_flash && tier.is_some() {
            antigravity_model = base_name.clone();
        }
    }
    
    let actual_model = if skip_alias {
        antigravity_model
    } else {
        if let Some(&aliased) = MODEL_ALIASES.get(model_without_quota.as_str()) {
            aliased.to_string()
        } else if let Some(&aliased) = MODEL_ALIASES.get(base_name.as_str()) {
            aliased.to_string()
        } else {
            base_name.clone()
        }
    };
    
    if is_image {
        return ResolvedModel {
            actual_model,
            thinking_budget: None,
            thinking_level: None,
        };
    }
    
    let is_effective_gemini3 = actual_model.to_lowercase().contains("gemini-3");
    let is_claude_thinking = actual_model.to_lowercase().contains("claude") && actual_model.to_lowercase().contains("thinking");
    
    if tier.is_none() {
        if is_effective_gemini3 {
            return ResolvedModel {
                actual_model,
                thinking_budget: None,
                thinking_level: Some("low".to_string()),
            };
        }
        if is_claude_thinking {
            return ResolvedModel {
                actual_model,
                thinking_budget: Some(32768),
                thinking_level: None,
            };
        }
        return ResolvedModel {
            actual_model,
            thinking_budget: None,
            thinking_level: None,
        };
    }
    
    let tier_val = tier.unwrap();
    if is_effective_gemini3 {
        return ResolvedModel {
            actual_model,
            thinking_budget: None,
            thinking_level: Some(tier_val),
        };
    }
    
    let family = if actual_model.contains("claude") {
        "claude"
    } else if actual_model.contains("gemini-2.5-pro") {
        "gemini-2.5-pro"
    } else if actual_model.contains("gemini-2.5-flash") {
        "gemini-2.5-flash"
    } else {
        "default"
    };
    
    let budget = get_budget_for_tier(family, &tier_val);
    ResolvedModel {
        actual_model,
        thinking_budget: Some(budget),
        thinking_level: None,
    }
}

pub fn resolve_model_for_antigravity(requested_model: &str) -> ResolvedModel {
    let lower = requested_model.to_lowercase();
    let is_gemini3 = lower.contains("gemini-3");
    
    if !is_gemini3 {
        return resolve_model_with_tier(requested_model);
    }
    
    // For antigravity header style
    let mut transformed = requested_model
        .replace("-preview-customtools", "")
        .replace("-preview", "");
    
    transformed = QUOTA_PREFIX_REGEX.replace(&transformed, "").to_string();
    
    let is_gemini3_pro = GEMINI_3_PRO_REGEX.is_match(&transformed);
    let has_tier_suffix = TIER_REGEX.is_match(&transformed);
    let is_image = IMAGE_GENERATION_MODELS.is_match(&transformed);
    
    if is_gemini3_pro && !has_tier_suffix && !is_image {
        transformed = format!("{}-low", transformed);
    }
    
    let prefixed = format!("antigravity-{}", transformed);
    resolve_model_with_tier(&prefixed)
}
