use praxis_provider_api::{ChatProvider, ChatRequest, ErrorKind};
use praxis_provider_llamacpp::LlamaCppProvider;
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> ChatRequest {
    serde_json::from_value(json!({"messages": [], "max_tokens": 100})).unwrap()
}

#[tokio::test]
async fn nonstream_json_errors_remain_typed_safe_and_bounded() {
    for (body, kind) in [
        (json!({"error": "PRIVATE model not found"}), ErrorKind::Configuration),
        (
            json!({"error": "PRIVATE prompt (4096 tokens) exceeds the available context size (2048 tokens), try increasing it"}),
            ErrorKind::ContextWindow,
        ),
        (json!({"error": "PRIVATE n_keep is larger than n_ctx"}), ErrorKind::ContextWindow),
        (json!({"error": "PRIVATE insufficient VRAM memory"}), ErrorKind::Unavailable),
        (
            json!({"choices": [{"message": {"content": "x".repeat(praxis_provider_http::MAX_JSON_BYTES)}}]}),
            ErrorKind::InvalidResponse,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prefix/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()))
            .chat(request())
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
    }
}

#[tokio::test]
async fn preflight_is_best_effort_and_a_missing_tokenizer_still_reaches_the_chat_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices": [{"message": {"content": "answer"}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 7, "completion_tokens": 5, "total_tokens": 12}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider =
        LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let response = provider.chat(request()).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("answer"));
    let usage = response.usage.unwrap();
    assert_eq!(
        (usage.prompt_tokens, usage.completion_tokens, usage.total_tokens),
        (7, 5, 12)
    );
}

#[tokio::test]
async fn exact_preflight_caps_output_and_reports_template_omission() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"n_ctx": 1024})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/apply-template"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt": "rendered"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/tokenize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"tokens": (0..900).collect::<Vec<u64>>(), "PRIVATE": "x".repeat(2048)}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"choices": [{"message": {"content": "answer"}, "finish_reason": "stop"}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider =
        LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let response = provider.chat(request()).await.unwrap();
    // 900 tokens + output + margin > 1024, so the ladder ran; output is capped
    // to n_ctx/4 and the minimal system replaced the persona.
    let calls = server.received_requests().await.unwrap();
    let chat = calls
        .iter()
        .find(|call| call.url.path().ends_with("/chat/completions"))
        .unwrap();
    let body = chat.body_json::<serde_json::Value>().unwrap();
    assert_eq!(body["max_tokens"], 256);
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("could not fit"));
    assert!(response.content.as_deref().unwrap().contains("--template not rendered context to big--"));
    assert_eq!(calls.iter().filter(|call| call.url.path().ends_with("/tokenize")).count(), 1);
}

#[tokio::test]
async fn confirmed_overflow_recovers_once_without_replaying_the_request_twice_after_reduction() {
    let server = MockServer::start().await;
    let first = std::sync::atomic::AtomicBool::new(true);
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            if first.fetch_and(false, std::sync::atomic::Ordering::Relaxed) {
                ResponseTemplate::new(500).set_body_json(json!({"error": {
                    "code": "out_of_context",
                    "message": "PRIVATE prompt exceeds the available context"
                }}))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"choices": [
                    {"message": {"content": "recovered"}, "finish_reason": "stop"}
                ]}))
            }
        })
        .mount(&server)
        .await;
    let provider =
        LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let mut request = request();
    request.messages = vec![
        serde_json::from_value(json!({"role": "system", "content": "persona"})).unwrap(),
        serde_json::from_value(json!({"role": "user", "content": "old turn"})).unwrap(),
        serde_json::from_value(
            json!({"role": "tool", "content": "x".repeat(4096) + "\n\n[Saved tool response] output_id=out_0000000000000000000000000000bbbb"}),
        )
        .unwrap(),
        serde_json::from_value(json!({"role": "user", "content": "new"})).unwrap(),
    ];
    let response = provider.chat(request).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("recovered"));
    let calls = server.received_requests().await.unwrap();
    assert_eq!(calls.iter().filter(|call| call.method == "POST").count(), 2);
}

#[tokio::test]
async fn anonymous_worker_path_sends_no_authorization_and_bounded_json_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "n_ctx": 8192, "PRIVATE": "x".repeat(praxis_provider_http::MAX_JSON_BYTES)
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "x".repeat(praxis_provider_http::MAX_JSON_BYTES + 1),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let provider =
        LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let error = provider.chat(request()).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidResponse);
    let calls = server.received_requests().await.unwrap();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert!(!call.headers.contains_key("authorization"));
    }
    // Oversized probe is ignored, never parsed or retried.
    let body = calls
        .iter()
        .find(|call| call.method == "POST")
        .unwrap()
        .body_json::<serde_json::Value>();
    assert!(body.is_err(), "oversized payload is not parsed as JSON");
}

#[tokio::test]
async fn list_models_and_health_use_shared_bounded_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "b-model"}, {"id": "a-model"}, {"id": ""}],
            "PRIVATE": "x".repeat(praxis_provider_http::MAX_JSON_BYTES)
        })))
        .expect(2)
        .mount(&server)
        .await;
    let provider =
        LlamaCppProvider::new(None, "fixture".into(), format!("{}/prefix", server.uri()));
    let error = provider.list_models().await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidResponse);
    assert!(provider.health_check().await);
}
