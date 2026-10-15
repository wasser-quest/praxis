use super::*;
use std::sync::Mutex;

#[test]
fn stream_preserves_interleaved_tool_fragments_and_reasoning() {
    let mut acc = Accumulator::default();
    let events = Mutex::new(Vec::new());
    let on = &|delta| events.lock().unwrap().push(delta);
    for delta in [
        serde_json::json!({"reasoning_content": "Denke…"}),
        serde_json::json!({"tool_calls": [
            {"index": 0, "id": "a", "function": {"name": "read_file", "arguments": "{\"path\":"}},
            {"index": 1, "id": "b", "function": {"name": "get_context", "arguments": "{"}}
        ]}),
        serde_json::json!({"tool_calls": [
            {"index": 1, "function": {"arguments": "}"}},
            {"index": 0, "function": {"arguments": "\"🙂\"}"}}
        ]}),
    ] {
        acc.push(&serde_json::json!({"choices": [{"delta": delta}]}), &on).unwrap();
    }
    assert_eq!(events.lock().unwrap().len(), 5, "previews available before finish");
    acc.push(&serde_json::json!({"choices": [{"finish_reason": "tool_calls"}]}), &on)
        .unwrap();
    let response = acc.finish().unwrap();
    assert_eq!(response.reasoning_content.as_deref(), Some("Denke…"));
    let calls = response.tool_calls.unwrap();
    assert_eq!(calls[0].function.arguments, "{\"path\":\"🙂\"}");
    assert_eq!(calls[1].function.arguments, "{}");
}

#[test]
fn stream_requires_explicit_completion() {
    assert!(Accumulator::default().finish().is_err());
}

#[test]
fn usage_metrics_preserve_unknown_and_deduplicate_snapshots() {
    for (usage, expected) in [
        (serde_json::json!({"completion_tokens": 5}), serde_json::Value::Null),
        (
            serde_json::json!({"prompt_tokens": 7, "completion_tokens": 5}),
            serde_json::json!({"prompt_tokens": 7, "completion_tokens": 5, "total_tokens": 12}),
        ),
    ] {
        let mut acc = Accumulator::default();
        for _ in 0..2 {
            acc.push(
                &serde_json::json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}], "usage": usage}),
                &|_| {},
            )
            .unwrap();
        }
        assert_eq!(serde_json::to_value(acc.finish().unwrap().usage).unwrap(), expected);
    }
}

#[test]
fn stream_output_limit_and_inband_error_are_typed() {
    use praxis_provider_api::ErrorKind;
    let mut acc = Accumulator::default();
    let err = acc
        .push(
            &serde_json::json!({"choices": [{"delta": {"content": "x".repeat(MAX_STREAM_BYTES + 1)}}]}),
            &|_| {},
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutputLimit);

    let mut acc = Accumulator::default();
    let err = acc
        .push(&serde_json::json!({"error": "PRIVATE upstream detail"}), &|_| {})
        .unwrap_err();
    assert!(!format!("{err:?} {err}").contains("PRIVATE"));
}

#[test]
fn tool_call_index_without_value_and_oversized_index_fail_closed() {
    let mut acc = Accumulator::default();
    for payload in [
        serde_json::json!({"choices": [{"delta": {"tool_calls": [{"function": {"name": "x"}}]}}]}),
        serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": MAX_TOOL_CALL_INDEX, "function": {"name": "x"}}]}}]}),
    ] {
        let err = acc.push(&payload, &|_| {}).unwrap_err();
        assert_eq!(err.kind, praxis_provider_api::ErrorKind::InvalidResponse);
    }
}
