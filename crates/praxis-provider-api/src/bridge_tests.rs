use super::*;
use crate::{
    handshake::WorkerIdentity, CallBinding, RequestContext, WireCapabilities, WireChatRequest,
    WireLimits, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf};

fn fixture() -> (WorkerLaunch, RequestEnvelope) {
    let context = RequestContext {
        binding: CallBinding {
            provider: "fixture".into(),
            worker_generation: 1,
            call_id: [1; 16],
        },
        request_id: [2; 16],
        nonce: [3; 16],
    };
    let launch = WorkerLaunch {
        executable: PathBuf::from("/nonexistent-worker"),
        arguments: vec![],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "fixture".into(),
            generation: 1,
            nonce: context.nonce,
        },
        capabilities: WireCapabilities {
            supports_output_limit: false,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
    };
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context,
        continuation: None,
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: None,
            model: None,
            thinking: None,
        },
    };
    (launch, request)
}

#[tokio::test]
async fn invalid_request_and_budget_do_not_spawn() {
    for case in 0..5 {
        let (launch, mut request) = fixture();
        let deadline = match case {
            0 => Duration::ZERO,
            1 => Duration::from_secs(301),
            _ => Duration::from_secs(1),
        };
        match case {
            2 => request.context.nonce = [4; 16],
            3 => request.version += 1,
            4 => request.context.binding.provider = "foreign".into(),
            _ => (),
        }
        assert!(matches!(
            invoke(
                launch,
                &request,
                deadline,
                &CancellationToken::new(),
                &|_| panic!("unexpected delta")
            )
            .await,
            Err(InvocationError::Validation)
        ));
    }
}

#[tokio::test]
async fn cancelled_invocation_does_not_spawn() {
    let (launch, request) = fixture();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        invoke(launch, &request, Duration::from_secs(1), &cancel, &|_| {}).await,
        Err(InvocationError::Cancelled)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn real_process_invocation_validates_terminal_and_context() {
    use crate::{ChatResponse, StreamEnvelope, WireStreamEvent};
    for mode in ["success", "eof", "foreign"] {
        let (mut launch, request) = fixture();
        let mut event = StreamEnvelope {
            version: WIRE_VERSION,
            context: request.context.clone(),
            seq: 0,
            event: WireStreamEvent::Attempt(WireChatAttempt {
                response: ChatResponse {
                    content: Some("checked answer".into()),
                    reasoning_content: None,
                    tool_calls: None,
                    finish_reason: None,
                    usage: None,
                },
                continuation: None,
            }),
        };
        if mode == "foreign" {
            event.context.request_id = [9; 16];
            event.event = WireStreamEvent::Delta(StreamDelta::Text {
                text: "untrusted".into(),
            });
        }
        launch.executable = PathBuf::from("/usr/bin/python3");
        launch.arguments = vec![
            "-c".into(),
            r#"
import sys, json, struct

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
read()
if sys.argv[1] != 'eof':
    write(json.loads(sys.argv[2]))
"#
            .into(),
            mode.into(),
            serde_json::to_string(&event).unwrap(),
        ];
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            invoke(
                launch,
                &request,
                Duration::from_secs(1),
                &CancellationToken::new(),
                &|_| panic!("unchecked or unexpected delta delivered"),
            ),
        )
        .await
        .unwrap();
        match mode {
            "success" => assert_eq!(
                result.unwrap().response.content.as_deref(),
                Some("checked answer")
            ),
            "eof" => assert!(matches!(
                result,
                Err(InvocationError::Exchange(ExchangeError::MissingTerminal))
            )),
            _ => assert!(matches!(
                result,
                Err(InvocationError::Exchange(ExchangeError::Validation))
            )),
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_and_cancellation_cover_readiness() {
    for cancelled in [false, true] {
        let (mut launch, request) = fixture();
        launch.executable = PathBuf::from("/bin/sh");
        launch.arguments = vec!["-c".into(), "exec sleep 30".into()];
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let signal = async {
                if cancelled {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    cancel.cancel();
                }
            };
            let (result, ()) = tokio::join!(
                invoke(
                    launch,
                    &request,
                    Duration::from_millis(50),
                    &cancel,
                    &|_| {}
                ),
                signal
            );
            result
        })
        .await
        .unwrap();
        if cancelled {
            assert!(matches!(result, Err(InvocationError::Cancelled)));
        } else {
            assert!(matches!(result, Err(InvocationError::Timeout)));
        }
    }
}
