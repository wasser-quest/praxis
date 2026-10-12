use praxis_provider_anthropic::{
    build_anthropic_messages, parse_anthropic_response, AnthropicProvider,
};
use praxis_provider_api::{ChatProvider, ChatRequest, ErrorKind, ThinkingMode, WireChatRequest};
use serde_json::json;
use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

fn request() -> ChatRequest {
    WireChatRequest {
        messages: vec![],
        tools: None,
        temperature: Some(0.7),
        max_tokens: None,
        model: None,
        thinking: None,
    }
    .into()
}

#[test]
fn checked_cache_usage_and_tool_response_preserve_native_semantics() {
    let response = parse_anthropic_response(&json!({
        "content":[{"type":"text","text":"one"},{"type":"thinking","thinking":"PRIVATE"},
            {"type":"text","text":"two"},{"type":"tool_use","id":"call","name":"lookup","input":{"q":"x"}}],
        "stop_reason":"tool_use", "usage":{"input_tokens":10,"output_tokens":2,
            "cache_creation_input_tokens":3,"cache_read_input_tokens":4,
            "cache_creation":{"ephemeral_5m_input_tokens":3}}
    })).unwrap();
    assert_eq!(response.content.as_deref(), Some("one\ntwo"));
    assert!(response.reasoning_content.is_none());
    assert_eq!(
        response.tool_calls.unwrap()[0].function.arguments,
        r#"{"q":"x"}"#
    );
    let usage = response.usage.unwrap();
    assert_eq!(
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens
        ),
        (17, 2, 19)
    );
    for counts in [
        json!({}),
        json!({"input_tokens":-1,"output_tokens":2}),
        json!({"input_tokens":1.5,"output_tokens":2}),
        json!({"input_tokens":4294967295u64,"output_tokens":1}),
        json!({"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":4294967295u64}),
        json!({"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":"bad"}),
    ] {
        assert!(parse_anthropic_response(&json!({"usage":counts}))
            .unwrap()
            .usage
            .is_none());
    }
}

#[tokio::test]
async fn native_defaults_thinking_model_url_and_output_limits_are_unchanged() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"content":[],"stop_reason":"end_turn"})),
        )
        .expect(4)
        .mount(&server)
        .await;
    let provider =
        AnthropicProvider::new("PRIVATE_TOKEN".into(), "default-model".into(), server.uri());
    assert_eq!(provider.name(), "anthropic");
    assert!(provider.as_any().is::<AnthropicProvider>());
    assert!(provider.supports_output_limit());
    assert_eq!(
        provider.list_models().await.unwrap_err().kind,
        ErrorKind::Unsupported
    );
    provider.chat(request()).await.unwrap();
    for limit in [100, 200] {
        let mut req = request();
        req.max_tokens = Some(limit);
        req.model = Some("override-model".into());
        provider.chat(req).await.unwrap();
    }
    let mut req = request();
    req.thinking = Some(ThinkingMode::Low);
    provider.chat(req).await.unwrap();
    let bodies: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|req| serde_json::from_slice(&req.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["model"], "default-model");
    assert_eq!(bodies[0]["max_tokens"], 4096);
    assert!(bodies[0].get("temperature").is_none());
    assert_eq!(bodies[1]["model"], "override-model");
    assert_eq!(bodies[1]["max_tokens"], 100);
    assert_eq!(bodies[2]["max_tokens"], 200);
    assert_eq!(
        bodies[3]["thinking"],
        json!({"type":"enabled","budget_tokens":1024})
    );
    assert!(bodies[3].get("temperature").is_none());
    // The native URL rule appends /v1/messages, including to an existing /v1.
    Mock::given(path("/v1/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content":[]})))
        .expect(1)
        .mount(&server)
        .await;
    AnthropicProvider::new(
        "PRIVATE_TOKEN".into(),
        "model".into(),
        format!("{}/v1", server.uri()),
    )
    .chat(request())
    .await
    .unwrap();
}

#[test]
fn system_and_tool_content_conversion_is_preserved() {
    let messages: Vec<praxis_provider_api::ChatMessage> = serde_json::from_value(json!([
        {"role":"system","content":"first"},{"role":"system","content":"second"},
        {"role":"assistant","content":"calling","tool_calls":[{"id":"call","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},
        {"role":"tool","tool_call_id":"call","content":"result","content_parts":[
            {"type":"text","text":"extra"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]}
    ])).unwrap();
    let (system, messages) = build_anthropic_messages(&messages);
    assert_eq!(system, "first\n\nsecond");
    assert_eq!(messages[0]["content"][1]["input"], json!({"q":"x"}));
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"][0]["tool_use_id"], "call");
    assert_eq!(
        messages[1]["content"][0]["content"][2]["source"]["data"],
        "AAAA"
    );
}
