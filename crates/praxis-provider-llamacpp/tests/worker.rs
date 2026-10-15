use praxis_provider_api::{
    bridge, handshake::WorkerIdentity, process::WorkerLaunch, CallBinding, RequestContext,
    RequestEnvelope, WireCapabilities, WireChatRequest, WireLimits, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;
use praxis_provider_http;
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn capabilities() -> WireCapabilities {
    WireCapabilities {
        supports_output_limit: true,
        supports_list_models: true,
        supports_embeddings: false,
    }
}

async fn invoke(
    server: &MockServer,
    model: Option<&str>,
    deltas: &Mutex<Vec<praxis_provider_api::StreamDelta>>,
) -> Result<praxis_provider_api::WireChatAttempt, bridge::InvocationError> {
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                provider: "llamacpp".into(),
                worker_generation: 1,
                call_id: [1; 16],
            },
            request_id: [2; 16],
            nonce: [3; 16],
        },
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: Some(0.25),
            max_tokens: Some(100),
            model: model.map(str::to_owned),
            thinking: None,
        },
        continuation: None,
        conversation: None,
    };
    bridge::invoke(
        WorkerLaunch {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_praxis-provider-llamacpp")),
            arguments: vec![format!("{}/prefix///", server.uri()), "fixture-model".into()],
            environment: BTreeMap::new(),
            identity: WorkerIdentity {
                provider: "llamacpp".into(),
                generation: 1,
                nonce: [3; 16],
            },
            capabilities: capabilities(),
            limits: WireLimits::default(),
        },
        &request,
        Duration::from_secs(10),
        &CancellationToken::new(),
        &|delta| deltas.lock().unwrap().push(delta),
    )
    .await
}

#[test]
fn invalid_anonymous_configuration_is_safe() {
    for args in [
        vec!["https://user:PRIVATE@localhost", "model"],
        vec!["http://localhost/?key=PRIVATE", "model"],
        vec!["http://localhost/#PRIVATE", "model"],
        vec!["file:///PRIVATE", "model"],
        vec!["http://localhost", ""],
        vec!["http://localhost", "model", "PRIVATE"],
        vec!["http://localhost", "model", "--credentials"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_praxis-provider-llamacpp"))
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    }
}

#[tokio::test]
async fn compiled_worker_forwards_url_model_options_usage_and_stream_deltas() {
    for model in [None, Some("override-model")] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/prefix/props"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"n_ctx": 8192}),
            ))
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tokens": [1, 2, 3]})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prefix/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Bonjour 日本語\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\" !\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\n",
                "data: [DONE]\n\n"
            )))
            .expect(1)
            .mount(&server)
            .await;
        let deltas = Mutex::new(vec![]);
        let attempt = invoke(&server, model, &deltas).await.unwrap();
        assert_eq!(attempt.response.content.as_deref(), Some("Bonjour 日本語 !"));
        let usage = attempt.response.usage.unwrap();
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (10, 2, 12)
        );
        let text: String = deltas
            .into_inner()
            .unwrap()
            .into_iter()
            .filter_map(|delta| match delta {
                praxis_provider_api::StreamDelta::Text { text } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Bonjour 日本語 !");
        let requests = server.received_requests().await.unwrap();
        for request in &requests {
            assert!(!request.headers.contains_key("authorization"));
        }
        let body = requests
            .iter()
            .find(|request| request.url.path().ends_with("/chat/completions"))
            .unwrap()
            .body_json::<serde_json::Value>()
            .unwrap();
        assert_eq!(body["model"], model.unwrap_or("fixture-model"));
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["temperature"], 0.25);
        assert_eq!(body["max_tokens"], 100);
    }
}

fn provider_error(error: bridge::InvocationError) -> praxis_provider_api::ProviderError {
    assert!(!format!("{error:?}").contains("PRIVATE"));
    let bridge::InvocationError::Exchange(praxis_provider_api::exchange::ExchangeError::Provider(
        error,
    )) = error
    else {
        panic!("expected typed provider error");
    };
    error.to_provider().unwrap()
}

#[tokio::test]
async fn compiled_worker_safe_failures_have_no_hidden_retry() {
    use praxis_provider_api::ErrorKind;
    for (response, kind) in [
        (
            ResponseTemplate::new(429)
                .insert_header("retry-after", "2")
                .set_body_string("PRIVATE"),
            ErrorKind::RateLimited,
        ),
        (
            ResponseTemplate::new(500).set_body_json(json!({"error": {
                "code": "out_of_context",
                "message": "PRIVATE prompt exceeds the available context"
            }})),
            ErrorKind::ContextWindow,
        ),
        (
            ResponseTemplate::new(200).set_body_string(
                "data: {\"error\":\"PRIVATE model not found\"}\n\ndata: [DONE]\n\n",
            ),
            ErrorKind::Configuration,
        ),
        (
            ResponseTemplate::new(200)
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"),
            ErrorKind::Interrupted,
        ),
        (
            ResponseTemplate::new(200).set_body_string("data: not json\n\n"),
            ErrorKind::InvalidResponse,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/prefix/props"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prefix/v1/chat/completions"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let error = provider_error(invoke(&server, None, &Mutex::new(vec![])).await.unwrap_err());
        assert_eq!(error.kind, kind);
        if kind == ErrorKind::RateLimited {
            assert_eq!(error.retry_after, Some(Duration::from_secs(2)));
        }
        assert_eq!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn compiled_worker_bounds_shared_props_json_and_sse_frames() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/prefix/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "n_ctx": 8192,
            "PRIVATE": "x".repeat(praxis_provider_http::MAX_JSON_BYTES)
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prefix/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: ".to_string() + &"x".repeat(1_048_577) + "\n\n",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let error = provider_error(invoke(&server, None, &Mutex::new(vec![])).await.unwrap_err());
    assert_eq!(error.kind, praxis_provider_api::ErrorKind::InvalidResponse);
    let requests = server.received_requests().await.unwrap();
    // props + chat only: the oversized probe is ignored, not retried, and no
    // template/tokenize preflight is built on it.
    assert_eq!(requests.len(), 2);
}
