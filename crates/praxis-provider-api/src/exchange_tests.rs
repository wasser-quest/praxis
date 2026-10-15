//! Real-pipe exchange regressions using an external scripted worker.
use super::*;
use crate::{
    handshake::WorkerIdentity, process::WorkerLaunch, CallBinding, ChatResponse, StreamEnvelope,
    WireCapabilities, WireChatRequest, WIRE_VERSION,
};
use std::{collections::BTreeMap, path::PathBuf};

fn context() -> RequestContext {
    RequestContext {
        binding: CallBinding {
            call_id: [1; 16],
            provider: "fixture".into(),
            worker_generation: 1,
        },
        request_id: [2; 16],
        nonce: [3; 16],
    }
}
fn request() -> RequestEnvelope {
    RequestEnvelope {
        version: WIRE_VERSION,
        context: context(),
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
    }
}
async fn worker(mode: &str, sequence: u64) -> WorkerProcess {
    let terminal = StreamEnvelope {
        version: WIRE_VERSION,
        context: context(),
        seq: sequence,
        event: WireStreamEvent::Attempt(WireChatAttempt {
            response: ChatResponse {
                content: Some("answer".into()),
                reasoning_content: None,
                tool_calls: None,
                finish_reason: None,
                usage: None,
            },
            continuation: None,
        }),
    };
    WorkerLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".into(),
            r#"
import sys, json, struct, time
def read():
    n = struct.unpack('>I', sys.stdin.buffer.read(4))[0]
    return json.loads(sys.stdin.buffer.read(n))
def write(body):
    body = json.dumps(body).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
    sys.stdout.buffer.flush()
hello = read()
hello['type'] = 'ready'
write(hello)
request = read()
if sys.argv[1] == 'terminal': write(json.loads(sys.argv[2]))
if sys.argv[1] == 'active': write(json.loads(sys.argv[3]))
if sys.argv[1] in ('hang', 'active'): time.sleep(30)
"#
            .into(),
            mode.into(),
            serde_json::to_string(&terminal).unwrap(),
            serde_json::to_string(&StreamEnvelope {
                version: WIRE_VERSION,
                context: context(),
                seq: 0,
                event: WireStreamEvent::Delta(StreamDelta::Text {
                    text: "started".into(),
                }),
            })
            .unwrap(),
        ],
        environment: BTreeMap::new(),
        identity: WorkerIdentity {
            provider: "fixture".into(),
            generation: 1,
            nonce: [3; 16],
        },
        capabilities: WireCapabilities {
            supports_output_limit: false,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
    }
    .spawn()
    .await
    .unwrap()
}

#[tokio::test]
async fn foreign_worker_binding_is_rejected_before_request_delivery() {
    for field in 0..3 {
        let mut expected = context();
        match field {
            0 => expected.binding.provider = "foreign".into(),
            1 => expected.binding.worker_generation += 1,
            _ => expected.nonce = [9; 16],
        }
        let mut request = request();
        request.context = expected.clone();
        let result = attempt(
            worker("hang", 0).await,
            &request,
            &expected,
            WireLimits::default(),
            Duration::from_secs(2),
            &CancellationToken::new(),
            &|_| panic!("foreign worker must not deliver deltas"),
        )
        .await;
        assert!(matches!(result, Err(ExchangeError::Validation)));
    }
}

#[tokio::test]
async fn invalid_deadlines_fail_without_delivering_deltas() {
    for deadline in [Duration::ZERO, Duration::from_secs(301)] {
        let result = attempt(
            worker("hang", 0).await,
            &request(),
            &context(),
            WireLimits::default(),
            deadline,
            &CancellationToken::new(),
            &|_| panic!("invalid deadline must not deliver deltas"),
        )
        .await;
        assert!(matches!(result, Err(ExchangeError::InvalidDeadline)));
    }
}

#[tokio::test]
async fn real_pipe_terminal_eof_and_sequence_validation() {
    for (mode, sequence) in [("terminal", 0), ("eof", 0), ("terminal", 1)] {
        let result = attempt(
            worker(mode, sequence).await,
            &request(),
            &context(),
            WireLimits::default(),
            Duration::from_secs(2),
            &CancellationToken::new(),
            &|_| {},
        )
        .await;
        match mode {
            "eof" => assert!(matches!(result, Err(ExchangeError::MissingTerminal))),
            _ if sequence == 1 => assert!(matches!(result, Err(ExchangeError::Validation))),
            _ => assert_eq!(result.unwrap().response.content.as_deref(), Some("answer")),
        }
    }
}

#[tokio::test]
async fn real_pipe_deadline_and_cancellation_are_bounded() {
    let result = attempt(
        worker("hang", 0).await,
        &request(),
        &context(),
        WireLimits::default(),
        Duration::from_millis(30),
        &CancellationToken::new(),
        &|_| {},
    )
    .await;
    assert!(matches!(result, Err(ExchangeError::Timeout)));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = attempt(
        worker("hang", 0).await,
        &request(),
        &context(),
        WireLimits::default(),
        Duration::from_secs(2),
        &cancel,
        &|_| {},
    )
    .await;
    assert!(matches!(result, Err(ExchangeError::Cancelled)));
}

#[tokio::test]
async fn cancellation_after_checked_delta_ends_active_exchange() {
    let cancel = CancellationToken::new();
    let seen = std::sync::atomic::AtomicBool::new(false);
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        attempt(
            worker("active", 0).await,
            &request(),
            &context(),
            WireLimits::default(),
            Duration::from_secs(2),
            &cancel,
            &|delta| {
                assert!(matches!(delta, StreamDelta::Text { text } if text == "started"));
                seen.store(true, std::sync::atomic::Ordering::Relaxed);
                cancel.cancel();
            },
        ),
    )
    .await
    .unwrap();
    assert!(seen.load(std::sync::atomic::Ordering::Relaxed));
    assert!(matches!(result, Err(ExchangeError::Cancelled)));
}
