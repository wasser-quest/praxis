//! Arbitrary HTTP chunk boundaries must not overflow the worker's delta queue.
use praxis_provider_api::{
    bridge, handshake::WorkerIdentity, process::WorkerLaunch, CallBinding, RequestContext,
    RequestEnvelope, WireCapabilities, WireChatRequest, WireLimits, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn compiled_worker_drains_many_records_in_one_http_chunk() {
    let server = MockServer::start().await;
    let mut frames = "{\"message\":{\"content\":\"a\"},\"done\":false}\n".repeat(40);
    // A single frame can also contain more previews than the queue capacity.
    let calls = (0..40)
        .map(|_| serde_json::json!({"function":{"name":"fixture","arguments":{}}}))
        .collect::<Vec<_>>();
    frames += &serde_json::json!({"message":{"tool_calls":calls},"done":true}).to_string();
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(frames))
        .expect(1)
        .mount(&server)
        .await;
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                provider: "ollama".into(),
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
    };
    let deltas = Mutex::new(vec![]);
    let attempt = bridge::invoke(
        WorkerLaunch {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_praxis-provider-ollama")),
            arguments: vec![server.uri(), "fixture".into()],
            environment: BTreeMap::new(),
            identity: WorkerIdentity {
                provider: "ollama".into(),
                generation: 1,
                nonce: [3; 16],
            },
            capabilities: WireCapabilities {
                supports_output_limit: true,
                supports_list_models: false,
                supports_embeddings: false,
            },
            limits: WireLimits::default(),
        },
        &request,
        Duration::from_secs(10),
        &CancellationToken::new(),
        &|d| deltas.lock().unwrap().push(d),
    )
    .await
    .unwrap();
    assert_eq!(
        attempt.response.content.as_deref(),
        Some("a".repeat(40).as_str())
    );
    assert_eq!(attempt.response.tool_calls.unwrap().len(), 40);
    assert_eq!(deltas.into_inner().unwrap().len(), 80);
}
