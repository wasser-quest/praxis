use super::*;
use crate::gateway::llm::provider::FunctionCall;

fn response(content: Option<&str>, reasoning: Option<&str>, finish: &str) -> ChatResponse {
    ChatResponse {
        content: content.map(String::from), reasoning_content: reasoning.map(String::from),
        tool_calls: None, finish_reason: Some(finish.into()), usage: None,
    }
}
#[test]
fn small_model_reasoning_is_not_a_final_answer_or_tool_history() {
    let r = response(None, Some("still thinking"), "length");
    assert_eq!(validate_response(r).unwrap_err().kind, ErrorKind::OutputLimit);
    assert_eq!(validate_response(response(None, Some("only reasoning"), "stop")).unwrap_err().kind, ErrorKind::InvalidResponse);
    let mut r = response(None, Some("tool reasoning"), "tool_calls");
    r.tool_calls = Some(vec![ToolCall { id: "a".into(), function: FunctionCall {name: "read_file".into(), arguments: "{}".into()} }]);
    let r = validate_response(r).unwrap();
    assert!(r.content.is_none());
    assert_eq!(r.reasoning_content.as_deref(), Some("tool reasoning"));
}
#[test]
fn small_model_cutoff_never_executes_a_partial_batch() {
    let mut r = response(Some("partial answer"), None, "length");
    r.tool_calls = Some(vec![
        ToolCall { id: "a".into(), function: FunctionCall {name: "write_file".into(), arguments: "{}".into()} },
        ToolCall { id: "b".into(), function: FunctionCall {name: "write_file".into(), arguments: "{\"path\":".into()} },
    ]);
    assert_eq!(validate_response(r).unwrap_err().kind, ErrorKind::OutputLimit);
}
#[test]
fn small_model_empty_tool_name_is_never_repaired_into_an_action() {
    let mut r = response(None, None, "tool_calls");
    r.tool_calls = Some(vec![ToolCall { id: "a".into(), function: FunctionCall {name: "".into(), arguments: "".into()} }]);
    let error = validate_response(r).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidResponse);
    assert!(error.to_string().contains("without a function name"));
}

#[test]
fn response_validation_distinguishes_missing_answers_and_bad_arguments() {
    for (r, cause) in [
        (response(None, None, "stop"), "no answer text"),
        (response(None, Some("private reasoning"), "stop"), "reasoning only"),
        (response(None, None, "tool_calls"), "reported tool_calls"),
    ] {
        assert!(validate_response(r).unwrap_err().to_string().contains(cause));
    }
    let mut r = response(None, None, "tool_calls");
    r.tool_calls = Some(vec![ToolCall {id:"a".into(),function:FunctionCall {name:"write_file".into(), arguments:"{SECRET".into()}}]);
    let error = validate_response(r).unwrap_err();
    assert!(error.to_string().contains("not a valid JSON object"));
    assert!(!format!("{error:?}").contains("SECRET"));
    let failure = CallFailure {attempts:1, failures:vec![("codex".into(),error.clone())], terminal:error};
    let message = failure.to_string();
    assert!(message.contains("codex:"));
    assert_eq!(message.matches("invalid or empty provider response").count(), 1);
}

#[test]
fn tool_only_failure_aborts_previews_before_retry() {
    let user = "tool-only-preview-abort-test";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let attempt = StreamAttempt::new(Some(user));
    attempt.previewed.store(true, Ordering::Relaxed);
    drop(attempt);
    let mut aborted = false;
    while let Ok(event) = events.try_recv() {
        aborted |= event.event == "stream_abort";
    }
    assert!(aborted);
}
