//! Common delivery policy for every gateway tool loop. Raw text is saved before
//! choosing what goes into the LLM context. Default is FULL text, not a preview;
//! legacy preview limits are ignored. Image content parts stay with the caller.
use crate::{db::Database, gateway::llm::provider::ToolCall, tools::tool_output};

/// Display status only; execution authority still comes from runtime receipts.
pub(crate) fn display_success(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("Error:") || text.starts_with("Error ") {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return true; };
    if let Some(outcome) = value.get("receipt").and_then(|r| r.get("outcome")).and_then(|v| v.as_str()) {
        return outcome == "committed" || (outcome == "passed" && value["receipt"]["verified"] == true);
    }
    if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    if value.get("error").is_some_and(|v| !v.is_null() && v.as_str() != Some("")) {
        return false;
    }
    value.get("exit_code").and_then(|v| v.as_i64()).is_none_or(|code| code == 0)
}

pub fn prepare(db: &Database, user: &str, tool: &str, call: &str, text: &str) -> anyhow::Result<String> {
    if tool == "read_tool_result" {
        // This is already a selected view of an existing snapshot, not a new source.
        return Ok(text.to_string());
    }
    let output = db.save_tool_output(user, tool, call, text)?;
    Ok(tool_output::complete_response(&output))
}

pub fn prepare_for_call(db: &Database, user: &str, call: &ToolCall, text: &str) -> anyhow::Result<String> {
    let args: serde_json::Value = serde_json::from_str(&call.function.arguments)?;
    let selection = args.get("_output").filter(|_| call.function.name != "read_tool_result");
    let Some(selection) = selection else {
        return prepare(db, user, &call.function.name, &call.id, text);
    };
    let output = db.save_tool_output(user, &call.function.name, &call.id, text)?;
    match tool_output::selected_response(&output, selection) {
        Ok(selected) => Ok(selected),
        Err(error) => Ok(serde_json::json!({
            "output_id":output.id, "error":format!("Output selection unavailable: {error}"),
            "tool_executed":true, "note":"The original tool already executed. Use read_tool_result with a corrected view; do not rerun it."
        }).to_string()),
    }
}

#[cfg(test)]
mod status_tests {
    use super::display_success;

    #[test]
    fn tool_status_rejects_execution_errors_and_failed_receipts() {
        for result in [
            "Error: missing file", "  Error task cancelled",
            r#"{"receipt":{"outcome":"failed","verified":false}}"#,
            r#"{"receipt":{"outcome":"rolled_back","verified":false}}"#,
            r#"{"receipt":{"outcome":"rollback_conflict","verified":false}}"#,
            r#"{"receipt":{"outcome":"passed","verified":false}}"#,
            r#"{"success":false}"#, r#"{"exit_code":1}"#, r#"{"error":"failure"}"#,
        ] {
            assert!(!display_success(result), "{result}");
        }
    }

    #[test]
    fn tool_status_preserves_successful_inspection_of_missing_or_error_text() {
        for result in [
            "plain successful output",
            r#"{"receipt":{"outcome":"committed","verified":true}}"#,
            r#"{"receipt":{"outcome":"passed","verified":true}}"#,
            r#"{"exists":false,"content":null,"sha256":null}"#,
            r#"{"content":"Error: these are file contents"}"#,
            r#"{"exit_code":0,"error":null}"#,
        ] {
            assert!(display_success(result), "{result}");
        }
    }
}
