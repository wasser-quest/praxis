use super::*;

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content: Some(content.into()),
        reasoning_content: None,
        content_parts: None,
        tool_calls: None,
        tool_call_id: None,
        tool_name: None,
    }
}

#[test]
fn mid_history_system_messages_merge_into_leading_slot() {
    // Reproduces the 2026-09-20 incident: a system notice mid-history made
    // Qwen3.6's template reject the request with HTTP 500.
    let mut msgs = vec![
        msg("system", "Du bist ein Assistent."),
        msg("user", "Frage 1"),
        msg("assistant", "Antwort 1"),
        msg("system", "nachträgliche Notiz"),
        msg("user", "Frage 2"),
    ];
    LlamaCppProvider::normalize_system_messages(&mut msgs);
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(
        msgs[0].content.as_deref(),
        Some("Du bist ein Assistent.\n\nnachträgliche Notiz")
    );
    assert_eq!(msgs[1].content.as_deref(), Some("Frage 1"));
    assert_eq!(msgs[3].content.as_deref(), Some("Frage 2"));
}

#[test]
fn no_system_messages_stays_untouched() {
    let mut msgs = vec![msg("user", "hi"), msg("assistant", "hey")];
    LlamaCppProvider::normalize_system_messages(&mut msgs);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
}

#[test]
fn tool_chain_order_is_preserved() {
    let mut msgs = vec![
        msg("system", "sys"),
        msg("user", "q"),
        msg("assistant", "a"),
        msg("system", "notice"),
        msg("tool", "tool-result"),
        msg("user", "next"),
    ];
    LlamaCppProvider::normalize_system_messages(&mut msgs);
    let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool", "user"]);
}

#[test]
fn tool_calls_carry_explicit_function_type_and_thinking_maps_to_template_kwargs() {
    let provider = LlamaCppProvider::new(None, "fixture".into(), "http://127.0.0.1:8080".into());
    let mut request = ChatRequest {
        messages: vec![ChatMessage {
            role: "assistant".into(),
            content: None,
            reasoning_content: None,
            content_parts: None,
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
            }]),
            tool_call_id: None,
            tool_name: None,
        }],
        tools: None,
        temperature: Some(0.25),
        max_tokens: Some(100),
        model: None,
        vision_provider: None,
        vision_model: None,
        thinking: Some(ThinkingMode::Low),
    };
    let body = provider.request_body(request.clone());
    assert_eq!(body["messages"][0]["tool_calls"][0]["type"], "function");
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(body["chat_template_kwargs"]["reasoning_effort"], "low");
    assert_eq!(body["reasoning_effort"], "low");

    request.thinking = Some(ThinkingMode::Off);
    let body = provider.request_body(request);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    assert!(body["reasoning_effort"].is_null());
}

