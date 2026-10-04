use super::*;
use crate::gateway::{
    llm::{error::CallFailure, resilience::ResilienceConfig, LLMRouter},
    task_control,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn reasoning(index: usize) -> Value {
    json!({
        "type":"reasoning", "id":format!("rs_{index}"),
        "summary":[{"type":"summary_text", "text":format!("[headline]Schritt {index} 日本🙂[/headline]")}],
        "encrypted_content":format!("OPAQUE-SECRET-{index}")
    })
}

fn completed(output: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "type":"response.completed",
            "response":{"status":"completed", "output":output,
                "usage":{"input_tokens":7,"output_tokens":5,"total_tokens":12}}
        })
    )
}

fn router(server: &MockServer, policy: ResilienceConfig) -> LLMRouter {
    LLMRouter::with_providers(
        vec![Box::new(provider(server))],
        "codex".into(),
        vec![],
        policy,
    )
}

#[tokio::test]
async fn codex_auto_requests_summaries_and_replayable_reasoning_without_forcing_effort() {
    let server = MockServer::start().await;
    let provider = provider(&server);
    let body = provider.build_body(&request());
    assert_eq!(body["reasoning"], json!({"summary":"auto"}));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["store"], false);
    for (mode, effort) in [
        (ThinkingMode::Off, "low"),
        (ThinkingMode::High, "high"),
        (ThinkingMode::Xhigh, "high"),
    ] {
        let mut req = request();
        req.thinking = Some(mode);
        let body = provider.build_body(&req);
        assert_eq!(body["reasoning"], json!({"summary":"auto","effort":effort}));
    }
}

#[tokio::test]
async fn codex_reasoning_continues_to_answer_or_tools_with_opt_in_headlines() {
    for tool in [false, true] {
        for visible in [None, Some(false), Some(true)] {
            let server = MockServer::start().await;
            Mock::given(path("/responses"))
                .respond_with(move |req: &wiremock::Request| {
                    let body: Value = req.body_json().unwrap();
                    let step = body["input"].as_array().unwrap().iter()
                        .filter(|item| item["type"] == "reasoning").count();
                    let text = if step < 2 {
                        let item = reasoning(step);
                        // First step streamed; second only in terminal output.
                        let delta = if step == 0 {
                            format!("data: {}\n\n", json!({"type":"response.reasoning_summary_text.delta",
                                "delta":item["summary"][0]["text"]}))
                        } else { String::new() };
                        delta + &completed(json!([item]))
                    } else if tool {
                        completed(json!([{"type":"function_call", "status":"completed",
                            "call_id":"call_1", "name":"read_file", "arguments":"{\"path\":\"a\"}"}]))
                    } else {
                        completed(json!([{"type":"message", "content":[{"type":"output_text", "text":"Fertig"}]}]))
                    };
                    ResponseTemplate::new(200).set_body_raw(text, "text/event-stream")
                })
                .expect(6) // Two independent logical calls, three attempts each.
                .mount(&server).await;
            let router = router(
                &server,
                ResilienceConfig {
                    max_attempts: 3,
                    ..Default::default()
                },
            );
            let user = format!("codex-continuation-{tool}-{visible:?}");
            let _guard = task_control::begin(&user).unwrap();
            task_control::set_show_thinking(&user, visible == Some(true));
            let mut events = crate::runtime::events::get_or_create(&user).subscribe();
            let mut req = request();
            req.tools = Some(vec![ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "read_file".into(),
                    description: "Read".into(),
                    parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
                },
            }]);
            req.max_tokens = Some(100);
            let user_id = visible.map(|_| user.as_str());
            for _ in 0..2 {
                let response = router
                    .chat_controlled(req.clone(), None, user_id, &CancellationToken::new())
                    .await
                    .unwrap();
                assert_eq!(
                    response.content.as_deref(),
                    if tool { None } else { Some("Fertig") }
                );
                assert_eq!(
                    response.tool_calls.as_ref().map_or(0, Vec::len),
                    usize::from(tool)
                );
                assert_eq!(
                    response.finish_reason.as_deref(),
                    Some(if tool { "tool_calls" } else { "stop" })
                );
                if tool {
                    assert_eq!(
                        response.tool_calls.as_ref().unwrap()[0].function.arguments,
                        "{\"path\":\"a\"}"
                    );
                }
                assert!(!serde_json::to_string(&response)
                    .unwrap()
                    .contains("OPAQUE-SECRET"));
                assert_eq!(response.usage.unwrap().total_tokens, 36);
            }
            let mut summaries = Vec::new();
            let mut assistants = Vec::new();
            let mut starts = 0;
            let mut ends = 0;
            while let Ok(event) = events.try_recv() {
                assert_ne!(
                    event.event, "stream_abort",
                    "a thinking step must not abort the task"
                );
                assert!(!event.data.contains("OPAQUE-SECRET"));
                if visible != Some(true) {
                    assert!(!event.event.starts_with("reasoning"));
                }
                match event.event.as_str() {
                    "reasoning" => summaries.push(event.data),
                    "assistant" => assistants.push(event.data),
                    "stream_start" => starts += 1,
                    "stream_end" => ends += 1,
                    _ => {}
                }
            }
            assert_eq!(starts, if visible.is_some() { 6 } else { 0 });
            assert_eq!(ends, starts);
            assert_eq!(
                assistants,
                if visible.is_some() && !tool {
                    vec!["Fertig", "Fertig"]
                } else {
                    vec![]
                }
            );
            assert_eq!(summaries.len(), if visible == Some(true) { 4 } else { 0 });
            for (i, summary) in summaries.iter().enumerate() {
                assert_eq!(
                    summary,
                    reasoning(i % 2)["summary"][0]["text"].as_str().unwrap()
                );
            }
            let requests = server.received_requests().await.unwrap();
            let bodies: Vec<Value> = requests.iter().map(|r| r.body_json().unwrap()).collect();
            for (index, body) in bodies.iter().enumerate() {
                let step = index % 3;
                let input = body["input"].as_array().unwrap();
                assert_eq!(input.len(), 1 + step * 2);
                assert_eq!(input[0]["content"][0]["text"], "Say hello");
                for previous in 0..step {
                    assert_eq!(input[1 + previous * 2], reasoning(previous));
                    assert_eq!(input[2 + previous * 2]["role"], "developer");
                }
                assert_eq!(body["tools"][0]["name"], "read_file");
                assert_eq!(body["reasoning"], bodies[0]["reasoning"]);
                assert!(
                    body.get("previous_response_id").is_none(),
                    "store=false requires stateless replay"
                );
                assert!(body.get("max_output_tokens").is_none());
            }
        }
    }
}

#[tokio::test]
async fn codex_endless_reasoning_is_bounded_and_never_a_false_final_answer() {
    let server = MockServer::start().await;
    Mock::given(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(completed(json!([reasoning(0)])), "text/event-stream"),
        )
        .expect(2)
        .mount(&server)
        .await;
    let router = router(
        &server,
        ResilienceConfig {
            max_attempts: 2,
            ..Default::default()
        },
    );
    let error = router.chat(request(), None).await.unwrap_err();
    let failure = error.downcast_ref::<CallFailure>().unwrap();
    assert_eq!(failure.attempts, 2);
    assert_eq!(failure.terminal.kind, ErrorKind::ReasoningOnly);
    assert!(error.to_string().contains("attempt limit"));
    assert!(!format!("{error:?}").contains("OPAQUE-SECRET"));
    assert!(!format!("{error:?}").contains("[headline]"));
}

#[tokio::test]
async fn codex_reasoning_continuations_obey_pacing_deadline_and_cancellation() {
    for cancel_task in [false, true] {
        let server = MockServer::start().await;
        Mock::given(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(completed(json!([reasoning(0)])), "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let router = router(
            &server,
            ResilienceConfig {
                requests_per_minute: 1,
                total_timeout_ms: 1000,
                ..Default::default()
            },
        );
        let user = format!("codex-continuation-cancel-{cancel_task}");
        let mut events = crate::runtime::events::get_or_create(&user).subscribe();
        let cancel = CancellationToken::new();
        let (result, ()) = tokio::join!(
            router.chat_controlled(request(), None, Some(&user), &cancel),
            async {
                if cancel_task {
                    tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        while let Ok(event) = events.recv().await {
                            if event.event == "feedback"
                                && event.data.contains("reasoning step completed")
                            {
                                cancel.cancel();
                                break;
                            }
                        }
                    })
                    .await
                    .unwrap();
                }
            }
        );
        let error = result.unwrap_err();
        let failure = error.downcast_ref::<CallFailure>().unwrap();
        assert_eq!(failure.attempts, 1);
        assert_eq!(
            failure.terminal.kind,
            if cancel_task {
                ErrorKind::Cancelled
            } else {
                ErrorKind::Deadline
            }
        );
    }
}

#[tokio::test]
async fn codex_continuation_state_counts_toward_token_admission() {
    let server = MockServer::start().await;
    let mut item = reasoning(0);
    item["encrypted_content"] = "opaque".repeat(1000).into();
    Mock::given(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(completed(json!([item])), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let router = router(
        &server,
        ResilienceConfig {
            tokens_per_minute: 100,
            ..Default::default()
        },
    );
    let mut req = request();
    req.max_tokens = Some(1);
    let error = router.chat(req, None).await.unwrap_err();
    let failure = error.downcast_ref::<CallFailure>().unwrap();
    assert_eq!(failure.attempts, 1);
    assert_eq!(failure.terminal.kind, ErrorKind::TokenBudget);
}

#[tokio::test]
async fn codex_reasoning_does_not_consume_tool_turns_or_pollute_tool_history() {
    let server = MockServer::start().await;
    Mock::given(path("/responses"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = req.body_json().unwrap();
            let input = body["input"].as_array().unwrap();
            let output = if input
                .iter()
                .any(|item| item["type"] == "function_call_output")
            {
                assert!(!body.to_string().contains("OPAQUE-SECRET"));
                assert!(!body.to_string().contains("[headline]"));
                assert!(!input.iter().any(|item| item["type"] == "reasoning"));
                json!([{"type":"message", "content":[{"text":"Fertig"}]}])
            } else if input.iter().any(|item| item["type"] == "reasoning") {
                json!([{"type":"function_call", "call_id":"feedback_1", "name":"agent_feedback",
                    "arguments":"{\"message\":\"Exactly once\"}"}])
            } else {
                json!([reasoning(0)])
            };
            ResponseTemplate::new(200).set_body_raw(completed(output), "text/event-stream")
        })
        .expect(3)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let tools = vec![ToolDefinition {
        tool_type: "function".into(),
        function: FunctionDefinition {
            name: "agent_feedback".into(),
            description: "Feedback".into(),
            parameters: json!({"type":"object","properties":{"message":{"type":"string"}}}),
        },
    }];
    let result = router(&server, Default::default())
        .chat_with_tools(
            &db,
            "codex-reasoning-tool-loop",
            request().messages,
            tools,
            Some(2),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.response, "Fertig");
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.feedback_messages, vec!["Exactly once"]);
}

#[tokio::test]
async fn codex_continuation_still_rejects_malformed_tool_batches() {
    let server = MockServer::start().await;
    Mock::given(path("/responses"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = req.body_json().unwrap();
            let output = if body["input"].as_array().unwrap().len() == 1 {
                json!([reasoning(0)])
            } else {
                json!([{"type":"function_call", "call_id":"bad", "name":"write_file", "arguments":"{SECRET"}])
            };
            ResponseTemplate::new(200).set_body_raw(completed(output), "text/event-stream")
        })
        .expect(2).mount(&server).await;
    let error = router(&server, Default::default())
        .chat(request(), None)
        .await
        .unwrap_err();
    let failure = error.downcast_ref::<CallFailure>().unwrap();
    assert_eq!(failure.attempts, 2);
    assert_eq!(failure.terminal.kind, ErrorKind::InvalidResponse);
    assert!(!format!("{error:?}").contains("SECRET"));
}
