use praxis_provider_api::{ChatProvider, ChatRequest, ErrorKind};
use praxis_provider_ollama::OllamaProvider;
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> ChatRequest {
    serde_json::from_value(json!({"messages":[],"max_tokens":100})).unwrap()
}

#[tokio::test]
async fn nonstream_json_errors_remain_typed_safe_and_bounded() {
    for (body, kind) in [
        (
            json!({"error":"PRIVATE model not found"}),
            ErrorKind::Configuration,
        ),
        (
            json!({"error":"PRIVATE context length exceeded"}),
            ErrorKind::ContextWindow,
        ),
        (
            json!({"error":"PRIVATE insufficient VRAM memory"}),
            ErrorKind::Unavailable,
        ),
        (
            json!({"message":{"content":"x".repeat(praxis_provider_http::MAX_JSON_BYTES)}}),
            ErrorKind::InvalidResponse,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = OllamaProvider::new(server.uri(), "fixture".into(), None)
            .chat(request())
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
    }
}

#[tokio::test]
async fn list_models_and_context_probe_use_shared_bounded_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"models":[],"PRIVATE":"x".repeat(praxis_provider_http::MAX_JSON_BYTES)}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"message":{"content":"answer"},"done":true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let provider = OllamaProvider::new(server.uri(), "fixture".into(), None);
    assert_eq!(
        provider.list_models().await.unwrap_err().kind,
        ErrorKind::InvalidResponse
    );
    // A failed best-effort probe preserves legacy options and never retries.
    assert_eq!(
        provider.chat(request()).await.unwrap().content.as_deref(),
        Some("answer")
    );
    let requests = server.received_requests().await.unwrap();
    let body = requests
        .iter()
        .find(|r| r.method == "POST")
        .unwrap()
        .body_json::<serde_json::Value>()
        .unwrap();
    assert_eq!(body["options"], json!({"num_predict":100}));
    assert_eq!(body["stream"], false);
}
