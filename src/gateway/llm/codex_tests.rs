use super::super::error::{ErrorKind, ProviderError};
use super::*;
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: Some("Say hello".into()),
            reasoning_content: None,
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }],
        tools: None,
        temperature: None,
        max_tokens: None,
        model: None,
        vision_provider: None,
        vision_model: None,
        thinking: None,
    }
}

fn provider(server: &MockServer) -> CodexProvider {
    CodexProvider::with_urls(
        CodexAuth {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            ..Default::default()
        },
        DEFAULT_MODEL.into(),
        format!("{}/responses", server.uri()),
        format!("{}/token", server.uri()),
        None,
    )
}

async fn stream(server: &MockServer, text: &str) {
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(text, "text/event-stream"),
        )
        .mount(server)
        .await;
}

#[path = "codex_continuation_tests.rs"]
mod continuation_tests;

const COMPLETE: &str = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Grüße 日本語\"}]}]}}\r\n\r\n";

#[tokio::test]
async fn codex_crlf_and_unicode_streams() {
    let server = MockServer::start().await;
    stream(&server, COMPLETE).await;
    assert_eq!(
        provider(&server)
            .chat(request())
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("Grüße 日本語")
    );
}

#[tokio::test]
async fn branding_codex_identifies_praxis_and_preserves_subscription_protocol() {
    let server = MockServer::start().await;
    stream(&server, COMPLETE).await;
    provider(&server).chat(request()).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["user-agent"],
        concat!("Praxis/", env!("CARGO_PKG_VERSION"), " (+https://getpraxis.boo)"));
    assert_eq!(requests[0].headers["originator"], "praxis");
    assert_eq!(requests[0].headers["authorization"], "Bearer access");
}

#[tokio::test]
async fn codex_requires_terminal_success_and_valid_json() {
    for (text, kind) in [
        ("data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n", ErrorKind::Interrupted),
        ("data: [DONE]\n\n", ErrorKind::Interrupted),
        ("data: not-json\n\n", ErrorKind::InvalidResponse),
        ("data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n", ErrorKind::InvalidResponse),
        ("data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"output\":[{\"type\":\"function_call\",\"call_id\":\"c\",\"name\":\"execute_terminal\",\"arguments\":\"{}\"}]}}\n\n", ErrorKind::OutputLimit),
    ] {
        let server = MockServer::start().await;
        stream(&server, text).await;
        let err = provider(&server).chat(request()).await.expect_err("must not return an incomplete answer or tool call");
        assert_eq!(err.kind, kind);
    }
}

#[tokio::test]
async fn codex_completed_tools_must_have_valid_arguments_names_and_unique_ids() {
    let valid = serde_json::json!({"type":"function_call", "status":"completed",
        "call_id":"call_1", "name":"read_file", "arguments":"{\"path\":\"a\"}"});
    let mut invalid_outputs = Vec::new();
    for (field, value) in [
        ("arguments", "{\"path\":"), ("arguments", "[]"), ("arguments", ""),
        ("name", ""), ("call_id", ""), ("status", "in_progress"),
    ] {
        let mut call = valid.clone();
        call[field] = value.into();
        invalid_outputs.push(vec![call]);
    }
    invalid_outputs.push(vec![valid.clone(), valid]);
    for output in invalid_outputs {
        let server = MockServer::start().await;
        let event = serde_json::json!({"type":"response.completed",
            "response":{"status":"completed", "output":output}});
        stream(&server, &format!("data: {event}\n\n")).await;
        let error = provider(&server).chat(request()).await.expect_err("invalid calls must never become executable");
        assert_eq!(error.kind, ErrorKind::InvalidResponse);
    }
}

#[tokio::test]
async fn codex_both_stream_entry_points_forward_their_display_deltas() {
    let server = MockServer::start().await;
    let events = [
        serde_json::json!({"type":"response.reasoning_summary_text.delta", "delta":"Synthetic reasoning"}),
        serde_json::json!({"type":"response.refusal.delta", "delta":"Synthetic refusal"}),
        serde_json::json!({"type":"response.output_item.added", "output_index":2,
            "item":{"type":"function_call", "call_id":"call_1", "name":"read_file"}}),
        serde_json::json!({"type":"response.function_call_arguments.delta", "output_index":2,
            "delta":"{\"path\":\"a\"}"}),
        serde_json::json!({"type":"response.completed", "response":{"status":"completed", "output":[
            {"type":"message", "content":[{"type":"refusal", "refusal":"Synthetic refusal"}]},
            {"type":"function_call", "status":"completed", "call_id":"call_1", "name":"read_file", "arguments":"{\"path\":\"a\"}"}
        ]}}),
    ];
    let sse: String = events.iter().map(|event| format!("data: {event}\r\n\r\n")).collect();
    stream(&server, &sse).await;
    let provider = provider(&server);
    let deltas = Mutex::new(Vec::new());
    let response = provider.chat_stream_events(request(), &|delta| deltas.lock().unwrap().push(delta)).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("Synthetic refusal"));
    assert_eq!(response.reasoning_content.as_deref(), Some("Synthetic reasoning"));
    assert_eq!(response.tool_calls.as_ref().unwrap()[0].function.arguments, "{\"path\":\"a\"}");
    assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    let deltas = deltas.into_inner().unwrap();
    assert_eq!(deltas.len(), 4);
    assert!(matches!(&deltas[0], StreamDelta::Reasoning { text } if text == "Synthetic reasoning"));
    assert!(matches!(&deltas[1], StreamDelta::Text { text } if text == "Synthetic refusal"));
    assert!(matches!(&deltas[2], StreamDelta::ToolCall { index:2, id:Some(id), name:Some(name), arguments:None }
        if id == "call_1" && name == "read_file"));
    assert!(matches!(&deltas[3], StreamDelta::ToolCall { index:2, id:None, name:None, arguments:Some(args) }
        if args == "{\"path\":\"a\"}"));
    let text = Mutex::new(String::new());
    provider.chat_stream(request(), &|delta| text.lock().unwrap().push_str(&delta)).await.unwrap();
    assert_eq!(*text.lock().unwrap(), "Synthetic refusal");
}

#[tokio::test]
async fn codex_streamed_tools_with_empty_terminal_output_execute_once_and_continue() {
    use serde_json::{json, Value};
    let server = MockServer::start().await;
    Mock::given(path("/responses"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = req.body_json().unwrap();
            let input = body["input"].as_array().unwrap();
            let results: Vec<_> = input.iter()
                .filter(|item| item["type"] == "function_call_output").collect();
            let events = if results.is_empty() {
                vec![
                    json!({"type":"response.output_item.added","output_index":0,"item":{
                        "type":"function_call","id":"fc_1","call_id":"call_1",
                        "name":"agent_feedback","arguments":"","status":"in_progress"}}),
                    json!({"type":"response.function_call_arguments.delta","output_index":0,
                        "delta":"{\"message\":"}),
                    json!({"type":"response.function_call_arguments.delta","output_index":0,
                        "delta":"\"Exactly once\"}"}),
                    json!({"type":"response.output_item.done","output_index":0,"item":{
                        "type":"function_call","id":"fc_1","call_id":"call_1",
                        "name":"agent_feedback","arguments":"{\"message\":\"Exactly once\"}",
                        "status":"completed"}}),
                    json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
                ]
            } else {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0]["call_id"], "call_1");
                let calls: Vec<_> = input.iter()
                    .filter(|item| item["type"] == "function_call").collect();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0]["call_id"], "call_1");
                assert_eq!(calls[0]["arguments"], "{\"message\":\"Exactly once\"}");
                vec![json!({"type":"response.completed","response":{"status":"completed",
                    "output":[{"type":"message","content":[{"type":"output_text","text":"Finished"}]}]}})]
            };
            let sse: String = events.iter().map(|event| format!("data: {event}\n\n")).collect();
            ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream")
        })
        .expect(3) // One streamed preview request, then a two-turn tool loop; no retries.
        .mount(&server).await;
    let router = super::super::LLMRouter::with_providers(
        vec![Box::new(provider(&server))], "codex".into(), vec![], Default::default(),
    );
    let user = "codex-empty-terminal-tool-loop";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let mut req = request();
    req.tools = Some(vec![ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: "agent_feedback".into(), description: "Feedback".into(),
            parameters: json!({"type":"object","properties":{"message":{"type":"string"}}}),
        },
    }]);
    let response = router.chat_controlled(
        req.clone(), None, Some(user), &tokio_util::sync::CancellationToken::new(),
    ).await.unwrap();
    assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(response.tool_calls.unwrap().len(), 1);
    let mut previews = 0;
    let mut ends = 0;
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "stream_abort");
        if event.event == "tool_call_delta" { previews += 1; }
        if event.event == "stream_end" { ends += 1; }
    }
    assert_eq!(previews, 3);
    assert_eq!(ends, 1);

    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let result = router.chat_with_tools(
        &db, user, req.messages, req.tools.unwrap(), Some(2), None, None, None,
    ).await.unwrap();
    assert_eq!(result.response, "Finished");
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.feedback_messages, vec!["Exactly once"]);
    crate::runtime::events::remove(user);
}

#[tokio::test]
async fn codex_refresh_rejection_requires_fresh_login_without_echoing_secrets() {
    for status in [400, 401, 403] {
        let server = MockServer::start().await;
        Mock::given(path("/responses")).respond_with(ResponseTemplate::new(401))
            .expect(1).mount(&server).await;
        Mock::given(path("/token")).respond_with(ResponseTemplate::new(status)
            .set_body_json(serde_json::json!({"error":{"message":"PRIVATE TOKEN", "code":"invalid_grant"}})))
            .expect(1).mount(&server).await;
        let error = provider(&server).chat(request()).await.unwrap_err();
        let typed = &error;
        assert_eq!(typed.kind, ErrorKind::Authentication);
        assert!(typed.cause.unwrap().contains("/login codex --device-auth"));
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
}

#[tokio::test]
async fn codex_stream_errors_are_typed_and_do_not_echo_server_messages() {
    let server = MockServer::start().await;
    stream(&server, "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"SECRET PROMPT\"}}}\n\n").await;
    let err = provider(&server).chat(request()).await.unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::RateLimited
    );
    assert!(!format!("{err:?}").contains("SECRET"));
}

#[tokio::test]
async fn codex_rate_limit_reset_headers_reach_router() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("x-codex-primary-used-percent", "100")
                .insert_header("x-codex-primary-reset-after-seconds", "42")
                .set_body_json(serde_json::json!({"error":{"type":"usage_limit_reached"}})),
        )
        .mount(&server)
        .await;
    let err = provider(&server).chat(request()).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);
    assert_eq!(err.retry_after, Some(std::time::Duration::from_secs(42)));
}

#[tokio::test]
async fn codex_parallel_401s_refresh_only_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer access"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(50))
                .set_body_json(
                    serde_json::json!({"access_token":"fresh", "refresh_token":"rotated"}),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPLETE, "text/event-stream"))
        .expect(2)
        .mount(&server)
        .await;
    let first = provider(&server);
    let reloaded = provider(&server); // overlapping router generations share refresh
    let (a, b) = tokio::join!(first.chat(request()), reloaded.chat(request()));
    assert!(a.is_ok(), "{a:?}");
    assert!(b.is_ok(), "{b:?}");
}

#[tokio::test]
async fn codex_wire_body_covers_tools_images_history_and_reasoning() {
    let server = MockServer::start().await;
    let provider = provider(&server);
    let mut req = request();
    req.thinking = Some(ThinkingMode::Off);
    req.temperature = Some(0.2);
    req.max_tokens = Some(123);
    req.messages[0].content_parts = Some(vec![ContentPart::ImageUrl {
        image_url: ImageUrlDetail {
            url: "data:image/png;base64,aA==".into(),
            detail: None,
        },
    }]);
    req.messages.push(ChatMessage {
        role: "assistant".into(),
        content: None,
        reasoning_content: None,
        content_parts: None,
        tool_calls: Some(vec![ToolCall {
            id: "call_1".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            },
        }]),
        tool_call_id: None,
        tool_name: None,
    });
    req.messages.push(ChatMessage {
        role: "tool".into(),
        content: Some("result".into()),
        reasoning_content: None,
        content_parts: None,
        tool_calls: None,
        tool_call_id: Some("call_1".into()),
        tool_name: Some("read_file".into()),
    });
    req.tools = Some(vec![ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: "read_file".into(),
            description: "Read".into(),
            parameters: serde_json::json!({"type":"object", "properties":{"path":{"type":"string"}}}),
        },
    }]);
    let body = provider.build_body(&req);
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert!(body.get("temperature").is_none());
    assert!(
        body.get("max_output_tokens").is_none(),
        "unsupported by subscription endpoint"
    );
    assert_eq!(
        body["reasoning"]["effort"], "low",
        "Codex cannot disable reasoning; use its minimum"
    );
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"][0]["strict"], false);
    assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][2]["call_id"], "call_1");
    assert_eq!(body["input"][2]["output"], "result");
}

#[tokio::test]
async fn codex_router_does_not_retry_incomplete_with_an_ignored_output_limit() {
    let server = MockServer::start().await;
    Mock::given(path("/responses")).respond_with(ResponseTemplate::new(200).set_body_raw(
        "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        "text/event-stream",
    )).expect(1).mount(&server).await;
    let router = super::super::LLMRouter::with_providers(
        vec![Box::new(provider(&server))],
        "codex".into(),
        vec![],
        Default::default(),
    );
    let mut req = request();
    req.max_tokens = Some(10);
    let error = router.chat(req, None).await.unwrap_err();
    let failure = error
        .downcast_ref::<super::super::error::CallFailure>()
        .unwrap();
    assert_eq!(failure.attempts, 1);
    assert_eq!(failure.terminal.kind, ErrorKind::OutputLimit);
}

#[test]
fn codex_retired_router_refresh_cannot_restore_logout_or_replace_a_new_account() {
    let _guard = crate::db::secrets::test_lock();
    let original = crate::db::secrets::get_secrets();
    let auth = CodexAuth {
        access_token: "old-access".into(),
        refresh_token: "old-refresh".into(),
        ..Default::default()
    };
    let next = CodexAuth {
        access_token: "new-access".into(),
        refresh_token: "new-refresh".into(),
        ..Default::default()
    };
    let mut secrets = crate::db::secrets::Secrets::default();
    auth.store(&mut secrets);
    crate::db::secrets::init_secrets(secrets.clone());
    let router = super::super::LLMRouter::new(&crate::config::Config::from_env(), &secrets);
    let provider = router
        .providers
        .iter()
        .find(|p| p.name() == "codex")
        .unwrap()
        .as_any()
        .downcast_ref::<CodexProvider>()
        .unwrap();
    let refresh = provider.on_refresh.as_ref().unwrap();
    refresh(&auth, &next);
    assert_eq!(
        CodexAuth::from_secrets(&crate::db::secrets::get_secrets()),
        Some(next.clone())
    );
    crate::db::secrets::init_secrets(Default::default());
    refresh(&auth, &next);
    assert!(CodexAuth::from_secrets(&crate::db::secrets::get_secrets()).is_none());
    let other = CodexAuth {
        access_token: "another-account".into(),
        refresh_token: "other-refresh".into(),
        ..Default::default()
    };
    other.store(&mut secrets);
    crate::db::secrets::init_secrets(secrets);
    refresh(&auth, &next);
    assert_eq!(
        CodexAuth::from_secrets(&crate::db::secrets::get_secrets()),
        Some(other)
    );
    crate::db::secrets::init_secrets(original);
}

/// Explicit opt-in only. Does not read ~/.codex or persist/print credentials.
#[tokio::test]
#[ignore = "requires PRAXIS_CODEX_AUTH_FILE and a ChatGPT subscription; makes a live request"]
async fn codex_live_smoke() {
    let path =
        std::env::var("PRAXIS_CODEX_AUTH_FILE").expect("set PRAXIS_CODEX_AUTH_FILE explicitly");
    let mut auth = CodexAuth::from_cli_file(&std::fs::read_to_string(path).unwrap()).unwrap();
    // A read-only smoke test must never rotate the supplied client's tokens.
    auth.refresh_token.clear();
    let model = std::env::var("CODEX_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        CodexProvider::new(auth, model, None).chat(request()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.content.is_some_and(|s| !s.trim().is_empty()));
}
