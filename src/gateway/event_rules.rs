//! `[on]` rules: a workflow reacts to events instead of re-reading state.
//!
//! ```ini
//! [on]
//! tool_failed execute_terminal x2 -> state debugger
//! tool_failed * x5               -> stop "Too many failing tool calls"
//! tool_ok write_file             -> set sm_data.dirty = true
//! changed sm_data.role by model  -> note "Role changed: re-read the plan first"
//! ```
//!
//! Triggers
//! - `tool_failed <tool|*> [xN]`: the Nth failure in a row of a matching tool
//!   (default 1); a success of a matching tool resets the count.
//! - `tool_ok <tool|*>`: a matching tool call succeeded.
//! - `changed <path|*> [by <source>]`: a logged context change touched the path
//!   (or anything below it). `by` matches the source prefix (`user`, `model`,
//!   `tool:write_file`, `compaction`, …). Without `by`, changes made by the
//!   runtime's bookkeeping and by rules themselves are ignored, so a rule cannot
//!   trigger itself.
//!
//! Actions
//! - `state <name>`: switch state, through the workflow's guards.
//! - `set <path> = <value>`: set a context value (JSON, else a string).
//! - `note <text>`: tell the model, as a system note before its next turn.
//! - `stop <text>`: stop the task, like the loop guard.
//!
//! Rules are operator-authored workflow configuration, like `[overrides]`. A
//! task fires at most `MAX_FIRINGS` rule actions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const MAX_FIRINGS: u32 = 32;
/// Source of context writes made by rule actions.
pub const RULE_SOURCE: &str = "workflow:on";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Trigger {
    ToolFailed { tool: String, times: u32 },
    ToolOk { tool: String },
    Changed { path: String, by: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    State { name: String },
    Set { path: String, value: Value },
    Note { text: String },
    Stop { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnRule {
    pub trigger: Trigger,
    pub action: Action,
    /// Source line, for messages.
    pub line: usize,
}

/// What happened, as the rules see it.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Tool { name: String, success: bool },
    Changed { path: String, source: String },
}

/// Parse one `[on]` line: `<trigger> -> <action>`.
pub fn parse(line: &str, line_num: usize) -> Result<OnRule, String> {
    let (trigger, action) = line
        .split_once("->")
        .ok_or_else(|| format!("Line {line_num}: [on] rules look like `<trigger> -> <action>`"))?;
    Ok(OnRule { trigger: parse_trigger(trigger.trim(), line_num)?, action: parse_action(action.trim(), line_num)?, line: line_num })
}

fn parse_trigger(text: &str, line: usize) -> Result<Trigger, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let name = |word: Option<&&str>| -> Result<String, String> {
        let word = word.ok_or_else(|| format!("Line {line}: trigger needs a tool name or *"))?;
        if *word == "*" || word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'/') {
            Ok(word.to_string())
        } else {
            Err(format!("Line {line}: invalid name `{word}`"))
        }
    };
    match words.first().copied() {
        Some("tool_failed") => {
            let tool = name(words.get(1))?;
            let times = match words.get(2) {
                None => 1,
                Some(count) => count
                    .strip_prefix('x')
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(|| format!("Line {line}: repeat count looks like x2 (1..100)"))?,
            };
            if words.len() > 3 {
                return Err(format!("Line {line}: unexpected `{}`", words[3]));
            }
            Ok(Trigger::ToolFailed { tool, times })
        }
        Some("tool_ok") if words.len() == 2 => Ok(Trigger::ToolOk { tool: name(words.get(1))? }),
        Some("changed") => {
            let path = words.get(1).ok_or_else(|| format!("Line {line}: changed needs a path or *"))?;
            let path = if *path == "*" { "*".to_string() } else { crate::db::contexts::canonical_context_key(path).into_owned() };
            let by = match (words.get(2), words.get(3), words.len()) {
                (None, _, 2) => None,
                (Some(&"by"), Some(source), 4) => Some(source.to_string()),
                _ => return Err(format!("Line {line}: changed <path> [by <source>]")),
            };
            Ok(Trigger::Changed { path, by })
        }
        _ => Err(format!("Line {line}: unknown trigger `{text}` (tool_failed, tool_ok, changed)")),
    }
}

fn parse_action(text: &str, line: usize) -> Result<Action, String> {
    let (verb, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let quoted = |text: &str| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(text).to_string();
    match verb {
        "state" if !rest.is_empty() && !rest.contains(char::is_whitespace) => Ok(Action::State { name: rest.to_string() }),
        "set" => {
            let (path, value) = rest.split_once('=').ok_or_else(|| format!("Line {line}: set <path> = <value>"))?;
            let path = crate::db::contexts::canonical_context_key(path.trim()).into_owned();
            let root = path.split('.').next().unwrap_or("");
            if path.is_empty() || matches!(root, "user_id" | "session_id") {
                return Err(format!("Line {line}: rules cannot set `{path}`"));
            }
            let raw = value.trim();
            let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(quoted(raw)));
            Ok(Action::Set { path, value })
        }
        "note" if !rest.is_empty() => Ok(Action::Note { text: quoted(rest) }),
        "stop" if !rest.is_empty() => Ok(Action::Stop { text: quoted(rest) }),
        _ => Err(format!("Line {line}: unknown action `{text}` (state, set, note, stop)")),
    }
}

/// Per-task matching state.
#[derive(Debug, Default)]
pub struct RuleState {
    failures: HashMap<usize, u32>,
    fired: u32,
}

impl RuleState {
    /// Actions triggered by `event`, in rule order. Stops firing after
    /// `MAX_FIRINGS` actions per task.
    pub fn handle(&mut self, rules: &[OnRule], event: &Event) -> Vec<Action> {
        let mut actions = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            let fires = match (&rule.trigger, event) {
                (Trigger::ToolFailed { tool, times }, Event::Tool { name, success }) if matches_name(tool, name) => {
                    let count = self.failures.entry(index).or_default();
                    if *success {
                        *count = 0;
                        false
                    } else {
                        *count += 1;
                        let reached = *count >= *times;
                        if reached {
                            *count = 0;
                        }
                        reached
                    }
                }
                (Trigger::ToolOk { tool }, Event::Tool { name, success: true }) => matches_name(tool, name),
                (Trigger::Changed { path, by }, Event::Changed { path: changed, source }) => {
                    let source_ok = match by {
                        Some(prefix) => source.starts_with(prefix.as_str()),
                        None => source != "runtime" && !source.starts_with("workflow"),
                    };
                    source_ok && (path == "*" || changed == path || changed.starts_with(&format!("{path}.")))
                }
                _ => false,
            };
            if fires && self.fired < MAX_FIRINGS {
                self.fired += 1;
                actions.push(rule.action.clone());
            }
        }
        actions
    }
}

fn matches_name(pattern: &str, name: &str) -> bool {
    pattern == "*" || pattern == name
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(text: &str) -> Vec<OnRule> {
        text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()).map(|(i, l)| parse(l, i + 1).unwrap()).collect()
    }
    fn tool(name: &str, success: bool) -> Event {
        Event::Tool { name: name.into(), success }
    }

    #[test]
    fn parses_all_trigger_and_action_forms() {
        let parsed = rules(
            "tool_failed execute_terminal x2 -> state debugger\n\
             tool_ok * -> set sm_data.dirty = true\n\
             changed sm_data.role by model -> note \"Re-read the plan\"\n\
             changed cl_data.x -> set sm_data.label = hello world\n\
             tool_failed * -> stop \"Too many failures\"",
        );
        assert_eq!(parsed[0].trigger, Trigger::ToolFailed { tool: "execute_terminal".into(), times: 2 });
        assert_eq!(parsed[0].action, Action::State { name: "debugger".into() });
        assert_eq!(parsed[1].action, Action::Set { path: "sm_data.dirty".into(), value: json!(true) });
        assert_eq!(parsed[2].trigger, Trigger::Changed { path: "sm_data.role".into(), by: Some("model".into()) });
        assert_eq!(parsed[2].action, Action::Note { text: "Re-read the plan".into() });
        assert_eq!(parsed[3].trigger, Trigger::Changed { path: "sm_data.x".into(), by: None }, "legacy cl_data is canonical");
        assert_eq!(parsed[3].action, Action::Set { path: "sm_data.label".into(), value: json!("hello world") });
        assert_eq!(parsed[4].trigger, Trigger::ToolFailed { tool: "*".into(), times: 1 });
    }

    #[test]
    fn rejects_malformed_rules_with_the_line_number() {
        for bad in [
            "tool_failed x -> state a b",
            "tool_failed t x0 -> stop x",
            "tool_ok -> stop x",
            "changed a.b from model -> stop x",
            "whenever -> stop x",
            "tool_ok t -> set user_id = bob",
            "tool_ok t -> launch rockets",
            "tool_ok t stop x",
        ] {
            let error = parse(bad, 7).unwrap_err();
            assert!(error.starts_with("Line 7:"), "{bad}: {error}");
        }
    }

    #[test]
    fn consecutive_failures_fire_once_per_streak_and_success_resets() {
        let rules = rules("tool_failed execute_terminal x2 -> state debugger");
        let mut state = RuleState::default();
        assert!(state.handle(&rules, &tool("execute_terminal", false)).is_empty());
        assert!(state.handle(&rules, &tool("execute_terminal", true)).is_empty(), "success resets");
        assert!(state.handle(&rules, &tool("execute_terminal", false)).is_empty());
        assert!(state.handle(&rules, &tool("read_file", false)).is_empty(), "other tools do not count");
        assert_eq!(state.handle(&rules, &tool("execute_terminal", false)), vec![Action::State { name: "debugger".into() }]);
        assert!(state.handle(&rules, &tool("execute_terminal", false)).is_empty(), "streak restarts after firing");
    }

    #[test]
    fn change_rules_match_paths_and_sources_but_never_their_own_writes() {
        let rules = rules("changed sm_data -> note any\nchanged settings.model by user -> note user");
        let mut state = RuleState::default();
        let changed = |path: &str, source: &str| Event::Changed { path: path.into(), source: source.into() };
        assert_eq!(state.handle(&rules, &changed("sm_data.role", "model:set_context")).len(), 1);
        assert!(state.handle(&rules, &changed("sm_data_other", "user")).is_empty(), "prefix means path segments");
        assert!(state.handle(&rules, &changed("sm_data.role", RULE_SOURCE)).is_empty(), "no self-triggering");
        assert!(state.handle(&rules, &changed("sm_data.x", "runtime")).is_empty());
        assert_eq!(state.handle(&rules, &changed("settings.model", "user")), vec![Action::Note { text: "user".into() }]);
        assert!(state.handle(&rules, &changed("settings.model", "model:set_context")).is_empty());
    }

    #[test]
    fn a_task_fires_a_bounded_number_of_actions() {
        let rules = rules("tool_ok * -> note again");
        let mut state = RuleState::default();
        let fired: usize = (0..100).map(|_| state.handle(&rules, &tool("t", true)).len()).sum();
        assert_eq!(fired, MAX_FIRINGS as usize);
    }
}
