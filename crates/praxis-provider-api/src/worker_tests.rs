use super::*;
use crate::{
    transport, CallBinding, ChatAttempt, ChatRequest, ChatResponse, ProviderContinuation,
    ProviderError, RequestContext, StreamDelta, StreamValidator, WireChatRequest,
};
use std::sync::atomic::AtomicUsize;

struct Provider {
    deltas: usize,
    calls: AtomicUsize,
    fail: bool,
}
#[async_trait::async_trait]
impl ChatProvider for Provider {
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        panic!("must use exactly one attempt")
    }
    async fn chat_attempt(
        &self,
        _: ChatRequest,
        _: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        for i in 0..self.deltas {
            on_delta.unwrap()(StreamDelta::Text {
                text: i.to_string(),
            });
        }
        if self.fail {
            return Err(ProviderError::new(crate::ErrorKind::Transport));
        }
        Ok(ChatAttempt {
            response: ChatResponse {
                content: Some("answer".into()),
                reasoning_content: None,
                tool_calls: None,
                finish_reason: None,
                usage: None,
            },
            continuation: None,
        })
    }
    fn name(&self) -> &str {
        "fixture"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
fn request() -> RequestEnvelope {
    RequestEnvelope {
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
    }
}
#[tokio::test]
async fn drains_deltas_before_success_or_safe_error_terminal() {
    for fail in [false, true] {
        let provider = Provider {
            deltas: 2,
            calls: AtomicUsize::new(0),
            fail,
        };
        let request = request();
        let limits = WireLimits::default();
        let (mut reader, writer) = tokio::io::duplex(64);
        let host = async {
            let mut validator = StreamValidator::new(request.context.clone(), limits).unwrap();
            let mut events = vec![];
            while let Some(bytes) = transport::read_frame(&mut reader, limits).await.unwrap() {
                events.push(validator.decode(&bytes).unwrap().event);
            }
            validator.finish().unwrap();
            assert_eq!(events.len(), 3);
            for (i, event) in events[..2].iter().enumerate() {
                assert!(
                    matches!(event, WireStreamEvent::Delta(StreamDelta::Text { text }) if text == &i.to_string())
                );
            }
            assert_eq!(matches!(events[2], WireStreamEvent::Error(_)), fail);
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let (result, ()) = tokio::join!(execute(&provider, &request, writer, limits), host);
            result.unwrap();
        })
        .await
        .unwrap();
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
    }
}
#[tokio::test]
async fn overflow_fails_without_any_successful_terminal_or_retry() {
    let provider = Provider {
        deltas: DELTA_QUEUE_CAPACITY + 1,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let (mut reader, writer) = tokio::io::duplex(64);
    let result = execute(&provider, &request(), writer, WireLimits::default()).await;
    assert!(matches!(result, Err(ServerError::Backpressure)));
    assert!(transport::read_frame(&mut reader, WireLimits::default())
        .await
        .unwrap()
        .is_none());
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

struct PendingProvider {
    dropped: AtomicBool,
    calls: AtomicUsize,
}
struct AttemptGuard<'a>(&'a AtomicBool);
impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
#[async_trait::async_trait]
impl ChatProvider for PendingProvider {
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        panic!("unexpected chat call")
    }
    async fn chat_attempt(
        &self,
        _: ChatRequest,
        _: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let _guard = AttemptGuard(&self.dropped);
        on_delta.unwrap()(StreamDelta::Text {
            text: "started".into(),
        });
        std::future::pending().await
    }
    fn name(&self) -> &str {
        "fixture"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn cancellation_drops_active_attempt_and_closes_without_terminal() {
    let provider = PendingProvider {
        dropped: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    };
    let request = request();
    let limits = WireLimits::default();
    let (mut reader, writer) = tokio::io::duplex(64);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let host = async {
        let mut validator = StreamValidator::new(request.context.clone(), limits).unwrap();
        let bytes = transport::read_frame(&mut reader, limits)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(validator.decode(&bytes).unwrap().event, WireStreamEvent::Delta(StreamDelta::Text { text }) if text == "started")
        );
        stop.send(()).unwrap();
        assert!(transport::read_frame(&mut reader, limits)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            validator.finish(),
            Err(crate::WireValidationError::MissingTerminal)
        );
    };
    let worker = async {
        tokio::select! {
            result = execute(&provider, &request, writer, limits) => panic!("attempt unexpectedly finished: {result:?}"),
            _ = stopped => {},
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(worker, host);
    })
    .await
    .unwrap();
    assert!(provider.dropped.load(Ordering::Relaxed));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn foreign_or_unknown_legacy_continuation_never_calls_provider() {
    let provider = Provider {
        deltas: 0,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let limits = WireLimits::default();
    for foreign in [false, true] {
        let mut request = request();
        let mut binding = request.context.binding.clone();
        if foreign {
            binding.call_id = [9; 16];
        }
        request.continuation = Some(
            crate::WireContinuation::new(binding, b"unknown-format".to_vec(), limits).unwrap(),
        );
        let (mut reader, writer) = tokio::io::duplex(64);
        assert!(matches!(
            execute(&provider, &request, writer, limits).await,
            Err(ServerError::Validation)
        ));
        assert!(transport::read_frame(&mut reader, limits)
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

fn session_config() -> SessionConfig {
    SessionConfig {
        provider_id: "fixture".into(),
        capabilities: crate::WireCapabilities {
            supports_output_limit: false,
            supports_list_models: false,
            supports_embeddings: false,
        },
        limits: WireLimits::default(),
        deadline: std::time::Duration::from_secs(1),
    }
}

#[tokio::test]
async fn session_deadline_cancellation_and_invalid_budget_do_not_call_provider() {
    let provider = Provider {
        deltas: 0,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    for mode in ["cancel", "timeout", "zero", "large"] {
        let mut config = session_config();
        config.deadline = match mode {
            "zero" => std::time::Duration::ZERO,
            "large" => std::time::Duration::from_secs(301),
            _ => std::time::Duration::from_millis(20),
        };
        let cancel = CancellationToken::new();
        if mode == "cancel" {
            cancel.cancel();
        }
        let (host, worker) = tokio::io::duplex(64);
        let (reader, writer) = tokio::io::split(worker);
        let result = serve_once(&provider, &config, reader, writer, &cancel).await;
        match mode {
            "cancel" => assert!(matches!(result, Err(SessionError::Cancelled))),
            "timeout" => assert!(matches!(result, Err(SessionError::Timeout))),
            _ => assert!(matches!(result, Err(SessionError::InvalidDeadline))),
        }
        let mut host = host;
        assert!(transport::read_frame(&mut host, config.limits)
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn session_runs_readiness_request_and_checked_terminal_once() {
    let provider = Provider {
        deltas: 2,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let config = session_config();
    let request = request();
    let (host, worker) = tokio::io::duplex(64);
    let (mut host_reader, mut host_writer) = tokio::io::split(host);
    let (reader, writer) = tokio::io::split(worker);
    let cancel = CancellationToken::new();
    let host = async {
        let identity = crate::handshake::WorkerIdentity {
            provider: "fixture".into(),
            generation: 1,
            nonce: request.context.nonce,
        };
        crate::handshake::negotiate(
            &mut host_reader,
            &mut host_writer,
            identity,
            config.capabilities,
            config.limits,
        )
        .await
        .unwrap();
        transport::write_frame(
            &mut host_writer,
            &request.encode(&request.context, config.limits).unwrap(),
            config.limits,
        )
        .await
        .unwrap();
        let mut validator = StreamValidator::new(request.context.clone(), config.limits).unwrap();
        let mut count = 0;
        while let Some(bytes) = transport::read_frame(&mut host_reader, config.limits)
            .await
            .unwrap()
        {
            validator.decode(&bytes).unwrap();
            count += 1;
        }
        validator.finish().unwrap();
        assert_eq!(count, 3);
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let (result, ()) = tokio::join!(
            serve_once(&provider, &config, reader, writer, &cancel),
            host
        );
        result.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn credential_session_constructs_only_after_checked_grant() {
    for mode in ["success", "foreign", "undeclared", "missing"] {
        let mut config = session_config();
        config.deadline = std::time::Duration::from_millis(100);
        let request = request();
        let names = vec!["api_key".to_owned()];
        let constructed = AtomicUsize::new(0);
        let (host, worker) = tokio::io::duplex(4096);
        let (mut host_reader, mut host_writer) = tokio::io::split(host);
        let (reader, writer) = tokio::io::split(worker);
        let cancel = CancellationToken::new();
        let host = async {
            let identity = crate::handshake::WorkerIdentity {
                provider: "fixture".into(),
                generation: 1,
                nonce: request.context.nonce,
            };
            crate::handshake::negotiate(
                &mut host_reader,
                &mut host_writer,
                identity.clone(),
                config.capabilities,
                config.limits,
            )
            .await
            .unwrap();
            if mode != "missing" {
                let mut target = identity.clone();
                if mode == "foreign" {
                    target.generation += 1;
                }
                let key = if mode == "undeclared" {
                    "other"
                } else {
                    "api_key"
                };
                let values =
                    std::collections::BTreeMap::from([(key.into(), "PRIVATE_TOKEN".into())]);
                let grant =
                    crate::credentials::CredentialGrant::new(target.clone(), &[key.into()], values)
                        .unwrap();
                grant
                    .send(&mut host_writer, &target, config.limits)
                    .await
                    .unwrap();
            }
            if mode == "success" {
                transport::write_frame(
                    &mut host_writer,
                    &request.encode(&request.context, config.limits).unwrap(),
                    config.limits,
                )
                .await
                .unwrap();
            }
            let mut validator =
                StreamValidator::new(request.context.clone(), config.limits).unwrap();
            let mut count = 0;
            while let Some(bytes) = transport::read_frame(&mut host_reader, config.limits)
                .await
                .unwrap()
            {
                validator.decode(&bytes).unwrap();
                count += 1;
            }
            if mode == "success" {
                validator.finish().unwrap();
                assert_eq!(count, 1);
            } else {
                assert_eq!(count, 0);
            }
        };
        let session =
            serve_once_with_credentials(&config, &names, reader, writer, &cancel, |grant| {
                assert_eq!(grant.get("api_key"), Some("PRIVATE_TOKEN"));
                constructed.fetch_add(1, Ordering::Relaxed);
                Ok(Provider {
                    deltas: 0,
                    calls: AtomicUsize::new(0),
                    fail: false,
                })
            });
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(session, host)
        })
        .await
        .unwrap();
        match mode {
            "success" => {
                result.unwrap();
                assert_eq!(constructed.load(Ordering::Relaxed), 1);
            }
            "missing" => assert!(matches!(result, Err(SessionError::Timeout))),
            _ => assert!(matches!(result, Err(SessionError::Credentials))),
        }
        if mode != "success" {
            assert_eq!(constructed.load(Ordering::Relaxed), 0);
        }
    }
}
