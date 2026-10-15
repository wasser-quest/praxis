use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;
use tempfile::NamedTempFile;

static SET_VAR_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[\[([a-zA-Z_][a-zA-Z0-9_.]*):((?:[^\[\]]*(?:\[[^\[\]]*\])?)*)\]\]").unwrap()
});

static DELETE_VAR_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([a-zA-Z_][a-zA-Z0-9_]*)~\]\]").unwrap());

static READ_VAR_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([a-zA-Z_][a-zA-Z0-9_]*)\]\]").unwrap());

static THINK_TAG_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<think>([\s\S]*?)</think>").unwrap());

#[allow(dead_code)]
static AGENT_SIGNAL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\[\[AGENT:(\w+)(?::"([^"]*)")?\]\]"#).unwrap());

#[derive(Debug, Clone)]
pub enum VariableEffect {
    Set(String, serde_json::Value),
    Delete(String),
}

#[derive(Debug, Clone)]
pub enum AgentSignal {
    Next,
    Template(String),
    Push(String),
    Pop,
    Input(String),
    Spawn(String),
    Complete,
    Path(String),
    Set(String, String),
    Feedback(String),
    Summarize,
}

pub async fn render(template_path: &str, context: &serde_json::Value) -> anyhow::Result<String> {
    if super::jinja::handles(std::path::Path::new(template_path)) {
        return super::jinja::render(std::path::Path::new(template_path), context, None);
    }
    tracing::info!(template = %template_path, "[POML] rendering template");
    let poml_cli = std::env::var("POML_CLI").unwrap_or_else(|_| "poml".to_string());

    let context_file = NamedTempFile::new()?;
    std::fs::write(context_file.path(), serde_json::to_string_pretty(context)?)?;

    let output = Command::new("node")
        .arg(&poml_cli)
        .arg("--file")
        .arg(template_path)
        .arg("--context-file")
        .arg(context_file.path())
        .arg("--prettyPrint")
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let result = String::from_utf8(output.stdout)?;
            tracing::info!(template = %template_path, result_len = result.len(), "[POML] render success");
            Ok(strip_think_tags(&result))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(template = %template_path, "[POML] render error, falling back to simple: {}", stderr);
            render_simple(template_path, context).await
        }
        Err(e) => {
            tracing::warn!(template = %template_path, "[POML] CLI not found: {}, using simple template", e);
            render_simple(template_path, context).await
        }
    }
}

/// Render instructions/validation with the real Microsoft CLI, without fallback.
/// Unlike `render`, this returns plain text rather than speaker-mode JSON.
/// The caller supplies only the variables the template needs, not stored secrets.
pub async fn render_strict(
    template_path: &str,
    context: &serde_json::Value,
) -> anyhow::Result<String> {
    render_strict_candidate(template_path, context, None).await
}

/// For validation, redirect reads of the final destination to the candidate.
/// This also exposes direct/transitive self-includes to the real renderer.
pub(crate) async fn render_strict_candidate(
    template_path: &str,
    context: &serde_json::Value,
    destination: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    use anyhow::Context;
    // Markdown/Jinja templates render in-process; only legacy .poml needs Node.
    if super::jinja::handles(std::path::Path::new(template_path)) {
        return super::jinja::render(std::path::Path::new(template_path), context, destination);
    }
    let cli = std::env::var("POML_CLI")
        .context("Set POML_CLI to the installed Microsoft POML JavaScript CLI path")?;
    let context_file = NamedTempFile::new()?;
    // Canonical public/saved JSON is sm_data. Retain a render-only alias so
    // existing user-owned POML templates do not break during an upgrade.
    let mut render_context = context.clone();
    if let Some(data) = context.get("sm_data") {
        render_context["cl_data"] = data.clone();
    }
    std::fs::write(context_file.path(), serde_json::to_vec(&render_context)?)?;
    let mut command = tokio::process::Command::new("node");
    let _preload = if let Some(destination) = destination {
        let preload = tempfile::Builder::new().suffix(".cjs").tempfile()?;
        std::fs::write(preload.path(), include_str!("poml_overlay.cjs"))?;
        command
            .arg("--require")
            .arg(preload.path())
            .env("PRAXIS_POML_DESTINATION", destination)
            .env("PRAXIS_POML_CANDIDATE", template_path);
        Some(preload)
    } else {
        None
    };
    command
        .args(["--", &cli, "--file", template_path, "--context-file"])
        .arg(context_file.path())
        .args(["--strict", "--speakerMode=false"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("POML rendering timed out after 30 seconds")?
        .context("Unable to run Node/POML; check Node and POML_CLI")?;
    anyhow::ensure!(
        output.status.success(),
        "POML rendering failed: {}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(1500)
            .collect::<String>()
    );
    // The CLI always returns a JSON envelope, even with speaker mode disabled.
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("POML output is not a JSON result")?;
    let text = result
        .get("messages")
        .and_then(serde_json::Value::as_str)
        .context("POML output has no plain-text messages field")?;
    anyhow::ensure!(
        !text.trim().is_empty(),
        "POML rendering returned empty output"
    );
    Ok(text.trim().to_string())
}

async fn render_simple(template_path: &str, context: &serde_json::Value) -> anyhow::Result<String> {
    let template = std::fs::read_to_string(template_path)?;

    let mut result = template;
    if let Some(obj) = context.as_object() {
        for (key, value) in obj {
            let placeholder = format!("{{{{{}}}}}", key);
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => String::new(),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }

    Ok(result)
}

pub fn strip_think_tags(content: &str) -> String {
    let result = THINK_TAG_REGEX.replace_all(content, "");
    // Clean up extra whitespace left by stripped tags
    let re = regex::Regex::new(r"\n{3,}").unwrap();
    let result = re.replace_all(&result, "\n\n");
    result.trim().to_string()
}

pub fn convert_think_tags(content: &str) -> String {
    let result = THINK_TAG_REGEX.replace_all(content, |caps: &regex::Captures| {
        let inner = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if inner.is_empty() {
            "".to_string()
        } else {
            format!("[THINK]{}[/THINK]", inner)
        }
    });
    let re = regex::Regex::new(r"\n{3,}").unwrap();
    let result = re.replace_all(&result, "\n\n");
    result.trim().to_string()
}

/// Extract agent signals from content: [[AGENT:NEXT]], [[AGENT:COMPLETE]], etc.
pub fn extract_agent_signals(content: &str) -> (Vec<AgentSignal>, String) {
    let mut signals = Vec::new();
    let mut result = content.to_string();

    let re = regex::Regex::new(r"\[\[AGENT:(\w+)(?::([^]]*))?\]\]").unwrap();
    let caps: Vec<_> = re.captures_iter(content).collect();
    for cap in &caps {
        let signal_name = cap.get(1).unwrap().as_str().to_lowercase();
        let raw_arg = cap.get(2).map(|m| m.as_str().to_string());
        let arg = raw_arg.map(|s| {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s
            }
        });

        let signal = match (signal_name.as_str(), arg) {
            ("next", None) => Some(AgentSignal::Next),
            ("template", Some(v)) => Some(AgentSignal::Template(v)),
            ("push", Some(v)) => Some(AgentSignal::Push(v)),
            ("pop", None) => Some(AgentSignal::Pop),
            ("input", Some(v)) => Some(AgentSignal::Input(v)),
            ("spawn", Some(v)) => Some(AgentSignal::Spawn(v)),
            ("complete", None) => Some(AgentSignal::Complete),
            ("path", Some(v)) => Some(AgentSignal::Path(v)),
            ("set", Some(v)) => {
                if let Some((key, val)) = v.split_once(':') {
                    Some(AgentSignal::Set(key.to_string(), val.to_string()))
                } else {
                    None
                }
            }
            ("feedback", msg) => Some(AgentSignal::Feedback(msg.unwrap_or_default())),
            ("summarize", None) => Some(AgentSignal::Summarize),
            _ => None,
        };

        if let Some(sig) = signal {
            signals.push(sig);
            result = result.replace(cap.get(0).unwrap().as_str(), "");
        }
    }

    (signals, result.trim().to_string())
}

/// Resolve [[var]] references in content using provided variables.
pub fn resolve_variables_in_content(content: &str, variables: &serde_json::Value) -> String {
    let mut result = content.to_string();

    if let Some(vars) = variables.as_object() {
        for cap in READ_VAR_REGEX.captures_iter(content) {
            let full_match = cap.get(0).unwrap().as_str();
            let var_name = cap.get(1).unwrap().as_str();

            if let Some(value) = vars.get(var_name) {
                let replacement = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Array(arr) => {
                        serde_json::to_string(arr).unwrap_or_else(|_| value.to_string())
                    }
                    serde_json::Value::Object(obj) => {
                        serde_json::to_string(obj).unwrap_or_else(|_| value.to_string())
                    }
                    serde_json::Value::Null => "null".to_string(),
                };
                result = result.replace(full_match, &replacement);
            }
        }
    }

    result
}

/// Extract variable effects from content: [[key: value]] sets, [[key~]] deletes.
/// Returns (effects, cleaned_content).
/// IMPORTANT: Call extract_agent_signals FIRST to remove agent signals.
pub fn extract_variable_effects(template: &str) -> (Vec<VariableEffect>, String) {
    let mut effects = Vec::new();

    for cap in SET_VAR_REGEX.captures_iter(template) {
        let var_name = cap.get(1).unwrap().as_str().to_string();
        let raw_value = cap.get(2).unwrap().as_str().to_string();
        let trimmed = raw_value.trim();

        let value = serde_json::from_str::<serde_json::Value>(trimmed)
            .unwrap_or_else(|_| serde_json::json!(trimmed));
        effects.push(VariableEffect::Set(var_name, value));
    }

    let mut cleaned = template.to_string();
    for cap in SET_VAR_REGEX.captures_iter(template) {
        cleaned = cleaned.replace(cap.get(0).unwrap().as_str(), "");
    }

    for cap in DELETE_VAR_REGEX.captures_iter(&cleaned.clone()) {
        let var_name = cap.get(1).unwrap().as_str().to_string();
        effects.push(VariableEffect::Delete(var_name));
        cleaned = cleaned.replace(cap.get(0).unwrap().as_str(), "");
    }

    (effects, cleaned)
}

/// Validate content for LLM - escapes unmatched [[ and {{ that aren't valid syntax.
/// This prevents the LLM from being confused by template-like patterns in user content.
pub fn validate_content_for_llm(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut chars = content.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '{' && chars.peek().map(|(_, c)| *c) == Some('{') {
            // Found opening {{
            chars.next(); // consume second {
            let start = i + 2;
            let remaining = &content[start..];
            if let Some(end) = remaining.find("}}") {
                let var_content = &remaining[..end];
                // Check if it's a valid template variable: {{identifier}}
                if var_content.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !var_content.is_empty()
                {
                    // Valid, keep as-is
                    result.push_str("{{");
                    result.push_str(var_content);
                    result.push_str("}}");
                    for _ in 0..end + 2 {
                        chars.next();
                    }
                    continue;
                }
            }
            // Invalid/unmatched {{, escape it
            result.push_str("{{}}");
        } else if ch == '[' && chars.peek().map(|(_, c)| *c) == Some('[') {
            // Found opening [[
            chars.next(); // consume second [
            let start = i + 2;
            let remaining = &content[start..];
            if let Some(end) = remaining.find("]]") {
                let var_content = &remaining[..end];
                // Check if it's valid: [[key: value]] or [[key~]] or [[key]]
                if var_content.contains(':')
                    || var_content.contains('~')
                    || var_content
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                {
                    // Valid [[]] syntax, escape by wrapping in extra brackets
                    result.push_str("[[[");
                    result.push_str(var_content);
                    result.push_str("]]]");
                    for _ in 0..end + 2 {
                        chars.next();
                    }
                    continue;
                }
            }
            // Invalid/unmatched [[, keep as-is
            result.push('[');
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod poml_tests {
    use super::*;

    #[test]
    fn test_strip_think_tags() {
        let input = "Hello <think>thinking here</think> World";
        let result = strip_think_tags(input);
        assert_eq!(result, "Hello  World"); // two spaces because think tag was between
    }

    #[test]
    fn test_strip_think_tags_multiline() {
        let input = "Hello\n<think>\nmultiline\nthink\n</think>\nWorld";
        let result = strip_think_tags(input);
        assert_eq!(result, "Hello\n\nWorld"); // extra newline from stripped tag
    }

    #[test]
    fn test_extract_variable_effects_set() {
        let input = r#"Hello [[name: "Alice"]] World"#;
        let (effects, cleaned) = extract_variable_effects(input);
        assert_eq!(effects.len(), 1);
        assert_eq!(cleaned, "Hello  World");
        match &effects[0] {
            VariableEffect::Set(name, value) => {
                assert_eq!(name, "name");
                assert_eq!(value, &serde_json::json!("Alice"));
            }
            _ => panic!("Expected Set"),
        }
    }

    #[test]
    fn test_extract_variable_effects_delete() {
        let input = r#"Hello [[old_var~]] World"#;
        let (effects, cleaned) = extract_variable_effects(input);
        assert_eq!(effects.len(), 1);
        assert_eq!(cleaned, "Hello  World");
        match &effects[0] {
            VariableEffect::Delete(name) => assert_eq!(name, "old_var"),
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_extract_variable_effects_multiple() {
        let input = r#"[[a: "1"]] text [[b: "2"]] [[c~]]"#;
        let (effects, _cleaned) = extract_variable_effects(input);
        assert_eq!(effects.len(), 3);
    }

    #[test]
    fn test_extract_variable_effects_number() {
        let input = r#"[[count: 42]]"#;
        let (effects, _) = extract_variable_effects(input);
        match &effects[0] {
            VariableEffect::Set(name, value) => {
                assert_eq!(name, "count");
                assert_eq!(value, &serde_json::json!(42));
            }
            _ => panic!("Expected Set"),
        }
    }

    #[test]
    fn test_resolve_variables() {
        let content = r#"Hello [[name]], you are [[role]]!"#;
        let variables = serde_json::json!({
            "name": "Alice",
            "role": "developer"
        });
        let result = resolve_variables_in_content(content, &variables);
        assert_eq!(result, "Hello Alice, you are developer!");
    }

    #[test]
    fn test_resolve_variables_number() {
        let content = r#"Turn: [[turn]]"#;
        let variables = serde_json::json!({"turn": 5});
        let result = resolve_variables_in_content(content, &variables);
        assert_eq!(result, "Turn: 5");
    }

    #[test]
    fn test_resolve_variables_missing() {
        let content = r#"Hello [[name]] [[missing]]"#;
        let variables = serde_json::json!({"name": "Alice"});
        let result = resolve_variables_in_content(content, &variables);
        assert_eq!(result, "Hello Alice [[missing]]");
    }

    #[test]
    fn test_extract_agent_signals_next() {
        // Test regex directly first
        let re = regex::Regex::new(r#"\[\[AGENT:(\w+)(?::(?:"([^"]*)"|([^\]]*)))?\]\]"#).unwrap();
        let input = r#"Done [[AGENT:NEXT]]"#;
        let caps: Vec<_> = re.captures_iter(input).collect();
        assert_eq!(caps.len(), 1, "Regex should match [[AGENT:NEXT]]");

        let (signals, clean) = extract_agent_signals(input);
        assert_eq!(clean, "Done");
        assert_eq!(signals.len(), 1);
        assert!(matches!(signals[0], AgentSignal::Next));
    }

    #[test]
    fn test_extract_agent_signals_complete() {
        let (signals, clean) = extract_agent_signals(r#"Finished [[AGENT:COMPLETE]]"#);
        assert_eq!(clean, "Finished");
        assert!(matches!(signals[0], AgentSignal::Complete));
    }

    #[test]
    fn test_extract_agent_signals_push() {
        let (signals, _) = extract_agent_signals(r#"[[AGENT:push:"tasks/code"]]"#);
        assert!(matches!(&signals[0], AgentSignal::Push(s) if s == "tasks/code"));
    }

    #[test]
    fn test_extract_agent_signals_set() {
        let re = regex::Regex::new(r#"\[\[AGENT:(\w+)(?::(?:"([^"]*)"|([^\]]*)))?\]\]"#).unwrap();
        let input = r#"[[AGENT:set:key:value]]"#;
        let caps: Vec<_> = re.captures_iter(input).collect();
        assert_eq!(caps.len(), 1, "Should match");
        assert_eq!(caps[0].get(1).unwrap().as_str(), "set");
        assert!(
            caps[0].get(2).is_some() || caps[0].get(3).is_some(),
            "Should have arg"
        );

        let (signals, _) = extract_agent_signals(input);
        assert_eq!(signals.len(), 1);
        assert!(matches!(&signals[0], AgentSignal::Set(k, v) if k == "key" && v == "value"));
    }

    #[test]
    fn test_extract_agent_signals_feedback() {
        let (signals, _) = extract_agent_signals(r#"[[AGENT:feedback:"almost done"]]"#);
        assert!(matches!(&signals[0], AgentSignal::Feedback(s) if s == "almost done"));
    }

    #[test]
    fn test_extract_agent_signals_multiple() {
        let (signals, clean) =
            extract_agent_signals(r#"Done [[AGENT:NEXT]] [[AGENT:feedback:"msg"]]"#);
        assert_eq!(clean, "Done");
        assert_eq!(signals.len(), 2);
    }

    #[test]
    fn test_extract_no_signals() {
        let (signals, clean) = extract_agent_signals("Just text");
        assert_eq!(clean, "Just text");
        assert!(signals.is_empty());
    }

    #[tokio::test]
    async fn test_render_simple_template() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "Hello {{name}}, you are {{role}}!").unwrap();

        let context = serde_json::json!({
            "name": "Alice",
            "role": "developer"
        });

        let result = render_simple(tmp.path().to_str().unwrap(), &context)
            .await
            .unwrap();
        assert_eq!(result, "Hello Alice, you are developer!");
    }

    #[tokio::test]
    async fn test_render_simple_with_numbers() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "Turn: {{turn}}, Mode: {{mode}}").unwrap();

        let context = serde_json::json!({
            "turn": 5,
            "mode": "agent"
        });

        let result = render_simple(tmp.path().to_str().unwrap(), &context)
            .await
            .unwrap();
        assert_eq!(result, "Turn: 5, Mode: agent");
    }

    #[test]
    fn test_validate_content_valid_template() {
        let content = "Hello {{name}}, you are {{role}}!";
        let result = validate_content_for_llm(content);
        assert_eq!(result, content); // Valid templates kept as-is
    }

    #[test]
    fn test_validate_content_invalid_template() {
        // Content with spaces is not a valid identifier, but find("}}") still finds the closing
        // The function treats it as valid since it found matching }}
        let content = "Hello {{invalid}}";
        let result = validate_content_for_llm(content);
        // "invalid" is all alphanumeric, so it's kept as valid
        assert_eq!(result, "Hello {{invalid}}");
    }

    #[test]
    fn test_validate_content_unmatched_opening() {
        // Unmatched {{ without closing }}
        let content = "Hello {{unclosed";
        let result = validate_content_for_llm(content);
        assert_eq!(result, "Hello {{}}unclosed"); // Escaped
    }

    #[test]
    fn test_validate_content_valid_brackets() {
        let content = r#"Set [[key: "value"]]"#;
        let result = validate_content_for_llm(content);
        assert!(result.contains("[[[")); // Escaped
    }

    #[test]
    fn test_validate_content_read_syntax() {
        let content = "Read [[variable]] here";
        let result = validate_content_for_llm(content);
        assert!(result.contains("[[[")); // Escaped
    }

    #[test]
    fn test_validate_content_mixed() {
        let content = "Hello {{name}}, set [[key: val]], read [[var]]";
        let result = validate_content_for_llm(content);
        // Valid {{name}} kept, [[...]] escaped
        assert!(result.contains("{{name}}"));
        assert!(result.contains("[[["));
    }

    #[test]
    fn test_validate_content_no_special() {
        let content = "Just plain text with no special syntax.";
        let result = validate_content_for_llm(content);
        assert_eq!(result, content);
    }
}
