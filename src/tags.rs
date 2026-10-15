use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagAction {
    pub tag: String,
    pub value: Option<String>,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagResult {
    pub actions: Vec<TagAction>,
    pub cleaned_response: String,
    pub context_updates: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default)]
pub struct TagExecution {
    pub feedback_messages: Vec<String>,
    pub should_complete: bool,
    pub should_advance: bool,
    pub pushed_templates: Vec<String>,
    pub popped_template: Option<String>,
    pub learned_facts: Vec<String>,
    pub learned_preferences: HashMap<String, serde_json::Value>,
    pub learned_topics: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TagParser {
    pub enabled: bool,
}

impl TagParser {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn parse(&self, content: &str) -> (String, Vec<TagAction>) {
        if !self.enabled {
            return (content.to_string(), Vec::new());
        }
        let result = parse_tags(content);
        (result.cleaned_response, result.actions)
    }
}

impl Default for TagParser {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Parse tags from LLM response.
/// Tags are §-prefixed commands: §tag or §tag="value" or §tag="key":"value"
pub const DEFAULT_TAG_PREFIX: &str = "§";

pub fn validate_tag_prefix(prefix: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!prefix.is_empty() && prefix.chars().count() <= 16
        && !prefix.chars().any(|ch| ch.is_whitespace() || ch.is_control()),
        "tag_prefix must contain 1–16 non-whitespace, non-control characters");
    Ok(())
}

pub fn parse_tags(response: &str) -> TagResult {
    parse_tags_with_prefix(response, DEFAULT_TAG_PREFIX).expect("constant tag prefix is valid")
}

/// Literal prefix, not a regex. Slash syntax is canonical; prefix+command
/// remains accepted for backwards compatibility with existing templates.
pub fn parse_tags_with_prefix(response: &str, prefix: &str) -> anyhow::Result<TagResult> {
    validate_tag_prefix(prefix)?;
    let mut actions = Vec::new();
    let mut context_updates = HashMap::new();
    let mut cleaned = response.to_string();

    let tag_regex = regex::Regex::new(&format!(r#"{}(?:/)?(\w+)(?:="([^"]*)")?(?::"([^"]*)")?"#,regex::escape(prefix)))?;

    let mut found_tags = Vec::new();
    for cap in tag_regex.captures_iter(response) {
        let full_match = cap.get(0).unwrap();
        let tag_name = cap.get(1).unwrap().as_str().to_string();
        let value1 = cap.get(2).map(|m| m.as_str().to_string());
        let value2 = cap.get(3).map(|m| m.as_str().to_string());

        let value = match (value1, value2) {
            (Some(v1), Some(v2)) => Some(format!("{}:{}", v1, v2)),
            (Some(v1), None) => Some(v1),
            (None, Some(v2)) => Some(v2),
            (None, None) => None,
        };

        found_tags.push((
            full_match.as_str().to_string(),
            tag_name.clone(),
            value.clone(),
        ));

        actions.push(TagAction {
            tag: tag_name.clone(),
            value: value.clone(),
            raw: full_match.as_str().to_string(),
        });
    }

    for (_, tag_name, value) in &found_tags {
        match tag_name.as_str() {
            "done" => {
                context_updates.insert("tag_used".to_string(), serde_json::json!("done"));
                context_updates.insert("tags_done".to_string(), serde_json::json!(true));
            }
            "next" => {
                context_updates.insert("tag_used".to_string(), serde_json::json!("next"));
            }
            "feedback" => {
                if let Some(msg) = value {
                    context_updates.insert("tag_used".to_string(), serde_json::json!("feedback"));
                    context_updates.insert("last_feedback".to_string(), serde_json::json!(msg));
                }
            }
            "push" => {
                if let Some(template) = value {
                    context_updates.insert("tag_used".to_string(), serde_json::json!("push"));
                    context_updates.insert("last_push".to_string(), serde_json::json!(template));
                }
            }
            "pop" => {
                context_updates.insert("tag_used".to_string(), serde_json::json!("pop"));
            }
            "path" => {
                if let Some(dir) = value {
                    context_updates.insert("tag_used".to_string(), serde_json::json!("path"));
                    context_updates.insert("tag_path".to_string(), serde_json::json!(dir));
                }
            }
            "mode" => {
                if let Some(m) = value {
                    context_updates.insert("tag_used".to_string(), serde_json::json!("mode"));
                    context_updates.insert("mode".to_string(), serde_json::json!(m));
                }
            }
            "set" => {
                if let Some(kv) = value {
                    if let Some((k, v)) = kv.split_once(':') {
                        context_updates.insert("tag_used".to_string(), serde_json::json!("set"));
                        let json_val = serde_json::from_str::<serde_json::Value>(v)
                            .unwrap_or_else(|_| serde_json::json!(v));
                        context_updates.insert(k.trim().to_string(), json_val);
                    }
                }
            }
            _ => {
                context_updates.insert("tag_used".to_string(), serde_json::json!("unknown"));
                context_updates.insert("last_unknown_tag".to_string(), serde_json::json!(tag_name));
            }
        }
    }

    for (raw_tag, _, _) in &found_tags {
        cleaned = cleaned.replace(raw_tag, "");
    }
    cleaned = cleaned.trim_end().to_string();

    // Clean extra newlines even when no tags found
    let re = regex::Regex::new(r"\n{3,}").unwrap();
    cleaned = re.replace_all(&cleaned, "\n\n").to_string();
    cleaned = cleaned.trim_end().to_string();

    if !actions.is_empty() {
        let tag_names: Vec<String> = actions.iter().map(|a| a.tag.clone()).collect();
        context_updates.insert("last_tag_names".to_string(), serde_json::json!(tag_names));
    }

    Ok(TagResult {
        actions,
        cleaned_response: cleaned,
        context_updates,
    })
}

/// Execute tag actions and return execution result.
pub fn execute_tags(result: &TagResult, ctx: &mut crate::db::contexts::Context) -> TagExecution {
    let mut execution = TagExecution::default();

    // Ensure custom_data is an object
    if ctx.custom_data.is_null() {
        ctx.custom_data = serde_json::json!({});
    }

    for action in &result.actions {
        match action.tag.as_str() {
            "done" => {
                execution.should_complete = true;
                tracing::info!("Tag §done: marking workflow complete");
            }
            "next" => {
                execution.should_advance = true;
                tracing::info!("Tag §next: advancing to next state");
            }
            "feedback" => {
                if let Some(ref msg) = action.value {
                    execution.feedback_messages.push(msg.clone());
                    tracing::info!(msg = %msg, "Tag §feedback: sending progress update");
                }
            }
            "push" => {
                if let Some(ref template) = action.value {
                    ctx.settings.active_templates.push(template.clone());
                    execution.pushed_templates.push(template.clone());
                    tracing::info!(template = %template, "Tag §push: added template to queue");
                }
            }
            "pop" => {
                let popped = ctx.settings.active_templates.pop();
                execution.popped_template = popped.clone();
                tracing::info!(template = ?popped, "Tag §pop: removed template from queue");
            }
            "path" => {
                if let Some(ref dir) = action.value {
                    ctx.settings.path = dir.clone();
                    tracing::info!(path = %dir, "Tag §path: set working directory");
                }
            }
            "mode" => {
                if let Some(ref m) = action.value {
                    if let serde_json::Value::Object(ref mut map) = ctx.custom_data {
                        map.insert("mode".to_string(), serde_json::json!(m));
                    }
                    tracing::info!(mode = %m, "Tag §mode: changed context mode");
                }
            }
            "set" => {
                if let Some(ref kv) = action.value {
                    if let Some((k, v)) = kv.split_once(':') {
                        let json_val = serde_json::from_str::<serde_json::Value>(v)
                            .unwrap_or_else(|_| serde_json::json!(v));
                        if let serde_json::Value::Object(ref mut map) = ctx.custom_data {
                            map.insert(k.trim().to_string(), json_val);
                        }
                        tracing::info!(key = %k.trim(), "Tag §set: updated context variable");
                    }
                }
            }
            "learn" => {
                if let Some(ref learning) = action.value {
                    if let Some((learn_type, content)) = learning.split_once(':') {
                        match learn_type.trim() {
                            "fact" => {
                                execution.learned_facts.push(content.trim().to_string());
                                tracing::info!(fact = %content.trim(), "Tag §learn: learned new fact");
                            }
                            "pref" => {
                                if let Some((key, val)) = content.split_once('=') {
                                    execution.learned_preferences.insert(
                                        key.trim().to_string(),
                                        serde_json::json!(val.trim()),
                                    );
                                    tracing::info!(key = %key.trim(), val = %val.trim(), "Tag §learn: learned preference");
                                }
                            }
                            "topic" => {
                                execution.learned_topics.push(content.trim().to_string());
                                tracing::info!(topic = %content.trim(), "Tag §learn: learned topic");
                            }
                            _ => {
                                execution.learned_facts.push(learning.trim().to_string());
                            }
                        }
                    } else {
                        execution.learned_facts.push(learning.trim().to_string());
                    }
                }
            }
            _ => {
                tracing::warn!(tag = %action.tag, "Unknown tag, skipping execution");
            }
        }
    }

    execution
}

pub fn get_tag_instructions_with_prefix(prefix: &str) -> anyhow::Result<String> {
    validate_tag_prefix(prefix)?;
    Ok(get_tag_instructions().replace(DEFAULT_TAG_PREFIX, &format!("{prefix}/")))
}

pub fn get_tag_instructions() -> &'static str {
    r#"
## Response Tags
You can use special tags in your response to control the workflow. Put them at the END of your response, one per line.

Available tags:
- `§done` — Task is complete, stop the loop
- `§next` — Advance to the next step in the workflow
- `§feedback="message"` — Send a progress update to the user (use for status updates, not structured feedback)
- `§push="template_name"` — Add a sub-task template to the queue
- `§pop` — Remove the last sub-task from the queue
- `§path="directory"` — Change the working directory
- `§mode="mode_name"` — Change the context mode
- `§set="key":"value"` — Set any context variable
- `§learn="fact: <fact>"` — Learn and store a fact
- `§learn="pref: <key>=<value>"` — Learn a user preference
- `§learn="topic: <topic>"` — Track a conversation topic

IMPORTANT: Tags are stripped before the user sees your response. Put them at the very END.
"#
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn configurable_tag_prefix_is_literal_and_legacy_default_still_works() {
        for prefix in ["§", "!praxis", "[px].*", "⚙️"] {
            let result=parse_tags_with_prefix(&format!("answer {prefix}/done"),prefix).unwrap();
            assert_eq!(result.cleaned_response,"answer");
            assert_eq!(result.actions[0].tag,"done");
            let other=parse_tags_with_prefix("answer §done",prefix).unwrap();
            assert_eq!(other.actions.len(),usize::from(prefix=="§"));
            assert!(get_tag_instructions_with_prefix(prefix).unwrap().contains(&format!("{prefix}/next")));
        }
        assert!(parse_tags_with_prefix("not a tag XXXdone",".*").unwrap().actions.is_empty());
        for prefix in ["","has space","line\nbreak","\u{0}"] {
            assert!(parse_tags_with_prefix("ordinary words",prefix).is_err());
        }
        assert_eq!(parse_tags("answer §done").actions.len(),1);
    }

    #[test]
    fn configurable_tag_prefix_persists_and_is_not_model_writable() {
        let dir=tempfile::tempdir().unwrap();
        let db=crate::db::Database::new(dir.path()).unwrap();
        let ctx=db.load_context("prefix-test").unwrap();
        assert_eq!(ctx.settings.tag_prefix,"§");
        db.save_context(&ctx).unwrap();
        db.merge_context("prefix-test",serde_json::json!({"settings.tag_prefix":"!praxis"})).unwrap();
        assert_eq!(db.load_context("prefix-test").unwrap().settings.tag_prefix,"!praxis");
        assert!(db.merge_context("prefix-test",serde_json::json!({"settings.tag_prefix":""})).is_err());
        assert!(db.merge_context_from_agent("prefix-test",serde_json::json!({"settings.tag_prefix":"!evil"})).is_err());
        assert_eq!(db.load_context("prefix-test").unwrap().settings.tag_prefix,"!praxis");
    }

    #[test]
    fn test_parse_done() {
        let result = parse_tags("Hello §done");
        assert_eq!(result.cleaned_response, "Hello");
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].tag, "done");
    }

    #[test]
    fn test_parse_next() {
        let result = parse_tags("Next step §next");
        assert_eq!(result.cleaned_response, "Next step");
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].tag, "next");
    }

    #[test]
    fn test_parse_feedback() {
        let result = parse_tags(r#"Hello §feedback="good job""#);
        assert_eq!(result.cleaned_response, "Hello");
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].tag, "feedback");
        assert_eq!(result.actions[0].value, Some("good job".to_string()));
    }

    #[test]
    fn test_parse_push() {
        let result = parse_tags(r#"§push="tasks/code""#);
        assert_eq!(result.cleaned_response, "");
        assert_eq!(result.actions[0].tag, "push");
        assert_eq!(result.actions[0].value, Some("tasks/code".to_string()));
    }

    #[test]
    fn test_parse_set() {
        let result = parse_tags(r#"§set="mode":"agent""#);
        assert_eq!(result.cleaned_response, "");
        assert_eq!(result.actions[0].tag, "set");
        assert_eq!(result.actions[0].value, Some("mode:agent".to_string()));
    }

    #[test]
    fn test_parse_mode() {
        let result = parse_tags(r#"§mode="agent""#);
        assert_eq!(result.actions[0].tag, "mode");
        assert_eq!(result.actions[0].value, Some("agent".to_string()));
    }

    #[test]
    fn test_parse_path() {
        let result = parse_tags(r#"§path="/home/user/project""#);
        assert_eq!(result.actions[0].tag, "path");
        assert_eq!(
            result.actions[0].value,
            Some("/home/user/project".to_string())
        );
    }

    #[test]
    fn test_parse_multiple_tags() {
        let result = parse_tags(r#"Done now §done §next §feedback="almost there""#);
        assert_eq!(result.cleaned_response, "Done now");
        assert_eq!(result.actions.len(), 3);
    }

    #[test]
    fn test_parse_learn_fact() {
        let result = parse_tags(r#"§learn="fact: Rust is great""#);
        assert_eq!(result.actions[0].tag, "learn");
        assert_eq!(
            result.actions[0].value,
            Some("fact: Rust is great".to_string())
        );
    }

    #[test]
    fn test_no_tags() {
        let result = parse_tags("Just plain text");
        assert_eq!(result.cleaned_response, "Just plain text");
        assert!(result.actions.is_empty());
    }

    #[test]
    fn test_context_updates_done() {
        let result = parse_tags("§done");
        assert_eq!(result.context_updates.get("tag_used").unwrap(), "done");
        assert_eq!(result.context_updates.get("tags_done").unwrap(), &true);
    }

    #[test]
    fn test_context_updates_mode() {
        let result = parse_tags(r#"§mode="code""#);
        assert_eq!(result.context_updates.get("mode").unwrap(), "code");
    }

    #[test]
    fn test_clean_extra_newlines() {
        let result = parse_tags("Hello\n\n\n\nWorld");
        assert_eq!(result.cleaned_response, "Hello\n\nWorld");
    }

    #[test]
    fn test_tag_parser_enabled() {
        let parser = TagParser::new(true);
        let (clean, tags) = parser.parse("Hello §done");
        assert_eq!(clean, "Hello");
        assert_eq!(tags.len(), 1);
    }

    #[test]
    fn test_tag_parser_disabled() {
        let parser = TagParser::new(false);
        let (clean, tags) = parser.parse("Hello §done");
        assert_eq!(clean, "Hello §done");
        assert!(tags.is_empty());
    }

    #[test]
    fn test_execute_done() {
        let result = parse_tags("§done");
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert!(exec.should_complete);
    }

    #[test]
    fn test_execute_next() {
        let result = parse_tags("§next");
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert!(exec.should_advance);
    }

    #[test]
    fn test_execute_feedback() {
        let result = parse_tags(r#"§feedback="almost done""#);
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.feedback_messages, vec!["almost done"]);
    }

    #[test]
    fn test_execute_push() {
        let result = parse_tags(r#"§push="tasks/code""#);
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.pushed_templates, vec!["tasks/code"]);
        assert_eq!(ctx.settings.active_templates, vec!["tasks/code"]);
    }

    #[test]
    fn test_execute_pop() {
        let mut ctx = crate::db::contexts::Context::default();
        ctx.settings.active_templates.push("tasks/a".to_string());
        ctx.settings.active_templates.push("tasks/b".to_string());

        let result = parse_tags("§pop");
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.popped_template, Some("tasks/b".to_string()));
        assert_eq!(ctx.settings.active_templates, vec!["tasks/a"]);
    }

    #[test]
    fn test_execute_mode() {
        let result = parse_tags(r#"§mode="code""#);
        let mut ctx = crate::db::contexts::Context::default();
        execute_tags(&result, &mut ctx);
        assert_eq!(ctx.custom_data["mode"], "code");
    }

    #[test]
    fn test_execute_set() {
        let result = parse_tags(r#"§set="language":"rust""#);
        let mut ctx = crate::db::contexts::Context::default();
        execute_tags(&result, &mut ctx);
        assert_eq!(ctx.custom_data["language"], "rust");
    }

    #[test]
    fn test_execute_learn_fact() {
        let result = parse_tags(r#"§learn="fact: User prefers Rust""#);
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.learned_facts, vec!["User prefers Rust"]);
    }

    #[test]
    fn test_execute_learn_pref() {
        let result = parse_tags(r#"§learn="pref: theme=dark""#);
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.learned_preferences["theme"], "dark");
    }

    #[test]
    fn test_execute_learn_topic() {
        let result = parse_tags(r#"§learn="topic: Rust async programming""#);
        let mut ctx = crate::db::contexts::Context::default();
        let exec = execute_tags(&result, &mut ctx);
        assert_eq!(exec.learned_topics, vec!["Rust async programming"]);
    }

    #[test]
    fn test_get_tag_instructions() {
        let instructions = get_tag_instructions();
        assert!(instructions.contains("§done"));
        assert!(instructions.contains("§next"));
        assert!(instructions.contains("§feedback"));
    }
}
