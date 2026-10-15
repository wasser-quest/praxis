//! Rendering of the gateway's context change log (`/changes`) and live
//! `context_changed` notices.
use serde_json::Value;

/// `/changes` output: newest last, one block per event.
pub fn render(log: &Value) -> String {
    let events = log["events"].as_array().cloned().unwrap_or_default();
    if events.is_empty() {
        return "No context changes recorded yet.".into();
    }
    let mut out = format!("Context changes (now at seq {}):\n", log["seq"]);
    for event in &events {
        out.push_str(&format!(
            "\n#{} {} by {} {}\n",
            event["seq"],
            event["kind"].as_str().unwrap_or("changed"),
            event["source"].as_str().unwrap_or("?"),
            event["at"].as_str().unwrap_or("")
        ));
        for change in event["changes"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  {}: {} → {}\n",
                change["path"].as_str().unwrap_or("?"),
                short(&change["before"]),
                short(&change["after"])
            ));
        }
        let shown = event["changes"].as_array().map_or(0, Vec::len) as u64;
        if let Some(total) = event["changed_paths"].as_u64().filter(|t| *t > shown) {
            out.push_str(&format!("  … {} more paths\n", total - shown));
        }
    }
    out.push_str("\nChange a value: /context set <path>=<value> (logged as a new change by you).");
    out
}

fn short(value: &Value) -> String {
    let text = match value {
        Value::Null => "∅".to_string(),
        Value::Object(map) if map.contains_key("sha256") => format!("<{} bytes>", map["bytes"]),
        other => other.to_string(),
    };
    if text.chars().count() > 60 { format!("{}…", text.chars().take(59).collect::<String>()) } else { text }
}

/// Status-line notice for changes the user did not make themselves; `None`
/// for the runtime's own bookkeeping and the user's own edits.
pub fn notice(data: &str) -> Option<String> {
    let event: Value = serde_json::from_str(data).ok()?;
    let source = event["source"].as_str()?;
    if source == "runtime" || source == "user" {
        return None;
    }
    let paths: Vec<&str> = event["paths"].as_array()?.iter().filter_map(Value::as_str).take(3).collect();
    if paths.is_empty() {
        return None;
    }
    Some(format!("{source} changed {} (#{}; /changes)", paths.join(", "), event["seq"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_renders_paths_values_and_sources() {
        let log = json!({"seq": 2, "events": [
            {"seq": 1, "kind": "changed", "source": "user", "at": "2026-10-15T10:00:00Z", "changed_paths": 1,
             "changes": [{"path": "settings.max_llm_turns", "before": null, "after": 7}]},
            {"seq": 2, "kind": "changed", "source": "model:set_context", "at": "2026-10-15T10:01:00Z", "changed_paths": 70,
             "changes": [{"path": "custom_data.notes", "before": "a", "after": {"bytes": 900, "sha256": "ab"}}]}
        ]});
        let text = render(&log);
        assert!(text.contains("#1 changed by user"));
        assert!(text.contains("settings.max_llm_turns: ∅ → 7"));
        assert!(text.contains("custom_data.notes: \"a\" → <900 bytes>"));
        assert!(text.contains("… 69 more paths"));
        assert_eq!(render(&json!({"seq": 0, "events": []})), "No context changes recorded yet.");
    }

    #[test]
    fn notices_skip_bookkeeping_and_own_edits() {
        assert_eq!(notice(r#"{"seq":5,"source":"model:set_context","paths":["sm_data.role"]}"#).unwrap(), "model:set_context changed sm_data.role (#5; /changes)");
        assert!(notice(r#"{"seq":6,"source":"runtime","paths":["turn"]}"#).is_none());
        assert!(notice(r#"{"seq":7,"source":"user","paths":["settings.model"]}"#).is_none());
    }
}
