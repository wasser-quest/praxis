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
                provider: "anthropic".into(),
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
        conversation: None,
    }
}

async fn invoke(
    server: &MockServer,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    invoke_mode(server, Some("PRIVATE_TOKEN")).await
}

async fn invoke_mode(
    server: &MockServer,
    key: Option<&str>,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    invoke_request(&server.uri(), request(), key).await
}

async fn invoke_request(
    endpoint: &str,
    request: RequestEnvelope,
    key: Option<&str>,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    let launch = WorkerLaunch {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_praxis-provider-anthropic")),
        arguments: vec![
            endpoint.into(),
            "fixture-model".into(),
            "--credentials".into(),
        ],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "anthropic".into(),
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
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_praxis-provider-anthropic"))
            .env_clear()
            .args([url, "fixture-model", "--credentials"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_praxis-provider-anthropic"))
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
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content":[{"type":"text","text":"worker answer"}],"stop_reason":"max_tokens",
            "usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let attempt = invoke(&server).await.unwrap();
    assert_eq!(attempt.response.content.as_deref(), Some("worker answer"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers["x-api-key"], "PRIVATE_TOKEN");
    assert_eq!(requests[0].headers["anthropic-version"], "2023-06-01");
    assert!(requests[0].headers["user-agent"]
        .to_str()
        .unwrap()
        .starts_with("Praxis/"));
    assert!(!requests[0].headers.contains_key("authorization"));
    assert_eq!(
        attempt.response.finish_reason.as_deref(),
        Some("max_tokens")
    );
    assert!(attempt.continuation.is_none());
    let usage = attempt.response.usage.unwrap();
    assert_eq!(
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens
        ),
        (17, 2, 19)
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["max_tokens"], 100);
}

#[tokio::test]
async fn compiled_worker_receives_scoped_key_and_rejects_invalid_header_values() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(wiremock::matchers::header("x-api-key", "PRIVATE_TOKEN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content":[{"type":"text","text":"authenticated answer"}],"stop_reason":"end_turn"
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
    Mock::given(method("POST")).and(path("/v1/messages"))
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

#[tokio::test]
async fn compiled_worker_forwards_system_tools_model_and_thinking() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content":[{"type":"tool_use","id":"next","name":"lookup","input":{"q":"y"}}],
            "stop_reason":"tool_use"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let mut req = request();
    req.request.messages = serde_json::from_value(serde_json::json!([
        {"role":"system","content":"instructions"},
        {"role":"assistant","tool_calls":[{"id":"call","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},
        {"role":"tool","tool_call_id":"call","content":"result","content_parts":[{"type":"text","text":"extra"}]}
    ])).unwrap();
    req.request.tools = Some(serde_json::from_value(serde_json::json!([{
        "type":"function","function":{"name":"lookup","description":"Lookup","parameters":{"type":"object"}}
    }])).unwrap());
    req.request.model = Some("override-model".into());
    req.request.max_tokens = Some(8000);
    req.request.thinking = Some(praxis_provider_api::ThinkingMode::High);
    let attempt = invoke_request(&server.uri(), req, Some("SECOND_PRIVATE_TOKEN"))
        .await
        .unwrap();
    assert!(attempt.continuation.is_none());
    assert!(attempt.response.usage.is_none());
    assert_eq!(attempt.response.tool_calls.unwrap()[0].id, "next");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["x-api-key"], "SECOND_PRIVATE_TOKEN");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["system"], "instructions");
    assert_eq!(body["model"], "override-model");
    assert_eq!(body["max_tokens"], 8000);
    assert_eq!(body["thinking"]["budget_tokens"], 4000);
    assert_eq!(
        body["tools"][0]["input_schema"],
        serde_json::json!({"type":"object"})
    );
    assert_eq!(
        body["messages"][0]["content"][0]["input"],
        serde_json::json!({"q":"x"})
    );
    assert_eq!(
        body["messages"][1]["content"][0]["content"][1]["text"],
        "extra"
    );
}

#[tokio::test]
async fn invalid_credentials_and_continuations_never_reach_http() {
    let server = MockServer::start().await;
    for key in [
        None,
        Some("PRIVATE TOKEN"),
        Some("PRIVATE\rTOKEN"),
        Some("PRIVATEé"),
    ] {
        let error = invoke_mode(&server, key).await.unwrap_err();
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    for wrong_binding in [false, true] {
        let mut req = request();
        let mut binding = req.context.binding.clone();
        if wrong_binding {
            binding.call_id = [99; 16];
        }
        req.continuation = Some(
            praxis_provider_api::WireContinuation::new(
                binding,
                b"PRIVATE INVALID CONTINUATION".to_vec(),
                WireLimits::default(),
            )
            .unwrap(),
        );
        let error = invoke_request(&server.uri(), req, Some("PRIVATE_TOKEN"))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    let launch = WorkerLaunch {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_praxis-provider-anthropic")),
        arguments: vec![server.uri(), "model".into(), "--credentials".into()],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "anthropic".into(),
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
    let grant = praxis_provider_api::credentials::CredentialGrant::new(
        WorkerIdentity {
            provider: "openai".into(),
            generation: 1,
            nonce: [3; 16],
        },
        &["api_key".into()],
        BTreeMap::from([("api_key".into(), "PRIVATE_FOREIGN_TOKEN".into())]),
    )
    .unwrap();
    let error = bridge::invoke_with_credentials(
        launch,
        &request(),
        &grant,
        Duration::from_secs(5),
        &CancellationToken::new(),
        &|_| panic!("unexpected delta"),
    )
    .await
    .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn compiled_worker_invalid_usage_is_unavailable_and_bad_json_is_safe() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content":[], "usage":{"input_tokens":4294967295u64,"output_tokens":1}
        })))
        .expect(1)
        .mount(&server)
        .await;
    assert!(invoke(&server).await.unwrap().response.usage.is_none());
    server.reset().await;
    Mock::given(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("PRIVATE BODY PRIVATE_TOKEN", "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = invoke(&server).await.unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    let bridge::InvocationError::Exchange(praxis_provider_api::exchange::ExchangeError::Provider(
        error,
    )) = error
    else {
        panic!("expected typed invalid response");
    };
    assert_eq!(
        error.to_provider().unwrap().kind,
        praxis_provider_api::ErrorKind::InvalidResponse
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn compiled_worker_bounds_success_json_before_wire_output() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content":[{"type":"text","text":"x".repeat(praxis_provider_http::MAX_JSON_BYTES)}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let error = invoke(&server).await.unwrap_err();
    let bridge::InvocationError::Exchange(praxis_provider_api::exchange::ExchangeError::Provider(
        error,
    )) = error
    else {
        panic!("expected bounded JSON error");
    };
    assert_eq!(
        error.to_provider().unwrap().kind,
        praxis_provider_api::ErrorKind::InvalidResponse
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn compiled_worker_transport_failure_is_typed_safe_and_not_retried() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/PRIVATE_PATH", listener.local_addr().unwrap());
    let connections = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), listener.accept())
                .await
                .is_err()
        );
    });
    let error = invoke_request(&endpoint, request(), Some("PRIVATE_TOKEN"))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    let bridge::InvocationError::Exchange(praxis_provider_api::exchange::ExchangeError::Provider(
        error,
    )) = error
    else {
        panic!("expected typed transport error");
    };
    assert_eq!(
        error.to_provider().unwrap().kind,
        praxis_provider_api::ErrorKind::Transport
    );
    connections.await.unwrap();
}
