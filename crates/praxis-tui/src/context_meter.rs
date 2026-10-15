//! Context fill shown in the header: how much of the model's window the last
//! request used, so a filling context is visible before it overflows.

/// Fill level, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Full,
    /// The window is unknown; only the prompt size is shown.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextMeter {
    pub prompt_tokens: u64,
    pub window: Option<u64>,
}

impl ContextMeter {
    /// Parse the gateway's `context` event payload.
    pub fn parse(data: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(data).ok()?;
        Some(Self {
            prompt_tokens: value.get("prompt_tokens")?.as_u64()?,
            window: value.get("window").and_then(serde_json::Value::as_u64).filter(|w| *w > 0),
        })
    }

    pub fn percent(&self) -> Option<u64> {
        self.window.map(|window| (self.prompt_tokens.saturating_mul(100) / window).min(999))
    }

    pub fn level(&self) -> Level {
        match self.percent() {
            None => Level::Unknown,
            Some(p) if p >= 85 => Level::Full,
            Some(p) if p >= 60 => Level::Warn,
            Some(_) => Level::Ok,
        }
    }

    /// `ctx 6.2k/16k ███░░░░░ 39%`, or `ctx 6.2k` without a known window.
    pub fn label(&self) -> String {
        match (self.window, self.percent()) {
            (Some(window), Some(percent)) => {
                const CELLS: u64 = 8;
                let filled = (percent.min(100) * CELLS).div_ceil(100).min(CELLS) as usize;
                format!(
                    " ctx {}/{} {}{} {percent}% ",
                    short(self.prompt_tokens),
                    short(window),
                    "█".repeat(filled),
                    "░".repeat(CELLS as usize - filled)
                )
            }
            _ => format!(" ctx {} ", short(self.prompt_tokens)),
        }
    }
}

/// 950 → "950", 6200 → "6.2k", 16384 → "16k", 131072 → "131k".
fn short(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=9_999 => format!("{:.1}k", tokens as f64 / 1000.0),
        _ => format!("{}k", tokens / 1000),
    }
}

/// One-line notice for a gateway `compaction` event.
pub fn compaction_notice(data: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    let before = value.get("tokens_before_estimate").and_then(serde_json::Value::as_u64);
    let size = before.map(|t| format!(" (~{} tokens)", short(t))).unwrap_or_default();
    match value.get("status").and_then(serde_json::Value::as_str) {
        Some("running") => format!("Compacting older turns{size}…"),
        Some("completed") => format!("Context compacted{size}: summary saved, current turn kept"),
        Some(other) => format!("Compaction {other}{size}"),
        None => "Compaction update".into(),
    }
}

/// One-line notice for a gateway `loop_guard` event.
pub fn loop_guard_notice(data: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    let reason = value.get("reason").and_then(serde_json::Value::as_str).unwrap_or("repeated itself");
    match value.get("on_abort").and_then(serde_json::Value::as_str) {
        Some(state) => format!("Stopped: {reason} → back to {state}"),
        None => format!("Stopped: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_shows_fill_against_the_window() {
        let meter = ContextMeter::parse(r#"{"prompt_tokens":6200,"window":16384}"#).unwrap();
        assert_eq!(meter.percent(), Some(37));
        assert_eq!(meter.level(), Level::Ok);
        assert_eq!(meter.label(), " ctx 6.2k/16k ███░░░░░ 37% ");
        let full = ContextMeter::parse(r#"{"prompt_tokens":15000,"window":16384}"#).unwrap();
        assert_eq!(full.level(), Level::Full);
        assert!(full.label().contains("████████"));
    }

    #[test]
    fn unknown_window_shows_only_the_prompt_size() {
        for data in [r#"{"prompt_tokens":950,"window":null}"#, r#"{"prompt_tokens":950,"window":0}"#] {
            let meter = ContextMeter::parse(data).unwrap();
            assert_eq!(meter.level(), Level::Unknown);
            assert_eq!(meter.label(), " ctx 950 ");
        }
        assert!(ContextMeter::parse("not json").is_none());
        assert!(ContextMeter::parse(r#"{"window":100}"#).is_none());
    }

    #[test]
    fn notices_read_gateway_events() {
        assert_eq!(compaction_notice(r#"{"status":"running","tokens_before_estimate":14000}"#), "Compacting older turns (~14k tokens)…");
        assert!(compaction_notice(r#"{"status":"completed"}"#).starts_with("Context compacted"));
        assert_eq!(loop_guard_notice(r#"{"reason":"tool calls kept failing","on_abort":"standard"}"#), "Stopped: tool calls kept failing → back to standard");
        assert_eq!(loop_guard_notice("{}"), "Stopped: repeated itself");
    }
}
