use praxis_provider_api::{
    bridge, handshake::WorkerIdentity, process::WorkerLaunch, CallBinding, RequestContext,
    RequestEnvelope, WireCapabilities, WireChatRequest, WireLimits, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request() -> RequestEnvelope {
    RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                provider: "openai".into(),
                worker_generation: 1,
                call_id: [1; 16],
            },
            request_id: [2; 16],
            nonce: [3; 16],
        },
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: Some(100),
            model: None,
            thinking: None,
        },
        continuation: None,
    }
}

async fn invoke(
    server: &MockServer,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    invoke_mode(server, None).await
}

async fn invoke_mode(
    server: &MockServer,
    key: Option<&str>,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    let request = request();
    let mut launch = WorkerLaunch {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_praxis-provider-openai")),
        arguments: vec![server.uri(), "fixture-model".into()],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "openai".into(),
            generation: 1,
            nonce: [3; 16],
        },
        capabilities: WireCapabilities {
            supports_output_limit: true,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
    };
    if let Some(key) = key {
        launch.arguments.push("--credentials".into());
        let values = BTreeMap::from([("api_key".into(), key.into())]);
        let grant = praxis_provider_api::credentials::CredentialGrant::new(
            launch.identity.clone(),
            &["api_key".into()],
            values,
        )
        .unwrap();
        return bridge::invoke_with_credentials(
            launch,
            &request,
            &grant,
            Duration::from_secs(5),
            &CancellationToken::new(),
            &|_| panic!("unexpected delta"),
        )
        .await;
    }
    bridge::invoke(
        launch,
        &request,
        Duration::from_secs(5),
        &CancellationToken::new(),
        &|_| panic!("non-streaming provider emitted a delta"),
    )
    .await
}

#[test]
fn invalid_worker_configuration_never_writes_protocol_or_secret_diagnostics() {
    for url in [
        "https://user:PRIVATE@localhost/v1",
        "http://localhost/?key=PRIVATE",
        "http://localhost/#PRIVATE",
        "file:///PRIVATE",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_praxis-provider-openai"))
            .env_clear()
            .args([url, "fixture-model"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_praxis-provider-openai"))
        .env_clear()
        .args(["http://localhost/v1", "model", "PRIVATE_KEY"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_KEY"));
}

#[tokio::test]
async fn compiled_worker_calls_http_once_and_returns_checked_chat() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices":[{"message":{"content":"worker answer"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let attempt = invoke(&server).await.unwrap();
    assert_eq!(attempt.response.content.as_deref(), Some("worker answer"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("authorization"));
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["max_tokens"], 100);
}

#[tokio::test]
async fn compiled_worker_receives_scoped_key_and_rejects_invalid_header_values() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer PRIVATE_TOKEN",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices":[{"message":{"content":"authenticated answer"},"finish_reason":"stop"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let attempt = invoke_mode(&server, Some("PRIVATE_TOKEN")).await.unwrap();
    assert_eq!(
        attempt.response.content.as_deref(),
        Some("authenticated answer")
    );
    let error = invoke_mode(&server, Some("PRIVATE\nTOKEN"))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn compiled_worker_preserves_safe_rate_limit_without_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2")
            .set_body_json(serde_json::json!({"error":{"code":"rate_limit_exceeded","message":"PRIVATE BODY"}})))
        .expect(1).mount(&server).await;
    let error = invoke(&server).await.unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE BODY"));
    let bridge::InvocationError::Exchange(praxis_provider_api::exchange::ExchangeError::Provider(
        error,
    )) = error
    else {
        panic!("expected typed worker rate limit");
    };
    let error = error.to_provider().unwrap();
    assert_eq!(error.kind, praxis_provider_api::ErrorKind::RateLimited);
    assert_eq!(error.retry_after, Some(Duration::from_secs(2)));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
