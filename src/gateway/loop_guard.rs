//! Abort conditions for a model that has stopped making progress.
//!
//! The turn and tool budgets bound *how much* a task may do; they do not notice
//! that a model is doing the *same* thing again. Small local models in
//! particular fall into loops: the identical tool call, a tool that keeps
//! failing, the same reply, role ping-pong or a generation that repeats one
//! phrase until max_tokens. The guard watches those signals, warns the model
//! once before the limit and then stops the task with a reason the user sees.
//!
//! Limits come from a workflow's `[limits]` section (see `docs/WORKFLOW_GRAPHS.md`);
//! a value of 0 disables that check.

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoopLimits {
    /// The same tool with the same arguments, back to back.
    pub max_same_tool_call: u32,
    /// Failed tool results in a row (any tool).
    pub max_tool_errors: u32,
    /// The same assistant text in consecutive turns.
    pub max_same_reply: u32,
    /// State/role switches within one task.
    pub max_state_changes: u32,
    /// One chunk repeated at the end of a single response ("the the the …").
    pub max_repeated_chunks: u32,
    /// State to enter after an abort, so the next message starts somewhere sane.
    pub on_abort: Option<String>,
}

impl Default for LoopLimits {
    fn default() -> Self {
        Self {
            max_same_tool_call: 3,
            max_tool_errors: 4,
            max_same_reply: 3,
            max_state_changes: 12,
            max_repeated_chunks: 12,
            on_abort: None,
        }
    }
}

impl LoopLimits {
    /// Apply one `key = value` line from a `[limits]` section.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let value = value.trim().trim_matches('"');
        if key == "on_abort" {
            if value.is_empty() {
                return Err("on_abort needs a state name".into());
            }
            self.on_abort = Some(value.to_string());
            return Ok(());
        }
        let number: u32 = value
            .parse()
            .map_err(|_| format!("Limit {key} must be a whole number (0 disables it)"))?;
        if number > 1000 {
            return Err("Limits above 1000 are not meaningful".into());
        }
        match key {
            "max_same_tool_call" => self.max_same_tool_call = number,
            "max_tool_errors" => self.max_tool_errors = number,
            "max_same_reply" => self.max_same_reply = number,
            "max_state_changes" => self.max_state_changes = number,
            "max_repeated_chunks" => self.max_repeated_chunks = number,
            _ => return Err(format!("Unknown limit: {key}")),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// One step before the limit: tell the model to change course.
    Warn(String),
    /// The limit is reached: stop the task.
    Abort(String),
}

#[derive(Debug, Default)]
pub struct LoopGuard {
    limits: LoopLimits,
    last_call: Option<u64>,
    same_call: u32,
    tool_errors: u32,
    last_reply: Option<u64>,
    same_reply: u32,
    last_state: Option<String>,
    state_changes: u32,
}

impl LoopGuard {
    pub fn new(limits: LoopLimits) -> Self {
        Self { limits, ..Default::default() }
    }

    pub fn limits(&self) -> &LoopLimits {
        &self.limits
    }

    pub fn tool_call(&mut self, name: &str, arguments: &str) -> Verdict {
        // Normalize JSON so key order and whitespace do not hide a repeat.
        let args = serde_json::from_str::<serde_json::Value>(arguments)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| arguments.trim().to_string());
        let key = hash(&(name, args));
        if self.last_call == Some(key) {
            self.same_call += 1;
        } else {
            self.last_call = Some(key);
            self.same_call = 1;
        }
        check(
            self.same_call,
            self.limits.max_same_tool_call,
            || format!("the tool call `{name}` was repeated with identical arguments"),
            "You are repeating the same tool call with the same arguments. Its result will not change. Use the result you already have, try a different approach, or give your final answer.",
        )
    }

    pub fn tool_result(&mut self, success: bool) -> Verdict {
        if success {
            self.tool_errors = 0;
            return Verdict::Ok;
        }
        self.tool_errors += 1;
        check(
            self.tool_errors,
            self.limits.max_tool_errors,
            || "tool calls kept failing".to_string(),
            "Several tool calls in a row have failed. Read the error messages, fix the cause or stop and explain to the user what is blocking you.",
        )
    }

    /// Every assistant response, with or without tool calls.
    pub fn reply(&mut self, text: &str) -> Verdict {
        if let Some((chunk, count)) = repeated_tail(text) {
            if self.limits.max_repeated_chunks > 0 && count >= self.limits.max_repeated_chunks {
                return Verdict::Abort(format!(
                    "the response degenerated into repeating \"{}\" {count} times",
                    crate::util::truncate_chars(chunk.trim(), 40)
                ));
            }
        }
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if normalized.is_empty() {
            return Verdict::Ok;
        }
        let key = hash(&normalized);
        if self.last_reply == Some(key) {
            self.same_reply += 1;
        } else {
            self.last_reply = Some(key);
            self.same_reply = 1;
        }
        check(
            self.same_reply,
            self.limits.max_same_reply,
            || "the model gave the same reply again".to_string(),
            "You wrote the same reply as in your previous turn. Do not repeat yourself: make progress or give your final answer.",
        )
    }

    pub fn state(&mut self, state: Option<&str>) -> Verdict {
        let state = state.unwrap_or("").to_string();
        match &self.last_state {
            None => {
                self.last_state = Some(state);
                Verdict::Ok
            }
            Some(previous) if *previous == state => Verdict::Ok,
            Some(_) => {
                self.last_state = Some(state);
                self.state_changes += 1;
                check(
                    self.state_changes,
                    self.limits.max_state_changes,
                    || "the workflow switched state/role too often".to_string(),
                    "You keep switching roles or states. Stay in the current role and finish the task.",
                )
            }
        }
    }
}

fn check(count: u32, limit: u32, reason: impl FnOnce() -> String, warning: &str) -> Verdict {
    if limit == 0 {
        Verdict::Ok
    } else if count >= limit {
        Verdict::Abort(format!("{} ({count}×, limit {limit})", reason()))
    } else if count + 1 == limit && count > 0 {
        Verdict::Warn(warning.to_string())
    } else {
        Verdict::Ok
    }
}

fn hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Find the chunk that repeats back to back at the end of `text`, with how
/// often it repeats. Only the last 4096 characters are inspected. A chunk is
/// 2..=256 characters, contains a letter or digit, and the repeats must cover
/// at least 256 characters, so "ha ha", rules like `-----` or a short list do
/// not count; runaway generations usually run on to max_tokens.
pub fn repeated_tail(text: &str) -> Option<(String, u32)> {
    let chars: Vec<char> = text.trim_end().chars().collect();
    let tail = &chars[chars.len().saturating_sub(4096)..];
    let mut best: Option<(usize, u32)> = None;
    for period in 2..=256.min(tail.len() / 2) {
        let chunk = &tail[tail.len() - period..];
        let mut count = 1u32;
        let mut end = tail.len() - period;
        while end >= period && &tail[end - period..end] == chunk {
            count += 1;
            end -= period;
        }
        if count >= 2
            && period * count as usize >= 256
            && chunk.iter().any(|c| c.is_alphanumeric())
            && best.is_none_or(|(_, c)| count > c) {
            best = Some((period, count));
        }
    }
    best.map(|(period, count)| (tail[tail.len() - period..].iter().collect(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_tool_calls_warn_then_abort() {
        let mut guard = LoopGuard::new(LoopLimits::default());
        assert_eq!(guard.tool_call("read_file", r#"{"path":"a"}"#), Verdict::Ok);
        assert!(matches!(guard.tool_call("read_file", r#"{ "path" : "a" }"#), Verdict::Warn(_)));
        assert!(matches!(guard.tool_call("read_file", r#"{"path":"a"}"#), Verdict::Abort(_)));
    }

    #[test]
    fn a_different_call_resets_the_repeat_count() {
        let mut guard = LoopGuard::new(LoopLimits::default());
        guard.tool_call("read_file", r#"{"path":"a"}"#);
        guard.tool_call("read_file", r#"{"path":"a"}"#);
        assert_eq!(guard.tool_call("read_file", r#"{"path":"b"}"#), Verdict::Ok);
        assert_eq!(guard.tool_call("read_file", r#"{"path":"a"}"#), Verdict::Ok);
    }

    #[test]
    fn consecutive_tool_errors_abort_and_a_success_resets() {
        let mut guard = LoopGuard::new(LoopLimits { max_tool_errors: 2, ..Default::default() });
        assert!(matches!(guard.tool_result(false), Verdict::Warn(_)));
        assert_eq!(guard.tool_result(true), Verdict::Ok);
        guard.tool_result(false);
        assert!(matches!(guard.tool_result(false), Verdict::Abort(_)));
    }

    #[test]
    fn the_same_reply_ignoring_whitespace_and_case_aborts() {
        let mut guard = LoopGuard::new(LoopLimits::default());
        assert_eq!(guard.reply("I will check the file."), Verdict::Ok);
        assert!(matches!(guard.reply("I will  check the FILE."), Verdict::Warn(_)));
        assert!(matches!(guard.reply("i will check the file."), Verdict::Abort(_)));
        assert_eq!(guard.reply(""), Verdict::Ok);
    }

    #[test]
    fn degenerate_generation_aborts_immediately() {
        let mut guard = LoopGuard::new(LoopLimits::default());
        let text = format!("Here is the answer. {}", "I am sorry. ".repeat(30));
        assert!(matches!(guard.reply(&text), Verdict::Abort(reason) if reason.contains("I am sorry.")));
    }

    #[test]
    fn ordinary_text_and_short_repeats_are_not_degenerate() {
        assert_eq!(repeated_tail("ha ha ha"), None);
        assert_eq!(repeated_tail("1. one\n2. two\n3. three\n4. four\n5. five"), None);
        let mut guard = LoopGuard::new(LoopLimits::default());
        assert_eq!(guard.reply(&"-".repeat(400)), Verdict::Ok);
        assert_eq!(repeated_tail(&"ab".repeat(20)), None);
    }

    #[test]
    fn state_ping_pong_aborts() {
        let mut guard = LoopGuard::new(LoopLimits { max_state_changes: 3, ..Default::default() });
        assert_eq!(guard.state(Some("code")), Verdict::Ok);
        assert_eq!(guard.state(Some("code")), Verdict::Ok);
        assert_eq!(guard.state(Some("review")), Verdict::Ok);
        assert!(matches!(guard.state(Some("code")), Verdict::Warn(_)));
        assert!(matches!(guard.state(Some("review")), Verdict::Abort(_)));
    }

    #[test]
    fn zero_disables_a_check() {
        let mut guard = LoopGuard::new(LoopLimits { max_same_tool_call: 0, ..Default::default() });
        for _ in 0..10 {
            assert_eq!(guard.tool_call("x", "{}"), Verdict::Ok);
        }
    }

    #[test]
    fn limits_parse_and_reject_unknown_keys() {
        let mut limits = LoopLimits::default();
        limits.set("max_tool_errors", "6").unwrap();
        limits.set("on_abort", "\"standard\"").unwrap();
        assert_eq!(limits.max_tool_errors, 6);
        assert_eq!(limits.on_abort.as_deref(), Some("standard"));
        assert!(limits.set("max_whatever", "1").is_err());
        assert!(limits.set("max_tool_errors", "-1").is_err());
    }
}
