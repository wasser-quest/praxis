//! Offline reproductions of Ollama Cloud exhausting num_predict on thinking.
//! No model, credentials, stored conversation or tool execution is involved.
use super::OllamaProvider;
use crate::gateway::llm::{provider::*, resilience::ResilienceConfig, LLMRouter};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage {
    reasoning_content: None,
            role: "user".into(),
            content: Some("synthetic task".into()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }],
        tools: Some(vec![ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: "get_context".into(),
                description: "Synthetic test tool; never executed".into(),
                parameters: json!({"type":"object","properties":{}}),
            },
        }]),
        max_tokens: Some(4096),
        temperature: Some(0.7),
        model: Some("synthetic-thinking-model".into()),
        vision_provider: None,
        vision_model: None,
        thinking: None,
    }
}

fn policy(attempts: &str, ceiling: &str) -> ResilienceConfig {
    ResilienceConfig::from_lookup(|key| {
        match key {
            "LLM_MAX_ATTEMPTS" => Some(attempts),
            "LLM_RETRY_MAX_OUTPUT_TOKENS" => Some(ceiling),
            "LLM_RETRY_BASE_MS" | "LLM_RETRY_MAX_MS" => Some("1"),
            "LLM_REQUEST_TIMEOUT_MS" => Some("2000"),
            "LLM_TOTAL_TIMEOUT_MS" => Some("5000"),
            _ => None,
        }
        .map(String::from)
    })
}

fn router(server: &MockServer, policy: ResilienceConfig) -> LLMRouter {
    let mut p = OllamaProvider::new(server.uri(), "unused-default".into(), None);
    p.client = reqwest::Client::builder().no_proxy().build().unwrap();
    LLMRouter::with_providers(vec![Box::new(p)], "ollama".into(), vec![], policy)
}

fn response(body: Value, streaming: bool) -> ResponseTemplate {
    if streaming {
        // Deliberately omit the final newline, as real cloud responses can do.
        ResponseTemplate::new(200).set_body_string(body.to_string())
    } else {
        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// A true num_predict cutoff: generation stops at the requested allowance, so
/// the output-limit recovery (larger allowance) is the correct move.
fn exhausted(num_predict: u64) -> Value {
    json!({
        "message":{"role":"assistant","content":"","thinking":"PRIVATE THINKING"},
        "done":true,"done_reason":"length","prompt_eval_count":100,"eval_count":num_predict
    })
}

/// Chat traffic only: the budget probe may ask `GET /api/tags` first.
async fn chat_requests(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/api/chat")
        .collect()
}

async fn mock_tags(server: &MockServer, context_length: u64) {
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [{"name": "synthetic-thinking-model", "details": {"context_length": context_length}}]
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn ollama_output_limit_recovers_thinking_only_on_both_paths() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let count = AtomicUsize::new(0);
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let predict = request.body_json::<Value>().unwrap()["options"]["num_predict"]
                    .as_u64()
                    .unwrap_or(4096);
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    exhausted(predict)
                } else {
                    json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop","prompt_eval_count":100,"eval_count":2})
                };
                response(body, streaming)
            }).expect(2).mount(&server).await;
        let r = router(&server, policy("5", "16384"));
        let user = format!("ollama-output-limit-{streaming}");
        let mut events = crate::runtime::events::get_or_create(&user).subscribe();
        let result = if streaming {
            r.streaming_chat(request(), None, &user).await
        } else {
            r.chat(request(), None).await
        }
        .unwrap();
        assert_eq!(result.content.as_deref(), Some("Ready"));
        assert_eq!(result.usage.unwrap().total_tokens, 102);
        let requests = chat_requests(&server).await;
        let mut first = requests[0].body_json::<Value>().unwrap();
        let second = requests[1].body_json::<Value>().unwrap();
        assert_eq!(first["options"]["num_predict"], 4096);
        assert_eq!(second["options"]["num_predict"], 8192);
        assert!(
            first.get("think").is_none(),
            "do not disable thinking or leak it as speech"
        );
        first["options"]["num_predict"] = json!(8192);
        assert_eq!(first, second, "only the output allowance may change");
        let mut assistant_events = 0;
        let mut chars = String::new();
        while let Ok(event) = events.try_recv() {
            assert!(!event.data.contains("PRIVATE"));
            assert_ne!(event.event, "stream_abort");
            if event.event == "char" {
                chars.push_str(&event.data);
            }
            if event.event == "assistant" {
                assistant_events += 1;
            }
        }
        if streaming {
            assert_eq!(chars, "Ready");
            assert_eq!(assistant_events, 1);
        }
    }
}

#[tokio::test]
async fn ollama_output_limit_does_not_commit_truncated_tool_batches() {
    let server = MockServer::start().await;
    let count = AtomicUsize::new(0);
    Mock::given(method("POST")).and(path("/api/chat"))
        .respond_with(move |_: &wiremock::Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_string(concat!(
                    "{\"message\":{\"tool_calls\":[{\"function\":{\"name\":\"get_context\",\"arguments\":{}}}]},\"done\":false}\n",
                    "{\"message\":{\"content\":\"\"},\"done\":true,\"done_reason\":\"length\"}\n"
                ))
            } else {
                response(json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop"}), true)
            }
        }).expect(2).mount(&server).await;
    let r = router(&server, policy("5", "16384"));
    let result = r
        .streaming_chat(request(), None, "ollama-output-tools")
        .await
        .unwrap();
    assert_eq!(result.content.as_deref(), Some("Ready"));
    assert!(
        result.tool_calls.is_none(),
        "no tools from a truncated attempt may escape"
    );
}

#[tokio::test]
async fn ollama_output_limit_never_replays_visible_partial_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(
            json!({"message":{"content":"partial"},"done":true,"done_reason":"length"}),
            true,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let user = "ollama-output-partial";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let error = router(&server, policy("5", "16384"))
        .streaming_chat(request(), None, user)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("partial stream"), "{error}");
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "assistant");
    }
}

#[tokio::test]
async fn ollama_output_limit_respects_attempt_cap_and_growth_ceiling() {
    for (attempts, ceiling, expected) in [
        ("1", "16384", vec![4096]),
        ("5", "0", vec![4096]),
        ("5", "10000", vec![4096, 8192, 10000]),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let predict = request.body_json::<Value>().unwrap()["options"]["num_predict"]
                    .as_u64()
                    .unwrap_or(4096);
                response(exhausted(predict), false)
            })
            .expect(expected.len() as u64)
            .mount(&server)
            .await;
        let error = router(&server, policy(attempts, ceiling))
            .chat(request(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("output token limit"), "{error}");
        assert!(!error.contains("PRIVATE"));
        let seen: Vec<_> = chat_requests(&server)
            .await
            .iter()
            .map(|r| {
                r.body_json::<Value>().unwrap()["options"]["num_predict"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(seen, expected);
    }
}

#[tokio::test]
async fn ollama_output_limit_reserves_the_larger_request_before_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response(exhausted(4096), false))
        .expect(1)
        .mount(&server)
        .await;
    let mut policy = policy("5", "16384");
    policy.tokens_per_minute = 6000; // first attempt fits; 8192 cannot be admitted
    let error = router(&server, policy)
        .chat(request(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("LLM_TOKENS_PER_MINUTE"), "{error}");
}

#[tokio::test]
async fn ollama_output_limit_does_not_blindly_retry_empty_or_malformed_responses() {
    for streaming in [false, true] {
        for body in [
            json!({"message":{"content":""},"done":true,"done_reason":"stop"}),
            json!({"done":true}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(response(body, streaming))
                .expect(1)
                .mount(&server)
                .await;
            let r = router(&server, policy("5", "16384"));
            let error = if streaming {
                r.streaming_chat(request(), None, "ollama-output-empty")
                    .await
            } else {
                r.chat(request(), None).await
            }
            .unwrap_err()
            .to_string();
            assert!(error.contains("invalid or empty"), "{error}");
            assert!(!error.contains("PRIVATE"));
        }
    }
}

#[tokio::test]
async fn ollama_think_only_plain_chat_surfaces_reasoning_as_content() {
    // Reasoning fallback without tools: a think-only response is usable output.
    for streaming in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(response(json!({"message":{"thinking":"MODEL REASONING"},"done":true}), streaming))
            .expect(1)
            .mount(&server)
            .await;
        let r = router(&server, policy("5", "16384"));
        let mut plain = request();
        plain.tools = None;
        let response = if streaming {
            r.streaming_chat(plain, None, "ollama-think-only").await
        } else {
            r.chat(plain, None).await
        }
        .unwrap();
        assert!(response.content.as_deref().is_some_and(|text| text.contains("MODEL REASONING") || text.contains("Denkspur")), "think text surfaced");
    }
}

#[tokio::test]
async fn ollama_think_only_with_tools_continues_instead_of_faking_an_answer() {
    // Codex parity: a reasoning step in a tool context is continued with an
    // explicit instruction, never dressed up as an assistant answer.
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let count = AtomicUsize::new(0);
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(move |_: &wiremock::Request| {
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    json!({"message":{"thinking":"PRIVATE THINKING"},"done":true,"done_reason":"stop"})
                } else {
                    json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop","prompt_eval_count":100,"eval_count":2})
                };
                response(body, streaming)
            })
            .expect(2)
            .mount(&server)
            .await;
        let r = router(&server, policy("5", "16384"));
        let user = format!("ollama-think-only-tools-{streaming}");
        let mut events = crate::runtime::events::get_or_create(&user).subscribe();
        let result = if streaming {
            r.streaming_chat(request(), None, &user).await
        } else {
            r.chat(request(), None).await
        }
        .unwrap();
        assert_eq!(result.content.as_deref(), Some("Ready"), "no faked [Denkspur] answer");
        let requests = chat_requests(&server).await;
        let replay = requests[1].body_json::<Value>().unwrap();
        let texts: Vec<&str> = replay["messages"].as_array().unwrap().iter().filter_map(|m| m["content"].as_str()).collect();
        assert!(texts.iter().any(|text| text.contains("PRIVATE THINKING")), "reasoning is replayed");
        assert!(texts.iter().any(|text| text.contains("Continue the original task")), "explicit continue instruction");
        while let Ok(event) = events.try_recv() {
            assert!(!event.data.contains("PRIVATE"), "thinking never leaks into events");
        }
    }
}

#[tokio::test]
async fn ollama_truncated_thinking_continues_instead_of_failing_the_turn() {
    // The production incident: done_reason=length at 23 of 4096 requested
    // tokens — the runtime window bound, not num_predict. The turn continues
    // with the preserved thinking instead of failing the whole message.
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let count = AtomicUsize::new(0);
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(move |_: &wiremock::Request| {
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    json!({"message":{"content":"","thinking":"PRIVATE THINKING"},"done":true,
                        "done_reason":"length","prompt_eval_count":8160,"eval_count":23})
                } else {
                    json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop",
                        "prompt_eval_count":100,"eval_count":2})
                };
                response(body, streaming)
            })
            .expect(2)
            .mount(&server)
            .await;
        let r = router(&server, policy("5", "16384"));
        let user = format!("ollama-truncated-thinking-{streaming}");
        let mut events = crate::runtime::events::get_or_create(&user).subscribe();
        let result = if streaming {
            r.streaming_chat(request(), None, &user).await
        } else {
            r.chat(request(), None).await
        }
        .unwrap();
        assert_eq!(result.content.as_deref(), Some("Ready"));
        assert_eq!(chat_requests(&server).await.len(), 2, "one continuation attempt");
        while let Ok(event) = events.try_recv() {
            assert!(!event.data.contains("PRIVATE"), "thinking never leaks into events");
        }
    }
}

#[tokio::test]
async fn ollama_starved_cutoff_names_the_window_not_the_output_budget() {
    // No thinking to continue from: fail with the real constraint named, so
    // "increase LLM_RETRY_MAX_OUTPUT_TOKENS" is never suggested for a window
    // problem.
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/api/chat"))
        .respond_with(response(json!({"message":{"content":""},"done":true,"done_reason":"length",
            "prompt_eval_count":8160,"eval_count":23}), false))
        .expect(1)
        .mount(&server)
        .await;
    let error = router(&server, policy("1", "16384"))
        .chat(request(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("output token limit"), "{error}");
    assert!(error.contains("OLLAMA_NUM_CTX"), "{error}");
}

#[tokio::test]
async fn ollama_sets_num_ctx_from_the_probed_model_window() {
    let server = MockServer::start().await;
    mock_tags(&server, 32_768).await;
    Mock::given(method("POST")).and(path("/api/chat"))
        .respond_with(response(json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop"}), false))
        .expect(1)
        .mount(&server)
        .await;
    router(&server, policy("1", "16384")).chat(request(), None).await.unwrap();
    let body = chat_requests(&server).await[0].body_json::<Value>().unwrap();
    assert_eq!(body["options"]["num_ctx"], 32_768, "runtime window set explicitly");
    assert_eq!(body["options"]["num_predict"], 4096, "requested output honored below the cap");
}

#[tokio::test]
async fn ollama_num_ctx_clamps_to_model_window_and_reserves_prompt_half() {
    let server = MockServer::start().await;
    mock_tags(&server, 32_768).await;
    Mock::given(method("POST")).and(path("/api/chat"))
        .respond_with(response(json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop"}), false))
        .expect(1)
        .mount(&server)
        .await;
    let mut p = OllamaProvider::new(server.uri(), "unused-default".into(), None).num_ctx(Some(65_536));
    p.client = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = LLMRouter::with_providers(vec![Box::new(p)], "ollama".into(), vec![], policy("1", "16384"));
    let mut big = request();
    big.max_tokens = Some(32_768);
    r.chat(big, None).await.unwrap();
    let body = chat_requests(&server).await[0].body_json::<Value>().unwrap();
    assert_eq!(body["options"]["num_ctx"], 32_768, "clamped to the model context");
    assert_eq!(body["options"]["num_predict"], 16_384, "output keeps at most half the window");
}

#[tokio::test]
async fn ollama_budget_previews_saved_tool_outputs_that_do_not_fit() {
    // Durable tool snapshots keep their read_tool_result handle; oversized
    // bodies become previews instead of pushing the turn over the window.
    let server = MockServer::start().await;
    mock_tags(&server, 32_768).await;
    Mock::given(method("POST")).and(path("/api/chat"))
        .respond_with(response(json!({"message":{"content":"Ready"},"done":true,"done_reason":"stop"}), false))
        .expect(1)
        .mount(&server)
        .await;
    let source = format!(
        "{}\n[Saved tool response] output_id=out_00000000000000000000000000000000",
        "large 漢🙂 ".repeat(20_000)
    );
    let mut heavy = request();
    heavy.messages.push(ChatMessage {
        role: "tool".into(),
        content: Some(source),
        reasoning_content: None,
        content_parts: None,
        tool_calls: None,
        tool_call_id: Some("call_1".into()),
        tool_name: Some("get_context".into()),
    });
    router(&server, policy("1", "16384")).chat(heavy, None).await.unwrap();
    let body = chat_requests(&server).await[0].body_json::<Value>().unwrap();
    let texts: Vec<&str> = body["messages"].as_array().unwrap().iter().filter_map(|m| m["content"].as_str()).collect();
    assert!(texts.iter().any(|text| text.contains("context_preview")), "oversized saved output previewed");
    assert!(texts.iter().any(|text| text.contains("out_00000000000000000000000000000000")), "durable handle preserved");
}

#[tokio::test]
async fn ollama_missing_model_is_a_configuration_error_with_operator_action() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(response(json!({"error":"model 'synthetic-thinking-model' not found, try pulling it"}), streaming))
            .expect(1)
            .mount(&server)
            .await;
        let error = router(&server, policy("1", "16384")).chat(request(), None).await.unwrap_err().to_string();
        assert!(error.contains("check OLLAMA_MODEL or settings.model"), "{error}");
    }
}
