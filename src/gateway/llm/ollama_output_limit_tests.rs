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

fn exhausted() -> Value {
    json!({
        "message":{"role":"assistant","content":"","thinking":"PRIVATE THINKING"},
        "done":true,"done_reason":"length","prompt_eval_count":100,"eval_count":4096
    })
}

#[tokio::test]
async fn ollama_output_limit_recovers_thinking_only_on_both_paths() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        let count = AtomicUsize::new(0);
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(move |_: &wiremock::Request| {
                let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    exhausted()
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
        let requests = server.received_requests().await.unwrap();
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
            .respond_with(response(exhausted(), false))
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
        let seen: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
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
        .respond_with(response(exhausted(), false))
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
async fn ollama_think_only_response_surfaces_reasoning_as_content() {
    // Reasoning fallback: a think-only response is usable output, not an error.
    for streaming in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response(json!({"message":{"thinking":"MODEL REASONING"},"done":true}), streaming))
            .expect(1)
            .mount(&server)
            .await;
        let r = router(&server, policy("5", "16384"));
        let response = if streaming {
            r.streaming_chat(request(), None, "ollama-think-only").await
        } else {
            r.chat(request(), None).await
        }
        .unwrap();
        assert!(response.content.as_deref().is_some_and(|text| text.contains("MODEL REASONING") || text.contains("Denkspur")), "think text surfaced");
    }
}
