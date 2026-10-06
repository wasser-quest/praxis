//! Real local HTTP contracts for the actual adapters, not just mock traits.
use praxis_provider_api::EmbeddingProvider as _;
use super::{provider::*, resilience::ResilienceConfig, LLMRouter};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![],
        tools: None,
        temperature: None,
        max_tokens: Some(10),
        model: None,
        vision_provider: None,
        vision_model: None,
        thinking: None,
    }
}
fn policy() -> ResilienceConfig {
    ResilienceConfig {
        max_attempts: 2,
        initial_backoff_ms: 1,
        max_backoff_ms: 1,
        request_timeout_ms: 2000,
        total_timeout_ms: 5000,
        ..Default::default()
    }
}
fn adapter(name: &str, base: String) -> (Box<dyn LLMProvider>, &'static str, serde_json::Value) {
    let key = "local-test-only".to_string();
    let model = "test-model".to_string();
    let openai =
        serde_json::json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]});
    let anthropic =
        serde_json::json!({"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"});
    match name {
        "openai" => (
            Box::new(super::openai::OpenAIProvider::new(key, model, base)),
            "/chat/completions",
            openai,
        ),
        "openrouter" => (
            Box::new(super::openrouter::OpenRouterProvider::new(key, model, base)),
            "/chat/completions",
            openai,
        ),
        "llamacpp" => (
            Box::new(super::llamacpp::LlamaCppProvider::new(
                Some(key),
                model,
                base,
            )),
            "/v1/chat/completions",
            openai,
        ),
        "free-router" => (
            Box::new(super::free_router::FreeRouterProvider::new(Some(key), model, base)),
            "/free/v1/chat/completions",
            openai,
        ),
        "anthropic" => (
            Box::new(super::anthropic::AnthropicProvider::new(key, model, base)),
            "/v1/messages",
            anthropic,
        ),
        "ollama" => (
            Box::new(super::ollama::OllamaProvider::new(base, model, Some(key))),
            "/api/chat",
            serde_json::json!({"message":{"content":"ok"},"done":true,"prompt_eval_count":5,"eval_count":2}),
        ),
        "minimax" => (
            Box::new(super::minimax::MiniMaxProvider::new(
                key,
                model,
                base,
                crate::config::ApiMode::OpenAI,
            )),
            "/text/chatcompletion_v2",
            openai,
        ),
        "mimo" => (
            Box::new(super::mimo::MiMoProvider::new(
                key,
                model,
                base,
                crate::config::ApiMode::OpenAI,
            )),
            "/chat/completions",
            openai,
        ),
        "minimax-anthropic" => (
            Box::new(super::minimax::MiniMaxProvider::new(
                key,
                model,
                base,
                crate::config::ApiMode::Anthropic,
            )),
            "/v1/messages",
            anthropic,
        ),
        "mimo-anthropic" => (
            Box::new(super::mimo::MiMoProvider::new(
                key,
                model,
                base,
                crate::config::ApiMode::Anthropic,
            )),
            "/v1/messages",
            anthropic,
        ),
        _ => panic!("test adapter"),
    }
}
const ADAPTERS: &[&str] = &[
    "openai",
    "openrouter",
    "anthropic",
    "ollama",
    "llamacpp",
    "minimax",
    "mimo",
    "minimax-anthropic",
    "mimo-anthropic",
];

#[tokio::test]
async fn branding_http_adapters_identify_praxis_without_changing_model_or_credentials() {
    for name in ADAPTERS.iter().copied().chain(["free-router"]) {
        let server = MockServer::start().await;
        let (provider, endpoint, response) = adapter(name, server.uri());
        Mock::given(method("POST")).and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1).mount(&server).await;
        provider.chat(request()).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let sent = requests.iter().find(|sent| sent.method == "POST" && sent.url.path() == endpoint).unwrap();
        assert_eq!(sent.headers.get("user-agent").unwrap().to_str().unwrap(),
            concat!("Praxis/", env!("CARGO_PKG_VERSION"), " (+https://getpraxis.boo)"), "{name}");
        assert_eq!(sent.body_json::<serde_json::Value>().unwrap()["model"], "test-model");
        if name == "openrouter" {
            assert_eq!(sent.headers["http-referer"], "https://getpraxis.boo");
            assert_eq!(sent.headers["x-openrouter-title"], "Praxis");
            assert_eq!(sent.headers["authorization"], "Bearer local-test-only");
        } else {
            assert!(!sent.headers.contains_key("http-referer"), "{name}");
            assert!(!sent.headers.contains_key("x-openrouter-title"), "{name}");
        }
    }
}

#[tokio::test]
async fn branding_openrouter_model_requests_use_the_same_app_attribution() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[]})))
        .expect(1).mount(&server).await;
    let (provider, _, _) = adapter("openrouter", server.uri());
    assert!(provider.health_check().await);
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent[0].headers["http-referer"], "https://getpraxis.boo");
    assert_eq!(sent[0].headers["x-openrouter-title"], "Praxis");
}

#[tokio::test]
async fn branding_embedding_clients_identify_praxis_without_openrouter_headers() {
    use super::embeddings::{EmbeddingConfig, EmbeddingProvider};
    for (name, endpoint, response) in [
        ("openai", "/embeddings", serde_json::json!({"data":[{"embedding":[0.1,0.2]}]})),
        ("ollama", "/api/embeddings", serde_json::json!({"embedding":[0.1,0.2]})),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1).mount(&server).await;
        let provider = EmbeddingProvider::new(EmbeddingConfig {
            provider: name.into(), model: "test-embedding".into(), base_url: server.uri(), api_key: Some("local-test-only".into()),
        });
        assert_eq!(provider.embed("synthetic input").await.unwrap().len(), 2);
        let sent = server.received_requests().await.unwrap();
        assert_eq!(sent[0].headers["user-agent"], concat!("Praxis/", env!("CARGO_PKG_VERSION"), " (+https://getpraxis.boo)"));
        assert!(!sent[0].headers.contains_key("http-referer"));
    }
}

#[tokio::test]
async fn resilience_http_all_adapters_retry_429_and_keep_request_identical() {
    for name in ADAPTERS {
        let server = MockServer::start().await;
        let (p, endpoint, response) = adapter(name, server.uri());
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(move |_: &wiremock::Request| {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(429)
                        .insert_header("Retry-After", "0")
                        .set_body_json(serde_json::json!({"error":{"message":"PRIVATE PROMPT"}}))
                } else {
                    ResponseTemplate::new(200).set_body_json(response.clone())
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let provider = p.name().to_string();
        let r = LLMRouter::with_providers(vec![p], provider, vec![], policy());
        assert_eq!(
            r.chat(request(), None).await.unwrap().content.as_deref(),
            Some("ok"),
            "{name}"
        );
        let requests: Vec<_> = server.received_requests().await.unwrap().into_iter()
            .filter(|r| r.method == "POST").collect();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, requests[1].body);
        if *name == "ollama" {
            assert_eq!(
                requests[0].body_json::<serde_json::Value>().unwrap()["options"]["num_predict"],
                10
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn resilience_http_all_adapters_reject_permanent_and_empty_responses() {
    for name in ADAPTERS {
        for (status, body) in [
            (400, serde_json::json!({"error":"PRIVATE PROMPT"})),
            (401, serde_json::json!({"error":"PRIVATE KEY"})),
            (200, serde_json::json!({})),
        ] {
            let server = MockServer::start().await;
            let (p, endpoint, _) = adapter(name, server.uri());
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let provider = p.name().to_string();
            let r = LLMRouter::with_providers(vec![p], provider, vec![], policy());
            let e = r.chat(request(), None).await.unwrap_err().to_string();
            assert!(!e.contains("PRIVATE"), "{e}");
            assert_eq!(server.received_requests().await.unwrap().iter().filter(|r|r.method == "POST").count(), 1, "{name}");
        }
    }
}

#[tokio::test]
async fn resilience_http_embedded_openrouter_error_is_not_empty_success() {
    let server = MockServer::start().await;
    let (p, endpoint, response) = adapter("openrouter", server.uri());
    let count = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path(endpoint))
        .respond_with(move |_: &wiremock::Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"error":{"code":503,"message":"PRIVATE"}}))
            } else {
                ResponseTemplate::new(200).set_body_json(response.clone())
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let r = LLMRouter::with_providers(vec![p], "openrouter".into(), vec![], policy());
    assert_eq!(
        r.chat(request(), None).await.unwrap().content.as_deref(),
        Some("ok")
    );
}

#[tokio::test]
async fn resilience_http_preserves_safe_diagnostics_and_quota_does_not_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(429)
        .insert_header("Retry-After", "17").insert_header("x-request-id", "synthetic-request-1")
        .set_body_json(serde_json::json!({"error":{"code":"insufficient_quota","message":"SECRET-TOKEN"}})))
        .expect(1).mount(&server).await;
    let (p, _, _) = adapter("openai", server.uri());
    let r = LLMRouter::with_providers(vec![p], "openai".into(), vec![], policy());
    let e = r.chat(request(), None).await.unwrap_err().to_string();
    for expected in ["quota", "HTTP 429", "17s", "synthetic-request-1"] {
        assert!(e.contains(expected), "{e}");
    }
    assert!(!e.contains("SECRET"));
}

#[tokio::test]
async fn resilience_http_ollama_stream_retries_before_output_and_parses_usage() {
    let server = MockServer::start().await;
    let count = AtomicUsize::new(0);
    Mock::given(method("POST")).and(path("/api/chat")).respond_with(move |request: &wiremock::Request| {
        let body = request.body_json::<serde_json::Value>().unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["options"]["num_predict"], 10);
        if count.fetch_add(1, Ordering::SeqCst) == 0 { ResponseTemplate::new(503) }
        else { ResponseTemplate::new(200).set_body_string("{\"message\":{\"content\":\"日本語\"},\"done\":false}\n{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n") }
    }).expect(2).mount(&server).await;
    let (p, _, _) = adapter("ollama", server.uri());
    let r = LLMRouter::with_providers(vec![p], "ollama".into(), vec![], policy());
    let mut events = crate::runtime::events::get_or_create("resilience-http-stream").subscribe();
    let response = r
        .streaming_chat(request(), None, "resilience-http-stream")
        .await
        .unwrap();
    assert_eq!(response.content.as_deref(), Some("日本語"));
    assert_eq!(response.usage.unwrap().total_tokens, 7);
    while let Ok(event) = events.try_recv() {
        assert_ne!(
            event.event, "stream_abort",
            "pre-output retry must not stop the task timer"
        );
    }
}

#[tokio::test]
async fn resilience_http_ollama_partial_stream_never_retries_or_emits_final() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("{\"message\":{\"content\":\"partial\"},\"done\":false}\n"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (p, _, _) = adapter("ollama", server.uri());
    let r = LLMRouter::with_providers(vec![p], "ollama".into(), vec![], policy());
    let user = "resilience-http-partial";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    assert!(r
        .streaming_chat(request(), None, user)
        .await
        .unwrap_err()
        .to_string()
        .contains("partial"));
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "assistant");
    }
}

#[tokio::test]
async fn resilience_transport_failure_diagnostics_never_expose_url_credentials() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (p, _, _) = adapter(
        "openai",
        format!("http://fake-user:SECRET-certificate-TOKEN@127.0.0.1:{port}"),
    );
    let r = LLMRouter::with_providers(vec![p], "openai".into(), vec![], policy());
    let e = r.chat(request(), None).await.unwrap_err().to_string();
    assert!(
        e.contains("transport failure") && e.contains("connection refused"),
        "{e}"
    );
    assert!(e.contains("2 attempt(s)"), "{e}");
    for private in ["SECRET-certificate-TOKEN", "fake-user", "127.0.0.1"] {
        assert!(!e.contains(private), "{e}");
    }
}

#[tokio::test]
async fn llamacpp_reasoning_cutoff_is_visible_but_never_a_final_answer() {
    // Provider reasoning is visible when opted in, including cutoff attempts.
    // It must not become a false final answer or an endlessly retried response.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(format!("data: {}\n\ndata: [DONE]\n\n", serde_json::json!({
                "choices":[{"index":0,"delta":{"reasoning_content":"step-by-step reasoning that exhausted the budget"},"finish_reason":"length"}],
                "usage":{"prompt_tokens":7,"completion_tokens":4096,"total_tokens":4103}
            }))))
        .expect(2)
        .mount(&server)
        .await;
    let (p, _, _) = adapter("llamacpp", server.uri());
    let r = LLMRouter::with_providers(vec![p], "llamacpp".into(), vec![], policy());
    let user = "llamacpp-reasoning-only";
    let _guard = crate::gateway::task_control::begin(user).unwrap();
    crate::gateway::task_control::set_show_thinking(user, true);
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let error = r.streaming_chat(request(), None, user).await.unwrap_err();
    assert!(error.to_string().contains("output token limit"), "{error}");
    let mut reasoning = 0;
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "assistant");
        if event.event == "reasoning" {
            assert_eq!(event.data, "step-by-step reasoning that exhausted the budget");
            reasoning += 1;
        }
    }
    assert_eq!(reasoning, 2);
}

/// Reproduces the reported production failure: a reasoning model spends ~130 s
/// and the whole max_tokens budget, then starts a tool call that gets cut off
/// mid-arguments (finish_reason=length, arguments='{"command":"gr'). The old
/// code raised a non-retryable InvalidResponse and dropped the turn. The fix
/// classifies it as OutputLimit so the router retries with a larger allowance.
#[tokio::test]
async fn llamacpp_truncated_tool_call_retries_with_larger_output_budget() {
    let server = MockServer::start().await;
    let seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Default::default();
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_in = seen.clone();
    let hits_in = hits.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
            seen_in.lock().unwrap().push(body);
            let first = hits_in.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "trunc1",
                                "function": {"name": "execute_terminal", "arguments": "{\"command\":\"gr"}
                            }]
                        },
                        "finish_reason": "length"
                    }],
                    "usage": {"completion_tokens": 4096}
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "ok1",
                                "function": {"name": "execute_terminal", "arguments": "{\"command\":\"grep x\"}"}
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {"completion_tokens": 30}
                }))
            }
        })
        .mount(&server)
        .await;
    let (p, _, _) = adapter("llamacpp", server.uri());
    let r = LLMRouter::with_providers(vec![p], "llamacpp".into(), vec![], policy());
    let response = r.chat(request(), None).await.expect("must retry, not fail");
    assert_eq!(response.tool_calls.as_ref().map(Vec::len), Some(1));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "truncation must trigger exactly one retry");
    assert_eq!(seen[0]["max_tokens"], serde_json::json!(10));
    assert_eq!(
        seen[1]["max_tokens"],
        serde_json::json!(20),
        "retry must double the output allowance"
    );
}

#[tokio::test]
async fn llamacpp_empty_tool_call_fields_are_repaired_not_rejected() {
    // llama.cpp omits the id and emits "" for no-argument tools; both used to
    // fail the whole response as InvalidResponse.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "",
                        "function": {"name": "agent_complete", "arguments": ""}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })))
        .mount(&server)
        .await;
    let (p, _, _) = adapter("llamacpp", server.uri());
    let r = LLMRouter::with_providers(vec![p], "llamacpp".into(), vec![], policy());
    let response = r.chat(request(), None).await.expect("must repair");
    let calls = response.tool_calls.expect("call must survive");
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].id.is_empty(), "empty id must be generated");
    assert_eq!(calls[0].function.arguments, "{}");
}

#[tokio::test]
async fn llamacpp_malformed_tool_call_without_cutoff_stays_invalid() {
    // Broken arguments with finish_reason=stop is a genuine provider bug:
    // no retry, no expansion, stays InvalidResponse.
    let server = MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_in = hits.clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_req: &wiremock::Request| {
            hits_in.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": "bad1",
                            "function": {"name": "x", "arguments": "{not json at all"}
                        }]
                    },
                    "finish_reason": "stop"
                }]
            }))
        })
        .mount(&server)
        .await;
    let (p, _, _) = adapter("llamacpp", server.uri());
    let r = LLMRouter::with_providers(vec![p], "llamacpp".into(), vec![], policy());
    let e = r.chat(request(), None).await.unwrap_err().to_string();
    assert!(e.contains("invalid or empty provider response"), "{e}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "must not retry");
}

#[test]
fn resilience_router_rejects_empty_keys_and_shares_same_account_gates() {
    let mut config = crate::config::Config::from_env();
    config.use_provider = "openai".into();
    config.llm_resilience = policy();
    config.llm_fallback_providers = vec![];
    let mut secrets = crate::db::secrets::Secrets {
        openai_api_key: Some("  ".into()),
        ..Default::default()
    };
    assert!(LLMRouter::new(&config, &secrets)
        .validate_configuration()
        .is_err());
    config.openai_api_base = "http://127.0.0.1:9/v1".into();
    config.openrouter_api_base = "http://127.0.0.1:9/another/path".into();
    secrets.openai_api_key = Some("synthetic-account".into());
    secrets.openrouter_api_key = secrets.openai_api_key.clone();
    let r = LLMRouter::new(&config, &secrets);
    assert!(Arc::ptr_eq(&r.gates["openai"], &r.gates["openrouter"]));
    secrets.openrouter_api_key = Some("separate-account".into());
    let r = LLMRouter::new(&config, &secrets);
    assert!(!Arc::ptr_eq(&r.gates["openai"], &r.gates["openrouter"]));
}

// Regression for issue #4: missing/invalid provider counters are not zero tokens.
// Exercise actual wire adapters, not a second copy of their parsing logic.
#[tokio::test]
async fn usage_metrics_http_requires_complete_exact_counters() {
    use serde_json::json;
    let mut failures = Vec::new();
    for name in ADAPTERS.iter().copied().chain(["free_router"]) {
        let server = MockServer::start().await;
        let (p, endpoint, template): (Box<dyn LLMProvider>, _, _) = if name == "free_router" {
            (Box::new(super::free_router::FreeRouterProvider::new(None, "test".into(), server.uri())),
                "/free/v1/chat/completions", json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}))
        } else { adapter(name, server.uri()) };
        let anthropic = name.contains("anthropic");
        let (input, output) = if anthropic { ("input_tokens", "output_tokens") }
            else if name == "ollama" { ("prompt_eval_count", "eval_count") }
            else { ("prompt_tokens", "completion_tokens") };
        for (case, counters, expected) in [
            ("missing", serde_json::Value::Null, serde_json::Value::Null),
            ("empty", json!({}), serde_json::Value::Null),
            ("array", json!([]), serde_json::Value::Null),
            ("fraction", json!({input:7.5,output:5}), serde_json::Value::Null),
            ("boolean", json!({input:true,output:5}), serde_json::Value::Null),
            ("partial", json!({output:5}), serde_json::Value::Null),
            ("negative", json!({input:-1,output:5}), serde_json::Value::Null),
            ("string", json!({input:"7",output:5}), serde_json::Value::Null),
            ("overflow", json!({input:4294967296u64,output:5}), serde_json::Value::Null),
            ("sum_overflow", json!({input:4294967295u64,output:1}), serde_json::Value::Null),
            ("zero", json!({input:0,output:0}), json!({"prompt_tokens":0,"completion_tokens":0,"total_tokens":0})),
            ("no_total", json!({input:7,output:5}), json!({"prompt_tokens":7,"completion_tokens":5,"total_tokens":12})),
        ] {
            let mut data = template.clone();
            if name == "ollama" {
                data.as_object_mut().unwrap().remove("prompt_eval_count");
                data.as_object_mut().unwrap().remove("eval_count");
                if let Some(obj) = counters.as_object() { data.as_object_mut().unwrap().extend(obj.clone()); }
            } else { data["usage"] = counters; }
            Mock::given(method("POST")).and(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(data))
                .expect(1).mount(&server).await;
            let response = p.chat(request()).await.unwrap();
            assert_eq!(response.content.as_deref(), Some("ok"), "usage must not break valid content");
            let actual = serde_json::to_value(response.usage).unwrap();
            if actual != expected { failures.push(format!("{name}/{case}: expected {expected}, got {actual}")); }
            server.reset().await;
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn usage_metrics_anthropic_total_includes_disjoint_cached_input() {
    let response = super::anthropic::parse_anthropic_response(&serde_json::json!({
        "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":7,"output_tokens":5,"cache_creation_input_tokens":11,
            "cache_read_input_tokens":13,"cache_creation":{"ephemeral_5m_input_tokens":11}}
    })).unwrap();
    assert_eq!(serde_json::to_value(response.usage).unwrap(),
        serde_json::json!({"prompt_tokens":31,"completion_tokens":5,"total_tokens":36}));
}

#[test]
fn usage_metrics_totals_are_exact_or_unavailable_not_wrapped() {
    use serde_json::json;
    for responses in [false, true] {
        let (input, output) = if responses { ("input_tokens", "output_tokens") } else { ("prompt_tokens", "completion_tokens") };
        for (total, expected) in [(json!(null), Some(12)), (json!(12), Some(12)), (json!(19), Some(19)),
            (json!(0), None), (json!(-1), None), (json!("12"), None), (json!(12.5), None), (json!(4294967296u64), None)] {
            let data = json!({input:7,output:5,"total_tokens":total});
            let usage = if responses { Usage::responses(&data) } else { Usage::openai(&data) };
            assert_eq!(usage.map(|u| u.total_tokens), expected, "{data}");
        }
        let data = json!({input:4294967295u64,output:0});
        let usage = if responses { Usage::responses(&data) } else { Usage::openai(&data) };
        assert_eq!(usage.unwrap().total_tokens, u32::MAX);
    }
}

#[test]
fn usage_metrics_anthropic_invalid_cache_counts_do_not_hide_missing_usage() {
    use serde_json::json;
    for field in ["cache_creation_input_tokens", "cache_read_input_tokens"] {
        for count in [json!(null),json!(-1),json!(0.5),json!("13"),json!(4294967296u64),json!(4294967295u64)] {
            let response = super::anthropic::parse_anthropic_response(&json!({
                "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
                "usage":{"input_tokens":7,"output_tokens":5,field:count}
            })).unwrap();
            assert!(response.usage.is_none(), "{field} malformed/overflow cache count cannot be a valid usage sample");
        }
    }
}

#[tokio::test]
async fn usage_metrics_ollama_stream_keeps_missing_distinct_from_zero() {
    use serde_json::json;
    for counters in [json!({}),json!({"eval_count":5}),json!({"prompt_eval_count":0,"eval_count":0}),
        json!({"prompt_eval_count":7,"eval_count":5}),json!({"prompt_eval_count":4294967296u64,"eval_count":5})] {
        let server = MockServer::start().await;
        let mut end = counters.clone(); end["done"] = json!(true);
        let body = format!("{}\n{end}\n", json!({"message":{"content":"Not a token counter: 世界"},"done":false}));
        Mock::given(method("POST")).and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body)).expect(1).mount(&server).await;
        let (p, _, _) = adapter("ollama", server.uri());
        let r = p.chat_stream(request(), &|_| {}).await.unwrap();
        assert_eq!(r.content.as_deref(), Some("Not a token counter: 世界"));
        let expected = match counters["prompt_eval_count"].as_u64() {
            Some(0) => json!({"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}),
            Some(7) => json!({"prompt_tokens":7,"completion_tokens":5,"total_tokens":12}),
            _ => json!(null),
        };
        assert_eq!(serde_json::to_value(r.usage).unwrap(), expected);
    }
}

#[tokio::test]
async fn usage_metrics_failed_retry_is_not_added_to_validated_reply() {
    for streamed in [false, true] {
        let server = MockServer::start().await;
        let calls = AtomicUsize::new(0);
        Mock::given(method("POST")).and(path("/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices":[{"message":{"content": if first { "" } else { "ok" }},
                        "finish_reason":if first { "length" } else { "stop" }}],
                    "usage":{"prompt_tokens":7,"completion_tokens":if first {10} else {5},
                        "total_tokens":if first {17} else {12}}
                }))
            }).expect(2).mount(&server).await;
        let (p, _, _) = adapter("openai", server.uri());
        let r = LLMRouter::with_providers(vec![p], "openai".into(),vec![],policy());
        let response = if streamed { r.streaming_chat(request(),None,"usage-retry-fixture").await }
            else { r.chat(request(),None).await }.unwrap();
        assert_eq!(serde_json::to_value(response.usage).unwrap(),
            serde_json::json!({"prompt_tokens":7,"completion_tokens":5,"total_tokens":12}));
        crate::runtime::events::remove("usage-retry-fixture");
    }
}

#[tokio::test]
async fn model_listing_uses_the_common_provider_interface() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [{"name": "qwen3:8b"}, {"name": "nomic-embed-text"}]
        })))
        .mount(&server)
        .await;
    let provider = super::ollama::OllamaProvider::new(server.uri(), "qwen3:8b".into(), None);
    let models = praxis_provider_api::ChatProvider::list_models(&provider)
        .await
        .unwrap();
    assert_eq!(
        models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(),
        ["nomic-embed-text", "qwen3:8b"]
    );
    assert_eq!(models[1].label.as_deref(), Some("qwen3:8b"));

    // A provider that cannot enumerate says so instead of inventing an empty
    // catalog.
    let unsupported = super::openrouter::OpenRouterProvider::new(
        "sk-test".into(),
        "openrouter/auto".into(),
        "https://openrouter.invalid".into(),
    );
    let error = praxis_provider_api::ChatProvider::list_models(&unsupported)
        .await
        .unwrap_err();
    assert_eq!(error.kind, praxis_provider_api::ErrorKind::Unsupported);
}
