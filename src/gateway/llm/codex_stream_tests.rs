use super::*;
use serde_json::json;
use std::sync::Mutex;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
use crate::gateway::llm::error::ProviderError;

fn event(value: Value) -> crate::sse::Event {
    crate::sse::Event {
        event: "message".into(),
        data: value.to_string(),
    }
}
fn completed(output: Value) -> Value {
    json!({"type":"response.completed","response":{"status":"completed","output":output}})
}
fn call() -> Value {
    json!({"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"日本🙂\"}"})
}
fn wire(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| format!("data: {value}\n\n"))
        .collect()
}
async fn serve(body: String, content_type: &str) -> (MockServer, reqwest::Response) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, content_type)
                .insert_header("x-request-id", "req_test-123"),
        )
        .mount(&server)
        .await;
    let response = http::client().get(server.uri()).send().await.unwrap();
    (server, response)
}

#[test]
fn codex_crlf_utf8_and_tool_previews_survive_every_fragment_boundary() {
    let input = wire(&[
        json!({"type":"response.reasoning_summary_text.delta","delta":"考える🙂\n"}),
        json!({"type":"response.output_text.delta","delta":"Hello 日本🙂\n\nsecond line"}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":"}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"\"日本🙂\"}"}),
        json!({"type":"response.output_item.done","item":call()}),
        completed(json!([{"type":"message","content":[{"type":"output_text","text":"Hello 日本🙂\n\nsecond line"}]},call()])),
    ]).replace('\n', "\r\n");
    for size in 1..=input.len().min(100) {
        let mut decoder = crate::sse::Decoder::default();
        let mut acc = Accumulator::default();
        let deltas = Mutex::new(Vec::new());
        let mut result = None;
        for chunk in input.as_bytes().chunks(size) {
            for ev in decoder.push(chunk).unwrap() {
                if let Some(response) = acc
                    .push(ev, &|delta| deltas.lock().unwrap().push(delta))
                    .unwrap()
                {
                    result = Some(response);
                }
            }
        }
        let attempt = result.unwrap();
        assert!(attempt.continuation.is_none());
        let result = attempt.response;
        assert_eq!(
            result.content.as_deref(),
            Some("Hello 日本🙂\n\nsecond line")
        );
        assert_eq!(result.reasoning_content.as_deref(), Some("考える🙂\n"));
        assert_eq!(
            result.tool_calls.unwrap()[0].function.arguments,
            "{\"path\":\"日本🙂\"}"
        );
        assert_eq!(deltas.lock().unwrap().len(), 5);
    }
}

#[test]
fn codex_accepts_event_names_and_done_alias_but_not_unfinished_items() {
    let mut acc = Accumulator::default();
    assert!(acc
        .push(
            event(json!({"type":"response.output_item.done","item":call()})),
            &|_| {}
        )
        .unwrap()
        .is_none());
    let response = acc
        .push(
            crate::sse::Event {
                event: "response.done".into(),
                data: json!({"response":{"status":"completed", "output":[call()]}}).to_string(),
            },
            &|_| {},
        )
        .unwrap()
        .unwrap();
    assert_eq!(response.response.tool_calls.unwrap().len(), 1);
    assert_eq!(
        Accumulator::default()
            .push(
                crate::sse::Event {
                    event: "message".into(),
                    data: "[DONE]".into()
                },
                &|_| {}
            )
            .unwrap_err()
            .kind,
        ErrorKind::Interrupted
    );
}

#[test]
fn codex_nonempty_terminal_output_is_authoritative_and_usage_totals_are_recovered() {
    let mut acc = Accumulator::default();
    acc.push(
        event(json!({"type":"response.output_item.done", "item":call()})),
        &|_| {},
    )
    .unwrap();
    let mut final_call = call();
    final_call["arguments"] = json!("{\"path\":\"final\"}");
    let mut response = completed(json!([final_call]));
    response["response"]["usage"] = json!({"input_tokens":7,"output_tokens":5});
    let response = acc.push(event(response), &|_| {}).unwrap().unwrap().response;
    let calls = response.tool_calls.unwrap();
    assert_eq!(calls.len(), 1, "terminal and streamed items must not be duplicated");
    assert_eq!(calls[0].function.arguments, "{\"path\":\"final\"}");
    assert_eq!(response.usage.unwrap().total_tokens, 12);

    // A nonempty terminal output replaces, rather than merges with, done items.
    let response = acc
        .push(
            event(completed(json!([{"type":"message","content":[{"text":"final answer"}]}]))),
            &|_| {},
        )
        .unwrap()
        .unwrap()
        .response;
    assert_eq!(response.content.as_deref(), Some("final answer"));
    assert!(response.tool_calls.is_none());
}

#[test]
fn codex_completed_items_supply_output_when_terminal_omits_it() {
    for kind in ["response.completed", "response.done"] {
        for output in [None, Some(Value::Null), Some(json!([]))] {
            let mut acc = Accumulator::default();
            assert!(acc
                .push(
                    event(json!({"type":"response.output_item.done","output_index":0,"item":call()})),
                    &|_| {},
                )
                .unwrap()
                .is_none(), "done items must wait for successful response completion");
            let mut terminal = json!({"type":kind,"response":{"status":"completed",
                "usage":{"input_tokens":7,"output_tokens":5}}});
            if let Some(output) = output {
                terminal["response"]["output"] = output;
            }
            let attempt = acc.push(event(terminal), &|_| {}).unwrap().unwrap();
            assert!(attempt.continuation.is_none());
            assert_eq!(attempt.response.finish_reason.as_deref(), Some("tool_calls"));
            let calls = attempt.response.tool_calls.unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_1");
            assert_eq!(calls[0].function.name, "read_file");
            assert_eq!(calls[0].function.arguments, "{\"path\":\"日本🙂\"}");
            assert_eq!(attempt.response.usage.unwrap().total_tokens, 12);
        }
    }
}

#[test]
fn codex_preview_fragments_never_supply_missing_terminal_tools() {
    let mut acc = Accumulator::default();
    for value in [
        json!({"type":"response.output_item.added","output_index":0,"item":call()}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{}"}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"arguments":"{}"}),
    ] {
        assert!(acc.push(event(value), &|_| {}).unwrap().is_none());
    }
    assert_eq!(
        acc.push(event(completed(json!([]))), &|_| {}).unwrap_err().kind,
        ErrorKind::InvalidResponse
    );
}

#[test]
fn codex_done_item_fallback_preserves_messages_and_replayable_reasoning() {
    let reasoning = json!({"type":"reasoning","id":"rs_1","summary":[],
        "encrypted_content":"OPAQUE-SECRET"});
    let mut acc = Accumulator::default();
    acc.push(
        event(json!({"type":"response.output_item.done","item":reasoning})),
        &|_| {},
    ).unwrap();
    let attempt = acc.push(event(completed(json!([]))), &|_| {}).unwrap().unwrap();
    assert_eq!(attempt.response.finish_reason.as_deref(), Some("reasoning"));
    let state = attempt.continuation.unwrap();
    assert!(!format!("{state:?}").contains("OPAQUE-SECRET"));
    let ProviderContinuation::Codex { input } = state;
    assert_eq!(input, vec![reasoning]);

    // Done-only messages also work when text deltas were omitted by a proxy.
    acc.push(
        event(json!({"type":"response.output_item.done","item":{"type":"message",
            "content":[{"type":"output_text","text":"Hello 日本🙂"}]}})),
        &|_| {},
    ).unwrap();
    let attempt = acc.push(event(completed(json!([]))), &|_| {}).unwrap().unwrap();
    assert!(attempt.continuation.is_none());
    assert_eq!(attempt.response.content.as_deref(), Some("Hello 日本🙂"));
}

#[test]
fn codex_done_items_do_not_override_failed_incomplete_or_malformed_terminals() {
    for (terminal, kind) in [
        (json!({"type":"response.failed","response":{"error":{"code":"server_error"}}}), ErrorKind::Unavailable),
        (json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}), ErrorKind::OutputLimit),
        (json!({"type":"response.completed","response":{"status":"failed","error":{"code":"server_error"}}}), ErrorKind::Unavailable),
        (json!({"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}), ErrorKind::OutputLimit),
        (json!({"type":"response.completed","response":{"status":"in_progress","output":[]}}), ErrorKind::InvalidResponse),
        (json!({"type":"response.completed","response":{"status":"completed","output":{}}}), ErrorKind::InvalidResponse),
        (json!({"type":"response.completed"}), ErrorKind::InvalidResponse),
    ] {
        let mut acc = Accumulator::default();
        acc.push(event(json!({"type":"response.output_item.done","item":call()})), &|_| {}).unwrap();
        assert_eq!(acc.push(event(terminal), &|_| {}).unwrap_err().kind, kind);
    }
    let mut acc = Accumulator::default();
    acc.push(event(json!({"type":"response.output_item.done","item":call()})), &|_| {}).unwrap();
    assert_eq!(acc.push(crate::sse::Event {
        event: "message".into(), data: "[DONE]".into(),
    }, &|_| {}).unwrap_err().kind, ErrorKind::Interrupted);
}

#[tokio::test]
async fn codex_done_item_fallback_validates_the_entire_tool_batch() {
    for (field, value, diagnostic) in [
        ("call_id", json!(""), "missing call_id"),
        ("call_id", json!("call_1"), "duplicate call_ids"),
        ("name", json!(""), "missing its name"),
        ("arguments", json!("{SECRET"), "not a valid JSON object"),
        ("arguments", json!("[]"), "not a valid JSON object"),
        ("arguments", json!({}), "not a string"),
        ("status", json!("in_progress"), "not completed"),
    ] {
        let mut bad_call = call();
        bad_call["call_id"] = json!("call_2");
        bad_call[field] = value;
        let (_server, response) = serve(wire(&[
            json!({"type":"response.output_item.done","output_index":0,"item":call()}),
            json!({"type":"response.output_item.done","output_index":1,"item":bad_call}),
            completed(json!([])),
        ]), "text/event-stream").await;
        let error = receive(response, &|_| {}).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidResponse);
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(error.to_string().contains("req_test-123"));
        assert!(!format!("{error:?}").contains("SECRET"));
    }
}

#[tokio::test]
async fn codex_stream_failures_keep_exhausted_rate_windows_and_diagnostics() {
    let server = MockServer::start().await;
    let body = wire(&[
        json!({"type":"response.failed","response":{"error":{"code":"usage_limit_reached","message":"SECRET"}}}),
    ]);
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream")
                .insert_header("x-codex-primary-used-percent", "100")
                .insert_header("x-codex-primary-reset-after-seconds", "42")
                .insert_header("x-codex-secondary-used-percent", "100")
                .insert_header("x-codex-secondary-reset-after-seconds", "90")
                .insert_header("retry-after", "10")
                .insert_header("x-request-id", "req_rate-window"),
        )
        .mount(&server)
        .await;
    let response = http::client().get(server.uri()).send().await.unwrap();
    let error = receive(response, &|_| {}).await.unwrap_err();
    let error = &error;
    assert_eq!(error.kind, ErrorKind::RateLimited);
    assert_eq!(error.retry_after, Some(std::time::Duration::from_secs(90)));
    assert_eq!(error.request_id.as_deref(), Some("req_rate-window"));
    assert!(error.cause.is_some());
    assert!(!format!("{error:?}").contains("SECRET"));
}

#[test]
fn codex_empty_and_malformed_events_have_specific_diagnostics() {
    for (value, cause) in [
        (completed(json!([])), "without any text"),
        (
            completed(json!([{"type":"custom_tool_call"}])),
            "unsupported output",
        ),
        (json!({"type":"response.completed"}), "missing its response"),
    ] {
        let error = Accumulator::default()
            .push(event(value), &|_| {})
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidResponse);
        assert!(error.to_string().contains(cause), "{error}");
        assert!(!format!("{error:?}").contains("private thought"));
    }
    let error = Accumulator::default()
        .push(
            crate::sse::Event {
                event: "message".into(),
                data: "{SECRET".into(),
            },
            &|_| {},
        )
        .unwrap_err();
    assert!(error.to_string().contains("malformed JSON"));
    assert!(!format!("{error:?}").contains("SECRET"));
}

#[test]
fn codex_reasoning_only_is_a_continuation_not_an_answer() {
    for item in [
        json!({"type":"reasoning", "summary":[{"text":"[headline]Prüfen[/headline]"}]}),
        json!({"type":"reasoning", "summary":[], "encrypted_content":"OPAQUE-SECRET"}),
    ] {
        let attempt = Accumulator::default()
            .push(event(completed(json!([item.clone()]))), &|_| {})
            .unwrap()
            .unwrap();
        assert_eq!(attempt.response.finish_reason.as_deref(), Some("reasoning"));
        assert!(attempt.response.content.is_none());
        assert!(attempt.response.tool_calls.is_none());
        let state = attempt.continuation.unwrap();
        assert!(!format!("{state:?}").contains("OPAQUE-SECRET"));
        let ProviderContinuation::Codex { input } = state;
        assert_eq!(input, vec![item]);
    }
}

#[test]
fn codex_reasoning_headlines_parts_and_done_events_are_not_lost_or_duplicated() {
    let mut acc = Accumulator::default();
    let deltas = Mutex::new(String::new());
    let on_delta = |delta| {
        if let StreamDelta::Reasoning { text } = delta {
            deltas.lock().unwrap().push_str(&text);
        }
    };
    for value in [
        json!({"type":"response.reasoning_summary_part.added", "output_index":0, "summary_index":0, "part":{"type":"summary_text", "text":""}}),
        json!({"type":"response.reasoning_summary_text.delta", "output_index":0, "summary_index":0, "delta":"[headline]Prüfen"}),
        json!({"type":"response.reasoning_summary_text.delta", "output_index":0, "summary_index":0, "delta":"[/headline]"}),
        json!({"type":"response.reasoning_summary_text.done", "output_index":0, "summary_index":0, "text":"[headline]Prüfen[/headline]"}),
        json!({"type":"response.reasoning_summary_part.done", "output_index":0, "summary_index":0, "part":{"text":"[headline]Prüfen[/headline]"}}),
        // Some proxies only retain done events. Preserve their text too.
        json!({"type":"response.reasoning_summary_text.done", "output_index":0, "summary_index":1, "text":"Nächster Schritt 日本🙂"}),
        json!({"type":"response.output_item.done", "output_index":0, "item":{"type":"reasoning", "summary":[{"text":"[headline]Prüfen[/headline]"},{"text":"Nächster Schritt 日本🙂"}]}}),
    ] {
        assert!(acc.push(event(value), &on_delta).unwrap().is_none());
    }
    let expected = "[headline]Prüfen[/headline]\n\nNächster Schritt 日本🙂";
    assert_eq!(*deltas.lock().unwrap(), expected);
    // With and without a terminal summary, identical text must be retained.
    for summary in [
        json!([]),
        json!([{"text":"[headline]Prüfen[/headline]"},{"text":"Nächster Schritt 日本🙂"}]),
    ] {
        let result = acc
            .push(
                event(completed(json!([{"type":"reasoning", "summary":summary}]))),
                &on_delta,
            )
            .unwrap()
            .unwrap();
        assert_eq!(result.response.reasoning_content.as_deref(), Some(expected));
    }
}

#[test]
fn codex_incomplete_and_failure_events_are_typed_without_leaking_messages() {
    for (code, kind) in [
        ("rate_limit_exceeded", ErrorKind::RateLimited),
        ("insufficient_quota", ErrorKind::QuotaExhausted),
        ("invalid_api_key", ErrorKind::Authentication),
        ("model_not_found", ErrorKind::InvalidRequest),
        ("context_length_exceeded", ErrorKind::ContextWindow),
        ("server_error", ErrorKind::Unavailable),
        ("SECRET", ErrorKind::InvalidResponse),
    ] {
        for value in [
            json!({"type":"response.failed","response":{"error":{"code":code,"message":"SECRET"}}}),
            json!({"type":"error","code":code,"message":"SECRET"}),
        ] {
            let error = Accumulator::default()
                .push(event(value), &|_| {})
                .unwrap_err();
            assert_eq!(error.kind, kind);
            assert!(!format!("{error:?} {error}").contains("SECRET"));
        }
    }
    for terminal in ["response.incomplete", "response.completed", "response.done"] {
        let value = json!({"type":terminal,"response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[call()]}});
        let error = Accumulator::default()
            .push(event(value), &|_| {})
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::OutputLimit);
    }
}

#[tokio::test]
async fn codex_eof_never_commits_partial_text_or_tools_and_retains_request_id() {
    for values in [
        vec![],
        vec![json!({"type":"response.output_text.delta","delta":"partial"})],
        vec![json!({"type":"response.output_item.done","item":call()})],
    ] {
        let (_server, response) = serve(wire(&values), "text/event-stream").await;
        let error = receive(response, &|_| {}).await.unwrap_err();
        let error = &error;
        assert_eq!(error.kind, ErrorKind::Interrupted);
        assert_eq!(error.request_id.as_deref(), Some("req_test-123"));
        assert!(error.to_string().contains("before response.completed"));
    }
}

#[tokio::test]
async fn codex_invalid_tool_batch_is_rejected_with_request_id_and_specific_cause() {
    for (field, value, diagnostic) in [
        ("call_id", json!(""), "missing call_id"),
        ("name", json!(""), "missing its name"),
        ("arguments", json!("{SECRET"), "not a valid JSON object"),
        ("arguments", json!([]), "not a string"),
    ] {
        let mut bad_call = call();
        bad_call[field] = value;
        let (_server, response) = serve(
            wire(&[completed(json!([bad_call, call()]))]),
            "text/event-stream",
        )
        .await;
        let error = receive(response, &|_| {}).await.unwrap_err();
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(error.to_string().contains("req_test-123"));
        assert!(!format!("{error:?}").contains("SECRET"));
    }
}

#[tokio::test]
async fn codex_rejects_non_sse_and_malformed_json_instead_of_empty_success() {
    for (body, content_type, diagnostic) in [
        ("{\"error\":\"SECRET\"}", "application/json", "non-SSE"),
        ("data: {SECRET\n\n", "text/event-stream", "malformed JSON"),
    ] {
        let (_server, response) = serve(body.into(), content_type).await;
        let error = receive(response, &|_| {}).await.unwrap_err();
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(!format!("{error:?}").contains("SECRET"));
        assert!(error.to_string().contains("req_test-123"));
    }
}

#[tokio::test]
async fn codex_router_does_not_replay_visible_text_or_unsupported_budget_increases() {
    use crate::gateway::llm::{
        codex::{CodexAuth, CodexProvider},
        error::CallFailure,
        resilience::ResilienceConfig,
        LLMRouter,
    };
    for (body, expected) in [
        (
            wire(&[
                json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            ]),
            ErrorKind::OutputLimit,
        ),
        (
            wire(&[json!({"type":"response.output_text.delta","delta":"already visible"})]),
            ErrorKind::PartialStream,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = CodexProvider::with_urls(
            CodexAuth {
                access_token: "synthetic".into(),
                ..Default::default()
            },
            "gpt-5-codex".into(),
            server.uri(),
            server.uri(),
            None,
        );
        let router = LLMRouter::with_providers(
            vec![Box::new(provider)],
            "codex".into(),
            vec![],
            ResilienceConfig {
                max_attempts: 3,
                ..Default::default()
            },
        );
        let request = ChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: Some(100),
            model: None,
            vision_provider: None,
            vision_model: None,
            thinking: None,
        };
        let error = router
            .chat_controlled(
                request,
                None,
                Some("codex-stream-policy-test"),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CallFailure>().unwrap();
        assert_eq!(failure.attempts, 1);
        assert_eq!(failure.terminal.kind, expected);
        assert!(failure.terminal.cause.is_some());
    }
}

#[tokio::test]
async fn codex_completion_ends_read_without_waiting_for_socket_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        socket.read(&mut request).await.unwrap();
        let body = wire(&[completed(
            json!([{"type":"message","content":[{"text":"done"}]}]),
        )]);
        let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n", body.len(), body);
        socket.write_all(headers.as_bytes()).await.unwrap();
        std::future::pending::<()>().await; // deliberately never EOF
    });
    let response = http::client()
        .get(format!("http://{address}"))
        .send()
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        receive(response, &|_| {}),
    )
    .await;
    server.abort();
    assert_eq!(
        result.unwrap().unwrap().response.content.as_deref(),
        Some("done")
    );
}

#[test]
fn usage_metrics_codex_partial_counts_are_unavailable() {
    for usage in [json!({}), json!({"output_tokens":5}), json!({"input_tokens":4294967296u64,"output_tokens":5})] {
        let mut value = completed(json!([{"type":"message","content":[{"type":"output_text","text":"ok"}]}]));
        value["response"]["usage"] = usage;
        let response = Accumulator::default().push(event(value), &|_| {}).unwrap().unwrap().response;
        assert!(response.usage.is_none(), "missing/overflowed counts must not become zero/clamped tokens: {:?}", response.usage);
    }
}
