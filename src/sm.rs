use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

static REGEX_CACHE: LazyLock<Mutex<HashMap<String, Regex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_cached_regex(pattern: &str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap();
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            cache.insert(pattern.to_string(), re.clone());
            Some(re)
        }
        Err(_) => None,
    }
}

fn split_condition_preserving_quotes<'a>(input: &'a str, delimiter: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let del_bytes = delimiter.as_bytes();
    let del_len = delimiter.len();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && in_quotes && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        }
        if !in_quotes && i + del_len <= bytes.len() && &bytes[i..i + del_len] == del_bytes {
            parts.push(&input[start..i]);
            start = i + del_len;
            i += del_len;
            continue;
        }
        i += 1;
    }
    parts.push(&input[start..]);
    parts
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StateMachine {
    pub name: String,
    pub version: String,
    pub steps: Vec<String>,
    pub states: HashMap<String, SmState>,
    pub transitions: Vec<SmTransition>,
    pub auto_rules: Vec<SmAutoRule>,
    pub overrides: Vec<SmOverride>,
    #[serde(default)]
    pub secret_overrides: Vec<SmSecretOverride>,
    /// Workflow-defined tool groups from a `[tool_groups]` section:
    /// name -> list of tool names (may reference other groups, including built-ins).
    /// Merged into `settings.tool_group_definitions` on every routing pass.
    #[serde(default)]
    pub tool_groups: HashMap<String, Vec<String>>,
    /// Trusted, named verifier commands. Never populated from model context.
    #[serde(default)]
    pub checks: HashMap<String, crate::gateway::action_contracts::CheckContract>,
    /// Destination state (or `_complete`) -> required current check receipts.
    #[serde(default)]
    pub guards: HashMap<String, Vec<String>>,
    /// Destination -> task-owned, verified plugin capability receipts.
    #[serde(default)]
    pub action_guards: HashMap<String, Vec<String>>,
    /// Trusted Decision IR opcode -> native tool or plugin/tool capability.
    #[serde(default)]
    pub decision_ir: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SmState {
    pub variables: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmTransition {
    pub from: String,
    pub to: String,
    pub condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmAutoRule {
    pub condition: String,
    pub target_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmOverride {
    pub condition: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmSecretOverride {
    pub condition: String,
    pub key: String,
    pub value: String,
}

const ALLOWED_SECRET_FIELDS: &[&str] = &[
    "mimo_voice",
    "mimo_tts_type",
    "mimo_tts_base_url",
    "mimo_tts_model",
    "mimo_style_instruction",
    "mimo_voice_design_prompt",
    "minimax_voice_id",
    "minimax_model",
    "voice_elevenlabs_voice_id",
    "qwen_tts_speaker",
    "qwen_tts_language",
    "qwen_voice_clone_enabled",
    "qwen_voice_clone_audio_path",
    "qwen_voice_clone_prompt",
    "rvc_on",
];

#[derive(Debug)]
pub enum SmError {
    ParseError(String),
    IoError(std::io::Error),
}

impl std::fmt::Display for SmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmError::ParseError(msg) => write!(f, "SM Parse Error: {}", msg),
            SmError::IoError(e) => write!(f, "SM IO Error: {}", e),
        }
    }
}

impl From<std::io::Error> for SmError {
    fn from(e: std::io::Error) -> Self {
        SmError::IoError(e)
    }
}

pub fn parse(content: &str) -> Result<StateMachine, SmError> {
    let mut sm = StateMachine::default();
    let mut current_section: Option<String> = None;
    let mut current_state_name: Option<String> = None;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let inner = &trimmed[1..trimmed.len() - 1];
            let parts: Vec<&str> = inner.splitn(2, ' ').collect();

            match parts[0] {
                "state" => {
                    let name = parts
                        .get(1)
                        .ok_or_else(|| {
                            SmError::ParseError(format!(
                                "Line {}: [state] requires a name",
                                line_num + 1
                            ))
                        })?
                        .to_string();
                    current_state_name = Some(name.clone());
                    sm.states.entry(name).or_default();
                    current_section = Some("state".to_string());
                }
                "transitions" => {
                    current_state_name = None;
                    current_section = Some("transitions".to_string());
                }
                "auto" => {
                    current_state_name = None;
                    current_section = Some("auto".to_string());
                }
                "overrides" => {
                    current_state_name = None;
                    current_section = Some("overrides".to_string());
                }
                "secrets" => {
                    current_state_name = None;
                    current_section = Some("secrets".to_string());
                }
                "tool_groups" => {
                    current_state_name = None;
                    current_section = Some("tool_groups".to_string());
                }
                "checks" | "guards" | "action_guards" | "decision_ir" => {
                    current_state_name = None;
                    current_section = Some(parts[0].to_string());
                }
                other => {
                    return Err(SmError::ParseError(format!(
                        "Line {}: Unknown section [{}]",
                        line_num + 1,
                        other
                    )));
                }
            }
            continue;
        }

        if trimmed.starts_with('@') {
            let (key, value) = parse_metadata(trimmed, line_num)?;
            match key.as_str() {
                "name" => sm.name = value,
                "version" => sm.version = value,
                "steps" => {
                    sm.steps = parse_array(&value);
                }
                _ => {}
            }
            continue;
        }

        match current_section.as_deref() {
            Some("state") => {
                if let Some(ref state_name) = current_state_name {
                    let (key, value) = parse_assignment(trimmed, line_num)?;
                    sm.states
                        .entry(state_name.clone())
                        .or_default()
                        .variables
                        .insert(key, value);
                }
            }
            Some("transitions") => {
                let transition = parse_transition(trimmed, line_num)?;
                sm.transitions.push(transition);
            }
            Some("auto") => {
                let rule = parse_auto_rule(trimmed, line_num)?;
                sm.auto_rules.push(rule);
            }
            Some("overrides") => {
                let overr = parse_override(trimmed, line_num)?;
                sm.overrides.push(overr);
            }
            Some("secrets") => {
                let secret_overr = parse_secret_override(trimmed, line_num)?;
                sm.secret_overrides.push(secret_overr);
            }
            Some("tool_groups") => {
                // `name = [tool_a, "tool_b", other_group]` or `name = single_tool`
                let (key, value) = parse_assignment(trimmed, line_num)?;
                if key.is_empty()
                    || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Err(SmError::ParseError(format!(
                        "Line {}: tool group name '{}' must be non-empty and use only [A-Za-z0-9_-]",
                        line_num + 1,
                        key
                    )));
                }
                let tools: Vec<String> = parse_array(&value)
                    .into_iter()
                    .map(|t| t.trim_matches('"').to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                sm.tool_groups.insert(key, tools);
            }
            Some("checks") => {
                let (key, value) = parse_assignment(trimmed, line_num)?;
                let contract: crate::gateway::action_contracts::CheckContract =
                    serde_json::from_str(&value).map_err(|e| SmError::ParseError(format!("Line {}: invalid check: {e}", line_num + 1)))?;
                contract.validate().map_err(|e| SmError::ParseError(e.to_string()))?;
                if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') || sm.checks.insert(key, contract).is_some() {
                    return Err(SmError::ParseError("Check names must be unique identifiers".into()));
                }
            }
            Some("action_guards") => {
                let (key, value) = parse_assignment(trimmed, line_num)?;
                let names: Vec<String> = parse_array(&value).into_iter().map(|s| s.trim_matches('"').to_string()).collect();
                if !value.starts_with('[') || !value.ends_with(']') || sm.action_guards.insert(key, names).is_some() {
                    return Err(SmError::ParseError("Action guards must be unique arrays of plugin/tool names".into()));
                }
            }
            Some("decision_ir") => {
                let (key, value) = parse_assignment(trimmed, line_num)?;
                let target = value.trim_matches('"').to_string();
                if !crate::gateway::decision_ir::valid_mapping(&key, &target)
                    || sm.decision_ir.len() >= 26
                    || sm.decision_ir.insert(key, target).is_some() {
                    return Err(SmError::ParseError("Decision IR requires unique A..Z opcodes mapped to inspect_file, agent_complete or plugin/tool capabilities".into()));
                }
            }
            Some("guards") => {
                let (key, value) = parse_assignment(trimmed, line_num)?;
                if !value.starts_with('[') || !value.ends_with(']') || sm.guards.insert(key, parse_array(&value).into_iter().map(|s| s.trim_matches('"').to_string()).collect()).is_some() {
                    return Err(SmError::ParseError("Guards must be unique arrays of check names".into()));
                }
            }
            _ => {
                if trimmed.contains('=') && !trimmed.contains("->") {
                    let (key, value) = parse_assignment(trimmed, line_num)?;
                    sm.states
                        .entry("_default".to_string())
                        .or_default()
                        .variables
                        .insert(key, value);
                }
            }
        }
    }

    for (target, required) in &sm.guards {
        if (target != "_complete" && (target == "_default" || !sm.states.contains_key(target))) || required.is_empty() || required.iter().any(|name| !sm.checks.contains_key(name)) {
            return Err(SmError::ParseError(format!("Invalid guard '{target}': expected a state or _complete and declared checks")));
        }
    }
    for (target, names) in &sm.action_guards {
        let mut seen = std::collections::HashSet::new();
        if (target != "_complete" && (target == "_default" || !sm.states.contains_key(target))) || names.is_empty() || names.len() > 16 || names.iter().any(|name| {
            let parts: Vec<_> = name.split('/').collect();
            parts.len() != 2 || !parts.iter().all(|part| crate::plugins::contracts::identifier(part)) || !seen.insert(name)
        }) {
            return Err(SmError::ParseError(format!("Invalid action guard '{target}': expected unique plugin/tool identifiers")));
        }
    }
    Ok(sm)
}

fn parse_metadata(line: &str, _line_num: usize) -> Result<(String, String), SmError> {
    let rest = line.strip_prefix('@').unwrap();
    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
    let key = parts[0].to_string();
    let value = parts.get(1).unwrap_or(&"").trim().to_string();

    let value = if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value
    };

    Ok((key, value))
}

fn parse_array(value: &str) -> Vec<String> {
    let inner = value.trim();
    if inner.starts_with('[') && inner.ends_with(']') {
        inner[1..inner.len() - 1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![inner.to_string()]
    }
}

fn parse_assignment(line: &str, line_num: usize) -> Result<(String, String), SmError> {
    let parts: Vec<&str> = line.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'key = value', got '{}'",
            line_num + 1,
            line
        )));
    }

    let key = parts[0].trim().to_string();
    let value = parts[1].trim().to_string();

    let value = if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value
    };

    Ok((key, value))
}

fn parse_transition(line: &str, line_num: usize) -> Result<SmTransition, SmError> {
    let line = line.replace('→', "->");
    let parts: Vec<&str> = line.splitn(2, "->").collect();
    if parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'from -> to : when condition', got '{}'",
            line_num + 1,
            line
        )));
    }

    let from = parts[0].trim().to_string();
    let rest = parts[1].trim();

    let to_and_cond: Vec<&str> = rest.splitn(2, ':').collect();
    let to = to_and_cond[0].trim().to_string();
    let condition = if to_and_cond.len() > 1 {
        let cond = to_and_cond[1].trim();
        cond.strip_prefix("when ").unwrap_or(cond).to_string()
    } else {
        String::new()
    };

    Ok(SmTransition {
        from,
        to,
        condition,
    })
}

fn parse_auto_rule(line: &str, line_num: usize) -> Result<SmAutoRule, SmError> {
    let line = line.replace('→', "->");
    let parts: Vec<&str> = line.splitn(2, "->").collect();
    if parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'condition -> use state', got '{}'",
            line_num + 1,
            line
        )));
    }

    let condition = parts[0].trim().to_string();
    let rest = parts[1].trim();
    let target_state = rest.strip_prefix("use ").unwrap_or(rest).trim().to_string();

    Ok(SmAutoRule {
        condition,
        target_state,
    })
}

fn parse_override(line: &str, line_num: usize) -> Result<SmOverride, SmError> {
    let line = line.replace('→', "->");
    let parts: Vec<&str> = line.splitn(2, "->").collect();
    if parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'if condition -> key = value', got '{}'",
            line_num + 1,
            line
        )));
    }

    let condition = parts[0].trim();
    let condition = condition
        .strip_prefix("if ")
        .unwrap_or(condition)
        .trim()
        .to_string();

    let kv = parts[1].trim();
    let kv_parts: Vec<&str> = kv.splitn(2, '=').collect();
    if kv_parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'key = value' after ->, got '{}'",
            line_num + 1,
            kv
        )));
    }

    let key = kv_parts[0].trim().to_string();
    let value = kv_parts[1].trim();
    let value = if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    };

    Ok(SmOverride {
        condition,
        key,
        value,
    })
}

fn parse_secret_override(line: &str, line_num: usize) -> Result<SmSecretOverride, SmError> {
    let line = line.replace('→', "->");
    let parts: Vec<&str> = line.splitn(2, "->").collect();
    if parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'if condition -> key = value', got '{}'",
            line_num + 1,
            line
        )));
    }

    let condition = parts[0].trim();
    let condition = condition
        .strip_prefix("if ")
        .unwrap_or(condition)
        .trim()
        .to_string();

    let kv = parts[1].trim();
    let kv_parts: Vec<&str> = kv.splitn(2, '=').collect();
    if kv_parts.len() != 2 {
        return Err(SmError::ParseError(format!(
            "Line {}: Expected 'key = value' after ->, got '{}'",
            line_num + 1,
            kv
        )));
    }

    let key = kv_parts[0].trim().to_string();
    let value = kv_parts[1].trim();
    let value = if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    };

    if !ALLOWED_SECRET_FIELDS.contains(&key.as_str()) {
        return Err(SmError::ParseError(format!(
            "Line {}: Secret field '{}' is not allowed. Allowed: {:?}",
            line_num + 1,
            key,
            ALLOWED_SECRET_FIELDS
        )));
    }

    Ok(SmSecretOverride {
        condition,
        key,
        value,
    })
}

/// Merge workflow `[tool_groups]` definitions into
/// `settings.tool_group_definitions`. Defining a group does NOT activate it;
/// states opt in with `settings.tool_groups = ["name"]`. Workflow definitions
/// override same-named context entries (so edits to the .sm propagate), while
/// user-added groups with other names are preserved.
fn apply_tool_groups(sm: &StateMachine, context: &mut serde_json::Value) {
    if sm.tool_groups.is_empty() {
        return;
    }
    for (name, tools) in &sm.tool_groups {
        set_nested_value(
            context,
            &format!("settings.tool_group_definitions.{name}"),
            serde_json::to_value(tools).unwrap_or(serde_json::Value::Null),
        );
    }
}

/// Apply SM workflow to context. Returns secret overrides.
pub fn apply_to_context(
    sm: &StateMachine,
    context: &mut serde_json::Value,
) -> Vec<(String, String)> {
    // Resolve tool groups and add to context settings
    apply_tool_groups(sm, context);
    
    let active_state = context
        .get("active_state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Apply default state variables first
    if let Some(default_state) = sm.states.get("_default") {
        for (key, value) in &default_state.variables {
            if get_nested_value(context.as_object().unwrap_or(&serde_json::Map::new()), key).is_none() {
                set_nested_value(context, key, parse_json_or_string(value));
            }
        }
    }

    // Apply current state variables before evaluating auto rules,
    // so auto conditions can reference state variables (e.g. regex patterns)
    if let Some(state) = sm.states.get(&active_state) {
        for (key, value) in &state.variables {
            set_nested_value(context, key, parse_json_or_string(value));
        }
    }

    let obj = match context.as_object() {
        Some(o) => o,
        None => return Vec::new(),
    };

    // Auto-rules drive model/workflow-side state switching. While a Decision
    // profile owns routing, its selected state must not be overridden here.
    let decision_owns_state = obj
        .get("settings")
        .is_some_and(|s| {
            s.get("use_decision_router").and_then(|v| v.as_bool()).unwrap_or(true)
                && s.get("decision_profile").and_then(|v| v.as_str()).is_some_and(|p| !p.is_empty() && p != "off")
        });
    let resolved_state = if decision_owns_state {
        resolve_auto_state(&StateMachine { auto_rules: Vec::new(), ..sm.clone() }, obj, &active_state)
    } else {
        resolve_auto_state(sm, obj, &active_state)
    };

    // Apply resolved state variables (may override current state if auto-rule triggered)
    if resolved_state != active_state {
        if let Some(state) = sm.states.get(&resolved_state) {
            for (key, value) in &state.variables {
                set_nested_value(context, key, parse_json_or_string(value));
            }
        }
    }

    // State variables are state-scoped: anything another state sets but the
    // resolved state does not falls back to `_default` or is removed, so a
    // state cannot inherit the previous state's template or tool allowance.
    reset_foreign_state_variables(sm, context, &resolved_state);

    set_nested_value(
        context,
        "active_state",
        serde_json::Value::String(resolved_state),
    );

    // Apply overrides
    for overr in &sm.overrides {
        if evaluate_condition(&overr.condition, context.as_object().unwrap_or(&serde_json::Map::new())) {
            set_nested_value(context, &overr.key, serde_json::Value::String(overr.value.clone()));
        }
    }

    // Collect secret overrides
    let mut secret_changes = Vec::new();
    for secret_overr in &sm.secret_overrides {
        if evaluate_condition(&secret_overr.condition, context.as_object().unwrap_or(&serde_json::Map::new())) {
            secret_changes.push((secret_overr.key.clone(), secret_overr.value.clone()));
        }
    }

    secret_changes
}

/// Remove (or reset to `_default`) every variable declared by some other state
/// but not by `state_name`.
fn reset_foreign_state_variables(sm: &StateMachine, context: &mut serde_json::Value, state_name: &str) {
    let canon = |k: &str| crate::db::contexts::canonical_context_key(k).into_owned();
    let own: std::collections::HashSet<String> = sm
        .states
        .get(state_name)
        .map(|s| s.variables.keys().map(|k| canon(k)).collect())
        .unwrap_or_default();
    let defaults: HashMap<String, &String> = sm
        .states
        .get("_default")
        .map(|s| s.variables.iter().map(|(k, v)| (canon(k), v)).collect())
        .unwrap_or_default();
    let mut foreign: Vec<String> = sm
        .states
        .iter()
        .filter(|(name, _)| name.as_str() != "_default" && name.as_str() != state_name)
        .flat_map(|(_, s)| s.variables.keys().map(|k| canon(k)))
        .filter(|key| !own.contains(key))
        .collect();
    foreign.sort();
    foreign.dedup();
    for key in foreign {
        match defaults.get(&key) {
            Some(value) => set_nested_value(context, &key, parse_json_or_string(value)),
            None => remove_nested_value(context, &key),
        }
    }
}

fn resolve_auto_state(
    sm: &StateMachine,
    context: &serde_json::Map<String, serde_json::Value>,
    current_state: &str,
) -> String {
    for rule in &sm.auto_rules {
        if evaluate_condition(&rule.condition, context) {
            return rule.target_state.clone();
        }
    }
    // Fall back to current state if set, otherwise first step or first state key.
    // `_default` holds fallback variables and is never a real state.
    if !current_state.is_empty() && current_state != "_default" && sm.states.contains_key(current_state) {
        return current_state.to_string();
    }
    sm.steps
        .first()
        .cloned()
        .or_else(|| sm.states.keys().filter(|name| name.as_str() != "_default").min().cloned())
        .unwrap_or_default()
}

/// Evaluate a condition against context. Supports ==, !=, >, <, >=, <=, &&, ||.
pub fn evaluate_condition(
    condition: &str,
    context: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let condition = condition.trim();

    if condition.contains("&&") {
        let parts = split_condition_preserving_quotes(condition, "&&");
        if parts.len() > 1 {
            return parts
                .iter()
                .all(|part| evaluate_condition(part.trim(), context));
        }
    }

    if condition.contains("||") {
        let parts = split_condition_preserving_quotes(condition, "||");
        if parts.len() > 1 {
            return parts
                .iter()
                .any(|part| evaluate_condition(part.trim(), context));
        }
    }

    // Check regex operators first (=~ and !~) before == and != to avoid conflicts
    let (var, op, value) = if let Some(pos) = condition.find("=~") {
        (condition[..pos].trim(), "=~", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find("!~") {
        (condition[..pos].trim(), "!~", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find("==") {
        (condition[..pos].trim(), "==", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find("!=") {
        (condition[..pos].trim(), "!=", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find(">=") {
        (condition[..pos].trim(), ">=", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find("<=") {
        (condition[..pos].trim(), "<=", condition[pos + 2..].trim())
    } else if let Some(pos) = condition.find('>') {
        (condition[..pos].trim(), ">", condition[pos + 1..].trim())
    } else if let Some(pos) = condition.find('<') {
        (condition[..pos].trim(), "<", condition[pos + 1..].trim())
    } else {
        // Simple truthy check
        let val = context.get(condition);
        return match val {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => !s.is_empty() && s != "false",
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            Some(serde_json::Value::Null) | None => false,
            _ => true,
        };
    };

    let ctx_val = get_nested_value(context, var);

    let resolved;
    let clean_value = if value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else if let Some(serde_json::Value::String(s)) = get_nested_value(context, value) {
        resolved = s.clone();
        resolved.as_str()
    } else {
        value
    };

    match op {
        "=~" => {
            let haystack = match ctx_val {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(serde_json::Value::Bool(b)) => b.to_string(),
                _ => return false,
            };
            match get_cached_regex(clean_value) {
                Some(re) => re.is_match(&haystack),
                None => false,
            }
        }
        "!~" => {
            let haystack = match ctx_val {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(serde_json::Value::Bool(b)) => b.to_string(),
                _ => return true,
            };
            match get_cached_regex(clean_value) {
                Some(re) => !re.is_match(&haystack),
                None => true,
            }
        }
        "==" => match ctx_val {
            Some(serde_json::Value::String(s)) => s == clean_value,
            Some(serde_json::Value::Number(n)) => {
                if let Ok(num) = clean_value.parse::<f64>() {
                    n.as_f64().unwrap_or(0.0) == num
                } else {
                    false
                }
            }
            Some(serde_json::Value::Bool(b)) => clean_value == b.to_string(),
            _ => false,
        },
        "!=" => match ctx_val {
            Some(serde_json::Value::String(s)) => s != clean_value,
            Some(serde_json::Value::Number(n)) => {
                if let Ok(num) = clean_value.parse::<f64>() {
                    n.as_f64().unwrap_or(0.0) != num
                } else {
                    true
                }
            }
            _ => true,
        },
        ">" => compare_numbers(ctx_val, clean_value, |a, b| a > b),
        "<" => compare_numbers(ctx_val, clean_value, |a, b| a < b),
        ">=" => compare_numbers(ctx_val, clean_value, |a, b| a >= b),
        "<=" => compare_numbers(ctx_val, clean_value, |a, b| a <= b),
        _ => false,
    }
}

fn compare_numbers<F>(ctx_val: Option<&serde_json::Value>, value_str: &str, cmp: F) -> bool
where
    F: Fn(f64, f64) -> bool,
{
    let num = match value_str.parse::<f64>() {
        Ok(n) => n,
        Err(_) => return false,
    };
    match ctx_val {
        Some(serde_json::Value::Number(n)) => cmp(n.as_f64().unwrap_or(0.0), num),
        Some(serde_json::Value::String(s)) => {
            if let Ok(n) = s.parse::<f64>() {
                cmp(n, num)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Resolve workflows under contexts/. A missing .sm permits legacy .cl;
/// malformed or unreadable .sm files never fall through to a different file.
pub fn load_file(path: &str) -> Result<StateMachine, SmError> {
    load_file_in(std::path::Path::new("contexts"), path)
}

pub fn load_file_in(root: &std::path::Path, path: &str) -> Result<StateMachine, SmError> {
    parse(&std::fs::read_to_string(resolve_file_in(root, path)?)?)
}

pub fn resolve_file_in(root: &std::path::Path, path: &str) -> Result<std::path::PathBuf, SmError> {
    let name = path.strip_prefix("./contexts/").or_else(|| path.strip_prefix("contexts/")).unwrap_or(path);
    let stem = name.strip_suffix(".sm").or_else(|| name.strip_suffix(".cl")).unwrap_or(name);
    if !stem.split('/').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
        return Err(SmError::ParseError("SM name must be a relative workflow name inside contexts/".into()));
    }
    let display_root = if root.is_absolute() { root.to_path_buf() }
        else { std::env::current_dir().map(|cwd| cwd.join(root)).unwrap_or_else(|_| root.to_path_buf()) };
    let root = root.canonicalize().map_err(|error| SmError::IoError(std::io::Error::new(
        error.kind(), format!("Workflow directory '{}' is unavailable while selecting '{path}': {error}. Check the installation working directory and restore missing bundled files with `praxis repair-assets --directory <installation-dir>`.", display_root.display())
    )))?;
    let candidates = if name.ends_with(".cl") { vec![format!("{stem}.cl")] }
        else if name.ends_with(".sm") { vec![format!("{stem}.sm")] }
        else { vec![format!("{stem}.sm"), format!("{stem}.cl")] };
    for candidate in candidates {
        let file = match root.join(candidate).canonicalize() {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if !file.starts_with(&root) {
            return Err(SmError::ParseError("SM file escapes contexts/".into()));
        }
        return Ok(file);
    }
    Err(SmError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, format!("No workflow found for '{path}' in '{}'. Restore the selected custom workflow, or restore bundled defaults with `praxis repair-assets --directory <installation-dir>`.", root.display()))))
}

/// Dashboard saves use the runtime root, not a shadow data/contexts directory.
/// A single file name is accepted here; nested workflow paths remain readable.
pub fn save_file_in(root: &std::path::Path, name: &str, content: &str) -> anyhow::Result<()> {
    let stem = name.strip_suffix(".sm").or_else(|| name.strip_suffix(".cl")).unwrap_or(name);
    anyhow::ensure!(!stem.is_empty() && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'), "Invalid SM file name");
    parse(content).map_err(|e| anyhow::anyhow!("{e}"))?;
    std::fs::create_dir_all(root)?;
    let name = if name.ends_with(".sm") || name.ends_with(".cl") { name.to_string() } else { format!("{name}.sm") };
    let destination = root.join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&destination) {
        anyhow::ensure!(!meta.file_type().is_symlink(), "SM destination must not be a symlink");
    }
    let temporary = tempfile::NamedTempFile::new_in(root)?;
    std::fs::write(temporary.path(), content)?;
    temporary.persist(destination).map_err(|e| e.error)?;
    Ok(())
}

/// Evaluate transitions and advance to next state if condition is met.
pub fn advance_state(sm: &StateMachine, context: &serde_json::Value) -> Option<String> {
    let obj = match context.as_object() {
        Some(o) => o,
        None => return None,
    };

    let current_state = obj
        .get("active_state")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    for transition in &sm.transitions {
        if transition.from == current_state && (transition.condition.trim().is_empty() || evaluate_condition(&transition.condition, obj)) {
            return Some(transition.to.clone());
        }
    }

    None
}

/// Explicit transitions take precedence; @steps provides a queue only where
/// the current state has no outgoing conditional transition to respect.
pub fn advance_workflow(sm: &StateMachine, context: &serde_json::Value) -> Option<String> {
    if let Some(next) = advance_state(sm, context) { return Some(next); }
    let current = context.get("active_state").and_then(|v| v.as_str()).unwrap_or("");
    if sm.transitions.iter().any(|t| t.from == current) { return None; }
    next_step_state(sm, context)
}

/// Force transition to a specific state.
pub fn transition_to(
    sm: &StateMachine,
    context: &mut serde_json::Value,
    target_state: &str,
) -> bool {
    let user = context.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
    if crate::gateway::action_contracts::require_for_workflow(user, sm, target_state).is_err() {
        return false;
    }
    if let Some(state) = sm.states.get(target_state) {
        // Persistent skill selection belongs to the user, not automated state
        // transitions (including agent_next and tag-driven transitions).
        let before = context.pointer("/settings/active_skill").cloned();
        let mut candidate = context.clone();
        for (key, value) in &state.variables {
            set_nested_value(&mut candidate, key, parse_json_or_string(value));
        }
        if candidate.pointer("/settings/active_skill").cloned() != before { return false; }
        // Same typing as apply_to_context: arrays/objects/bools stay JSON.
        for (key, value) in &state.variables {
            set_nested_value(context, key, parse_json_or_string(value));
        }
        reset_foreign_state_variables(sm, context, target_state);
        set_nested_value(
            context,
            "active_state",
            serde_json::Value::String(target_state.to_string()),
        );
        true
    } else {
        false
    }
}

fn parse_json_or_string(s: &str) -> serde_json::Value {
    // Try to parse as JSON first (for arrays, objects)
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        return v;
    }
    // Fallback to string
    serde_json::Value::String(s.to_string())
}

fn set_nested_value(context: &mut serde_json::Value, path: &str, value: serde_json::Value) {
    let path = crate::db::contexts::canonical_context_key(path);
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let mut current = context;
    for part in &parts[..parts.len() - 1] {
        if !current.is_object() {
            *current = serde_json::json!({});
        }
        if !current.as_object().unwrap().contains_key(*part) {
            current
                .as_object_mut()
                .unwrap()
                .insert(part.to_string(), serde_json::json!({}));
        }
        current = current.as_object_mut().unwrap().get_mut(*part).unwrap();
    }

    // Optional namespaces such as sm_data start as null in a fresh context.
    // The final parent must become an object too, not silently drop state vars.
    if !current.is_object() {
        *current = serde_json::json!({});
    }
    if let Some(obj) = current.as_object_mut() {
        obj.insert(parts.last().unwrap().to_string(), value);
    }
}

fn remove_nested_value(context: &mut serde_json::Value, path: &str) {
    let path = crate::db::contexts::canonical_context_key(path);
    let parts: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = parts.split_last() else { return };
    let mut current = context;
    for part in parents {
        match current.get_mut(*part) {
            Some(next) => current = next,
            None => return,
        }
    }
    if let Some(obj) = current.as_object_mut() {
        obj.remove(*last);
    }
}

fn get_nested_value<'a>(context: &'a serde_json::Map<String, serde_json::Value>, path: &str) -> Option<&'a serde_json::Value> {
    let path = crate::db::contexts::canonical_context_key(path);
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let mut current = context.get(parts[0])?;
    for part in &parts[1..] {
        current = current.get(part)?;
    }
    Some(current)
}

/// Get the next state in the @steps sequence.
pub fn next_step_state(sm: &StateMachine, context: &serde_json::Value) -> Option<String> {
    let obj = match context.as_object() {
        Some(o) => o,
        None => return None,
    };

    let current_state = obj
        .get("active_state")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let pos = sm.steps.iter().position(|s| s == current_state);
    match pos {
        Some(i) if i + 1 < sm.steps.len() => Some(sm.steps[i + 1].clone()),
        _ => None,
    }
}

#[cfg(test)]
mod sm_tests {
    use super::*;

    #[test]
    fn backend_step_queue_and_transition_guards() {
        let sm = parse("@steps [one, two]\n[state one]\n[state two]\n").unwrap();
        let context = serde_json::json!({"active_state":"one", "ready":false});
        assert_eq!(advance_workflow(&sm, &context).as_deref(), Some("two"));
        let guarded = parse("@steps [one, two]\n[state one]\n[state two]\n[transitions]\none -> two : when ready == true\n").unwrap();
        assert!(advance_workflow(&guarded, &context).is_none());
        let unguarded = parse("[state one]\n[state two]\n[transitions]\none -> two\n").unwrap();
        assert_eq!(advance_workflow(&unguarded, &context).as_deref(), Some("two"));
    }

    #[test]
    fn onboarding_missing_workflow_errors_identify_the_directory_and_repair() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("contexts");
        let error = load_file_in(&root, "standard").unwrap_err().to_string();
        assert!(error.contains(root.to_str().unwrap()));
        assert!(error.contains("standard") && error.contains("repair-assets"));
        std::fs::create_dir(&root).unwrap();
        let error = load_file_in(&root, "custom-selected").unwrap_err().to_string();
        assert!(error.contains("custom-selected") && error.contains(root.to_str().unwrap()));
    }

    #[test]
    fn backend_sm_save_validates_before_replacing() {
        let root = tempfile::tempdir().unwrap();
        save_file_in(root.path(), "standard", "@name old").unwrap();
        assert!(save_file_in(root.path(), "standard", "[invalid]").is_err());
        assert_eq!(load_file_in(root.path(), "standard").unwrap().name, "old");
        assert!(save_file_in(root.path(), "../escape", "@name bad").is_err());
        save_file_in(root.path(), "standard.sm", "@name new").unwrap();
        assert_eq!(load_file_in(root.path(), "standard").unwrap().name, "new");
    }

    #[test]
    fn backend_workflow_resolution_prefers_sm_without_masking_errors() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("standard.cl"), "@name legacy").unwrap();
        assert_eq!(load_file_in(root.path(), "standard").unwrap().name, "legacy");
        std::fs::write(root.path().join("standard.sm"), "@name canonical").unwrap();
        assert_eq!(load_file_in(root.path(), "contexts/standard").unwrap().name, "canonical");
        assert_eq!(load_file_in(root.path(), "standard.cl").unwrap().name, "legacy");
        std::fs::write(root.path().join("standard.sm"), "[invalid]").unwrap();
        assert!(load_file_in(root.path(), "standard").is_err());
        assert!(load_file_in(root.path(), "../standard").is_err());
    }


    #[test]
    fn test_parse_basic() {
        let input = r#"
@name "Test Workflow"
@version "1.0"
@steps [calm, focused]

[state calm]
mode = "chat"
voice = "calm.wav"

[state focused]
mode = "code"
voice = "focused.wav"

[auto]
step == 1 -> use calm
step == 2 -> use focused

[overrides]
if hour < 6 -> style = "whisper"
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.name, "Test Workflow");
        assert_eq!(sm.version, "1.0");
        assert_eq!(sm.steps, vec!["calm", "focused"]);
        assert_eq!(sm.states.len(), 2);
        assert_eq!(sm.auto_rules.len(), 2);
        assert_eq!(sm.overrides.len(), 1);
    }

    #[test]
    fn test_parse_state_variables() {
        let input = r#"
[state calm]
mode = "chat"
voice = "calm.wav"
"#;
        let sm = parse(input).unwrap();
        let state = sm.states.get("calm").unwrap();
        assert_eq!(state.variables.get("mode").unwrap(), "chat");
        assert_eq!(state.variables.get("voice").unwrap(), "calm.wav");
    }

    #[test]
    fn test_parse_transitions() {
        let input = r#"
[transitions]
calm -> focused : when step > 1
focused -> calm : when reset
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.transitions.len(), 2);
        assert_eq!(sm.transitions[0].from, "calm");
        assert_eq!(sm.transitions[0].to, "focused");
        assert_eq!(sm.transitions[0].condition, "step > 1");
    }

    #[test]
    fn test_parse_auto_rules() {
        let input = r#"
[auto]
step == 1 -> use calm
step == 2 -> use focused
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.auto_rules.len(), 2);
        assert_eq!(sm.auto_rules[0].condition, "step == 1");
        assert_eq!(sm.auto_rules[0].target_state, "calm");
    }

    #[test]
    fn test_parse_overrides() {
        let input = r#"
[overrides]
if hour < 6 -> style = "whisper"
if mode == "code" -> voice = "focused.wav"
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.overrides.len(), 2);
        assert_eq!(sm.overrides[0].condition, "hour < 6");
        assert_eq!(sm.overrides[0].key, "style");
        assert_eq!(sm.overrides[0].value, "whisper");
    }

    #[test]
    fn test_parse_secret_overrides() {
        let input = r#"
[secrets]
if mode == "code" -> mimo_voice = "coder"
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.secret_overrides.len(), 1);
        assert_eq!(sm.secret_overrides[0].key, "mimo_voice");
    }

    #[test]
    fn test_parse_secret_overrides_invalid_field() {
        let input = r#"
[secrets]
if mode == "code" -> invalid_field = "value"
"#;
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_evaluate_condition_eq() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("step".to_string(), serde_json::json!(1));
        ctx.insert("mode".to_string(), serde_json::json!("chat"));

        assert!(evaluate_condition("step == 1", &ctx));
        assert!(!evaluate_condition("step == 2", &ctx));
        assert!(evaluate_condition("mode == \"chat\"", &ctx));
    }

    #[test]
    fn test_evaluate_condition_neq() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("step".to_string(), serde_json::json!(1));

        assert!(evaluate_condition("step != 2", &ctx));
        assert!(!evaluate_condition("step != 1", &ctx));
    }

    #[test]
    fn test_evaluate_condition_gt_lt() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("step".to_string(), serde_json::json!(5));

        assert!(evaluate_condition("step > 3", &ctx));
        assert!(!evaluate_condition("step > 5", &ctx));
        assert!(evaluate_condition("step < 10", &ctx));
        assert!(evaluate_condition("step >= 5", &ctx));
        assert!(evaluate_condition("step <= 5", &ctx));
    }

    #[test]
    fn test_evaluate_condition_compound() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("step".to_string(), serde_json::json!(1));
        ctx.insert("mode".to_string(), serde_json::json!("chat"));

        assert!(evaluate_condition("step == 1 && mode == \"chat\"", &ctx));
        assert!(!evaluate_condition("step == 1 && mode == \"code\"", &ctx));
        assert!(evaluate_condition("step == 1 || mode == \"code\"", &ctx));
    }

    #[test]
    fn test_evaluate_condition_truthy() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("flag".to_string(), serde_json::json!(true));
        ctx.insert("empty".to_string(), serde_json::json!(""));

        assert!(evaluate_condition("flag", &ctx));
        assert!(!evaluate_condition("empty", &ctx));
        assert!(!evaluate_condition("nonexistent", &ctx));
    }

    #[test]
    fn test_apply_to_context() {
        let input = r#"
[state calm]
mode = "chat"
voice = "calm.wav"

[state focused]
mode = "code"
voice = "focused.wav"

[auto]
step == 1 -> use calm
step == 2 -> use focused
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({"step": 1});
        apply_to_context(&sm, &mut ctx);

        assert_eq!(ctx["mode"], "chat");
        assert_eq!(ctx["voice"], "calm.wav");
        assert_eq!(ctx["active_state"], "calm");
    }

    #[test]
    fn test_apply_overrides() {
        let input = r#"
[state calm]
mode = "chat"

[overrides]
if hour < 6 -> mode = "whisper"
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({"step": 1, "hour": 3});
        apply_to_context(&sm, &mut ctx);

        assert_eq!(ctx["mode"], "whisper");
    }

    #[test]
    fn test_advance_state() {
        let input = r#"
@steps [calm, focused]

[state calm]
mode = "chat"

[state focused]
mode = "code"

[transitions]
calm -> focused : when step > 1
"#;
        let sm = parse(input).unwrap();
        let ctx = serde_json::json!({"active_state": "calm", "step": 2});

        let new_state = advance_state(&sm, &ctx);
        assert_eq!(new_state, Some("focused".to_string()));
    }

    #[test]
    fn test_advance_state_no_transition() {
        let input = r#"
[state calm]
mode = "chat"

[transitions]
calm -> focused : when step > 1
"#;
        let sm = parse(input).unwrap();
        let ctx = serde_json::json!({"active_state": "calm", "step": 1});

        let new_state = advance_state(&sm, &ctx);
        assert!(new_state.is_none());
    }

    #[test]
    fn test_transition_to() {
        let input = r#"
[state calm]
mode = "chat"

[state focused]
mode = "code"
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({"active_state": "calm"});

        let success = transition_to(&sm, &mut ctx, "focused");
        assert!(success);
        assert_eq!(ctx["mode"], "code");
        assert_eq!(ctx["active_state"], "focused");
    }

    #[test]
    fn test_transition_to_nonexistent() {
        let input = r#"
[state calm]
mode = "chat"
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({"active_state": "calm"});

        let success = transition_to(&sm, &mut ctx, "nonexistent");
        assert!(!success);
    }

    #[test]
    fn test_next_step_state() {
        let input = r#"
@steps [calm, focused, review]

[state calm]
[state focused]
[state review]
"#;
        let sm = parse(input).unwrap();
        let ctx = serde_json::json!({"active_state": "calm"});

        let next = next_step_state(&sm, &ctx);
        assert_eq!(next, Some("focused".to_string()));
    }

    #[test]
    fn test_next_step_state_last() {
        let input = r#"
@steps [calm, focused]

[state calm]
[state focused]
"#;
        let sm = parse(input).unwrap();
        let ctx = serde_json::json!({"active_state": "focused"});

        let next = next_step_state(&sm, &ctx);
        assert!(next.is_none());
    }

    #[test]
    fn test_parse_empty() {
        let sm = parse("").unwrap();
        assert!(sm.states.is_empty());
    }

    #[test]
    fn test_parse_unknown_section() {
        let input = "[unknown]";
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_workflow() {
        let input = r#"
@name "default"
@version "1.0"
@steps [understand, plan, code, review]

[state understand]
mode = "agent"
task = "understand the task"

[state plan]
mode = "agent"
task = "create a plan"

[state code]
mode = "code"
task = "write code"

[state review]
mode = "agent"
task = "review code"

[transitions]
understand -> plan : when understood
plan -> code : when approved
code -> review : when done

[auto]
turn > 10 -> use plan
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.name, "default");
        assert_eq!(sm.steps.len(), 4);
        assert_eq!(sm.states.len(), 4);
        assert_eq!(sm.transitions.len(), 3);
        assert_eq!(sm.auto_rules.len(), 1);
    }

    #[test]
    fn test_evaluate_condition_regex_match() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("used_tools".to_string(), serde_json::json!({
            "last_call": "execute_terminal",
            "last_result": "partition sda1 created successfully",
            "count": 1
        }));

        assert!(evaluate_condition("used_tools.last_call =~ \"execute_terminal\"", &ctx));
        assert!(evaluate_condition("used_tools.last_call =~ \"execute\"", &ctx));
        assert!(evaluate_condition("used_tools.last_call =~ \"^execute_terminal$\"", &ctx));
        assert!(evaluate_condition("used_tools.last_result =~ \"partition.*created\"", &ctx));
        assert!(evaluate_condition("used_tools.last_result =~ \"(?i)PARTITION.*CREATED\"", &ctx));
        assert!(evaluate_condition("used_tools.last_call =~ \"^(write_file|execute_terminal)$\"", &ctx));
    }

    #[test]
    fn test_evaluate_condition_regex_no_match() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("used_tools".to_string(), serde_json::json!({
            "last_call": "write_file",
            "count": 1
        }));

        assert!(!evaluate_condition("used_tools.last_call =~ \"execute_terminal\"", &ctx));
        assert!(evaluate_condition("used_tools.last_call !~ \"execute_terminal\"", &ctx));
        assert!(!evaluate_condition("used_tools.last_call !~ \"write_file\"", &ctx));
        assert!(evaluate_condition("used_tools.last_call =~ \"^read_file$\"", &ctx) == false);
    }

    #[test]
    fn test_evaluate_condition_regex_with_context() {
        let input = r#"
[state installing]
settings.system_template = "installing"

[auto]
used_tools.last_result =~ "partition.*created" -> use installing
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({
            "active_state": "partitioning",
            "used_tools": {
                "last_call": "execute_terminal",
                "last_result": "partition sda1 created successfully",
                "count": 1
            }
        });
        let secrets = apply_to_context(&sm, &mut ctx);
        assert!(secrets.is_empty());
        assert_eq!(ctx["active_state"], "installing");
    }

    #[test]
    fn test_evaluate_condition_regex_invalid() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("val".to_string(), serde_json::json!("test"));

        // Invalid regex should return false, not panic
        assert!(!evaluate_condition("val =~ \"[invalid\"", &ctx));
        assert!(evaluate_condition("val !~ \"[invalid\"", &ctx));
    }

    #[test]
    fn test_evaluate_condition_regex_number() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("count".to_string(), serde_json::json!(42));

        assert!(evaluate_condition("count =~ \"42\"", &ctx));
        assert!(!evaluate_condition("count =~ \"^99$\"", &ctx));
    }

    #[test]
    fn test_evaluate_condition_regex_with_ampersand_in_pattern() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("val".to_string(), serde_json::json!("a&&b"));
        ctx.insert("mode".to_string(), serde_json::json!("chat"));

        assert!(evaluate_condition("val =~ \"a&&b\" && mode == \"chat\"", &ctx));
        assert!(!evaluate_condition("val =~ \"a&&b\" && mode == \"code\"", &ctx));
        assert!(evaluate_condition("val =~ \"a&&b\" || mode == \"code\"", &ctx));
    }

    #[test]
    fn test_split_condition_preserving_quotes() {
        let parts = split_condition_preserving_quotes("val =~ \"a&&b\" && mode == \"chat\"", "&&");
        assert_eq!(parts, vec!["val =~ \"a&&b\" ", " mode == \"chat\""]);

        let parts = split_condition_preserving_quotes("a && b && c", "&&");
        assert_eq!(parts, vec!["a ", " b ", " c"]);

        let parts = split_condition_preserving_quotes("a || b", "||");
        assert_eq!(parts, vec!["a ", " b"]);
    }

    #[test]
    fn test_evaluate_condition_rhs_variable_resolution() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("val".to_string(), serde_json::json!("hello world"));
        ctx.insert("my_regex".to_string(), serde_json::json!("hello.*"));
        ctx.insert("exact".to_string(), serde_json::json!("^hello world$"));
        ctx.insert("num".to_string(), serde_json::json!(10));
        ctx.insert("threshold".to_string(), serde_json::json!("5"));

        // Unquoted RHS resolves as context variable
        assert!(evaluate_condition("val =~ my_regex", &ctx));
        assert!(evaluate_condition("val =~ exact", &ctx));
        assert!(evaluate_condition("num > threshold", &ctx));

        // Quoted RHS is always a literal
        assert!(!evaluate_condition("val =~ \"my_regex\"", &ctx));
        assert!(!evaluate_condition("val == \"my_regex\"", &ctx));
    }

    #[test]
    fn test_state_variables_available_in_auto_rules() {
        let input = r#"
[state working]
tool_regex = "vm_input"

[state done]
mode = "idle"

[auto]
used_tools.last_call =~ tool_regex && used_tools.last_args.action == "left" -> use done
"#;
        let sm = parse(input).unwrap();
        let mut ctx = serde_json::json!({
            "active_state": "working",
            "used_tools": {
                "last_call": "vm_input",
                "last_args": {"action": "left", "count": 3}
            }
        });
        apply_to_context(&sm, &mut ctx);

        assert_eq!(ctx["active_state"], "done");
        assert_eq!(ctx["mode"], "idle");
    }
}

#[cfg(test)]
mod sm_tool_group_tests {
    use super::*;

    #[test]
    fn tool_groups_section_defines_groups_without_activating_them() {
        let input = r#"
@steps [research, chat]

[tool_groups]
web = [brave_web_search, "read_file"]
agent_basic = [agent_complete, agent_next]
research_kit = [web, agent_basic]

[state research]
settings.activated_tools = ["research_kit", "execute_terminal"]

[state chat]
settings.activated_tools = ["agent_basic"]
"#;
        let sm = parse(input).unwrap();
        assert_eq!(sm.tool_groups["web"], vec!["brave_web_search", "read_file"]);
        assert_eq!(sm.tool_groups["research_kit"], vec!["web", "agent_basic"]);

        let mut ctx = serde_json::json!({
            "active_state": "chat",
            "settings": {"tool_group_definitions": {"web": ["stale"], "mine": ["memory_get"]}}
        });
        apply_to_context(&sm, &mut ctx);
        let defs = &ctx["settings"]["tool_group_definitions"];
        assert_eq!(defs["web"], serde_json::json!(["brave_web_search", "read_file"]), "workflow wins for same name");
        assert_eq!(defs["mine"], serde_json::json!(["memory_get"]), "user groups are preserved");
        assert_eq!(ctx["settings"]["activated_tools"], serde_json::json!(["agent_basic"]), "defining does not activate");

        let settings: crate::db::contexts::ContextSettings = serde_json::from_value(ctx["settings"].clone()).unwrap();
        let expanded = crate::tools::registry::expand_tool_groups(&["research_kit".into(), "execute_terminal".into()], &settings.tool_group_definitions);
        assert_eq!(expanded, vec!["brave_web_search", "read_file", "agent_complete", "agent_next", "execute_terminal"]);
    }

    #[test]
    fn transition_to_keeps_json_typed_state_values() {
        let sm = parse("[state a]\nsettings.activated_tools = [\"read_file\"]\nsettings.tags_enabled = true\n").unwrap();
        let mut ctx = serde_json::json!({"active_state": "start", "settings": {}});
        assert!(transition_to(&sm, &mut ctx, "a"));
        assert_eq!(ctx["settings"]["activated_tools"], serde_json::json!(["read_file"]));
        assert_eq!(ctx["settings"]["tags_enabled"], serde_json::json!(true));
        let _: crate::db::contexts::ContextSettings = serde_json::from_value(ctx["settings"].clone()).unwrap();
    }

    #[test]
    fn state_variables_do_not_leak_into_other_states() {
        let sm = parse(r#"
settings.tool_discovery_mode = "DescriptionOnly"
[state a]
settings.system_template = "a"
settings.activated_tools = ["read_file"]
settings.tool_discovery_mode = "Full"
[state b]
settings.system_template = "b"
"#).unwrap();
        let mut ctx = serde_json::json!({"active_state": "a", "settings": {"tags_enabled": true}});
        apply_to_context(&sm, &mut ctx);
        assert_eq!(ctx["settings"]["tool_discovery_mode"], "Full");
        ctx["active_state"] = serde_json::json!("b");
        apply_to_context(&sm, &mut ctx);
        assert_eq!(ctx["settings"]["system_template"], "b");
        assert!(ctx["settings"].get("activated_tools").is_none(), "foreign state variable removed");
        assert_eq!(ctx["settings"]["tool_discovery_mode"], "DescriptionOnly", "falls back to _default");
        assert_eq!(ctx["settings"]["tags_enabled"], true, "unmanaged keys untouched");
        assert!(transition_to(&sm, &mut ctx, "a"));
        assert_eq!(ctx["settings"]["activated_tools"], serde_json::json!(["read_file"]));
        // _default is not a selectable state.
        ctx["active_state"] = serde_json::json!("_default");
        apply_to_context(&sm, &mut ctx);
        assert_eq!(ctx["active_state"], "a");
    }

    #[test]
    fn auto_rules_yield_to_an_active_decision_profile() {
        let sm = parse("[state a]\n[state b]\n[auto]\nsm_data.role == \"b\" -> use b\n").unwrap();
        let mut ctx = serde_json::json!({"active_state": "a", "sm_data": {"role": "b"}, "settings": {"decision_profile": "tasks"}});
        apply_to_context(&sm, &mut ctx);
        assert_eq!(ctx["active_state"], "a");
        ctx["settings"]["decision_profile"] = serde_json::json!("off");
        apply_to_context(&sm, &mut ctx);
        assert_eq!(ctx["active_state"], "b");
    }

    #[test]
    fn tool_group_names_are_validated() {
        assert!(parse("[tool_groups]\nbad.name = [read_file]\n").is_err());
        assert!(parse("[tool_groups]\n= [read_file]\n").is_err());
        assert!(parse("[tool_groups]\nok_name-1 = read_file\n").is_ok());
    }
}
