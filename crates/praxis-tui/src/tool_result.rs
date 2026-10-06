//! Display-only decoding. Never rewrite Message.content or parse command arguments.
use serde_json::Value;

pub(super) fn format(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if !object.contains_key("stdout") && !object.contains_key("stderr") { return None; }
    // A malformed/mixed schema must remain visible verbatim, not lose fields.
    for key in ["stdout", "stderr"] {
        if object.get(key).is_some_and(|value| !value.is_string()) { return None; }
    }
    let mut sections = Vec::new();
    for key in ["stdout", "stderr"] {
        if let Some(text) = object.get(key).and_then(Value::as_str) {
            // Exactly one JSON decode: literal paths, regexes and JSON within
            // stdout are output data, not additional serialization layers.
            sections.push(format!("{key}:\n{text}"));
        }
    }
    if let Some(code) = object.get("exit_code") { sections.push(format!("exit_code: {code}")); }
    let mut metadata = object.clone();
    for key in ["stdout", "stderr", "exit_code"] { metadata.remove(key); }
    if !metadata.is_empty() {
        sections.push(format!("metadata:\n{}", serde_json::to_string_pretty(&metadata).ok()?));
    }
    Some(sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_results_shared_corpus_decodes_once_and_preserves_unknown() {
        let cases: Value = serde_json::from_str(include_str!("../../../scripts/fixtures/structured_tool_results.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let raw = case["raw"].as_str().unwrap();
            assert_eq!(format(raw).as_deref(), case["display"].as_str(), "{}", case["name"]);
        }
    }
    #[test]
    fn structured_results_large_unicode_has_no_presentation_truncation() {
        let stdout = "世界 👩‍💻\n  next\n".repeat(10000);
        let raw = serde_json::json!({"stdout":stdout,"stderr":"","exit_code":0}).to_string();
        let display = format(&raw).unwrap();
        assert!(display.contains(&stdout));
        assert!(display.ends_with("exit_code: 0"));
    }
}
