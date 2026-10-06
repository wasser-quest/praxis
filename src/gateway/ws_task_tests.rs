//! End-to-end local WS control/feedback tests. Only POML rendering is real.
use crate::gateway::{
    llm::{
        error::{ErrorKind, ProviderError},
        provider::*,
        resilience::ResilienceConfig,
        LLMRouter,
    },
    GatewayState,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_tungstenite::tungstenite::Message;

struct TestProvider {
    wait: bool,
    calls: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl LLMProvider for TestProvider {
    fn name(&self) -> &str {
        "synthetic-ws"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if self.wait {
            std::future::pending::<()>().await;
        }
        if attempt == 0 {
            return Err(ProviderError::new(ErrorKind::Unavailable).into());
        }
        Ok(ChatResponse {
    reasoning_content: None,
            content: Some("done".into()),
            tool_calls: None,
            finish_reason: None,
            usage: None,
        })
    }
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; synthetic local WebSocket server and mock LLM only"]
async fn resilience_websocket_forwards_retries_and_accepts_ping_stop_while_busy() {
    assert!(std::env::var("POML_CLI").is_ok());
    for wait in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let user = format!("resilience-ws-{wait}");
        let mut ctx = db.load_context(&user).unwrap();
        ctx.settings.provider = Some("synthetic-ws".into());
        ctx.settings.use_tts = false;
        db.save_context(&ctx).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let llm = LLMRouter::with_providers(
            vec![Box::new(TestProvider {
                wait,
                calls: calls.clone(),
                started: started.clone(),
            })],
            "synthetic-ws".into(),
            vec![],
            ResilienceConfig {
                initial_backoff_ms: 20,
                max_backoff_ms: 20,
                ..Default::default()
            },
        );
        let state = GatewayState {
            db,
            config: crate::config::Config::from_env(),
            secrets: Default::default(),
            llm: crate::gateway::LlmHandle::new(llm),
            plugins: Arc::new(crate::plugins::PluginRegistry::new()),
            event_tx: tokio::sync::broadcast::channel(16).0,
            start_time: std::time::Instant::now(),
        };
        let app = axum::Router::new()
            .route("/ws", axum::routing::get(super::ws_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let signal = shutdown.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(signal.cancelled_owned())
                .await
                .unwrap()
        });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        socket.send(Message::Text(serde_json::json!({"type":"message","user_id":user,"content":"synthetic offline test","channel_id":"web"}).to_string())).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), started.notified())
            .await
            .unwrap();
        if wait {
            socket
                .send(Message::Text(r#"{"type":"ping"}"#.into()))
                .await
                .unwrap();
        }
        let mut retried = false;
        let mut pong = false;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(frame) = socket.next().await {
                let Message::Text(text) = frame.unwrap() else {
                    continue;
                };
                let event: serde_json::Value = serde_json::from_str(&text).unwrap();
                match event["type"].as_str().unwrap() {
                    "feedback" => retried |= event["content"].as_str().unwrap().contains("retry"),
                    "pong" => {
                        pong = true;
                        socket
                            .send(Message::Text(
                                serde_json::json!({"type":"stop","user_id":user}).to_string(),
                            ))
                            .await
                            .unwrap();
                    }
                    "response" => {
                        assert!(!wait);
                        assert_eq!(event["content"], "done");
                        break;
                    }
                    "error" => {
                        assert!(wait);
                        assert!(event["message"].as_str().unwrap().contains("cancelled"));
                        break;
                    }
                    other => panic!("unexpected WS event {other}"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(pong, wait);
        assert_eq!(retried, !wait);
        assert_eq!(calls.load(Ordering::SeqCst), if wait { 1 } else { 2 });
        socket.close(None).await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert!(crate::gateway::task_control::cancellation(&user).is_none());
    }
}
