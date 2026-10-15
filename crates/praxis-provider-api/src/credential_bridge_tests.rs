use super::*;
use crate::{
    credentials::CredentialGrant, handshake::WorkerIdentity, CallBinding, RequestContext,
    WireCapabilities, WireChatRequest, WireLimits, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf};

fn fixture() -> (WorkerLaunch, RequestEnvelope) {
    let identity = WorkerIdentity {
        provider: "fixture".into(),
        generation: 1,
        nonce: [3; 16],
    };
    let launch = WorkerLaunch {
        executable: PathBuf::from("/nonexistent-worker"),
        arguments: vec![],
        environment: BTreeMap::new(),
        identity,
        capabilities: WireCapabilities {
            supports_output_limit: false,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
    };
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                provider: "fixture".into(),
                worker_generation: 1,
                call_id: [1; 16],
            },
            request_id: [2; 16],
            nonce: [3; 16],
        },
        continuation: None,
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: None,
            model: None,
            thinking: None,
        },
        conversation: None,
    };
    (launch, request)
}
fn grant(identity: WorkerIdentity) -> CredentialGrant {
    CredentialGrant::new(
        identity,
        &["api_key".into()],
        BTreeMap::from([("api_key".into(), "PRIVATE_TOKEN".into())]),
    )
    .unwrap()
}

#[tokio::test]
async fn foreign_grant_is_rejected_before_spawn() {
    let (launch, request) = fixture();
    let mut foreign = launch.identity.clone();
    foreign.generation += 1;
    let value = grant(foreign);
    assert!(matches!(
        invoke_with_credentials(
            launch,
            &request,
            &value,
            Duration::from_secs(1),
            &CancellationToken::new(),
            &|_| {}
        )
        .await,
        Err(InvocationError::Validation)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn private_process_receives_grant_before_request_without_arguments_or_environment() {
    let (mut launch, request) = fixture();
    let value = grant(launch.identity.clone());
    let event = crate::StreamEnvelope {
        version: WIRE_VERSION,
        context: request.context.clone(),
        seq: 0,
        event: crate::WireStreamEvent::Attempt(WireChatAttempt {
            response: crate::ChatResponse {
                content: Some("authenticated fixture".into()),
                reasoning_content: None,
                tool_calls: None,
                finish_reason: None,
                usage: None,
            },
            continuation: None,
        }),
    };
    launch.executable = PathBuf::from("/usr/bin/python3");
    launch.arguments = vec![
        "-c".into(),
        r#"
import sys, json, struct, os
def read():
    n = struct.unpack('>I', sys.stdin.buffer.read(4))[0]
    return json.loads(sys.stdin.buffer.read(n))
def write(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
    sys.stdout.buffer.flush()
hello = read()
hello['type'] = 'ready'
write(hello)
grant = read()
assert grant['type'] == 'credentials'
for key in ['provider', 'generation', 'nonce']:
    assert grant[key] == hello['identity'][key]
secret = grant['secrets'][0]['value']
assert grant['secrets'][0]['name'] == 'api_key'
assert secret and not any(secret in arg for arg in sys.argv)
assert not any(secret in val for val in os.environ.values())
request = read()
assert request['context']['nonce'] == hello['identity']['nonce']
write(json.loads(sys.argv[1]))
"#
        .into(),
        serde_json::to_string(&event).unwrap(),
    ];
    let result = invoke_with_credentials(
        launch,
        &request,
        &value,
        Duration::from_secs(1),
        &CancellationToken::new(),
        &|_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        result.response.content.as_deref(),
        Some("authenticated fixture")
    );
}
