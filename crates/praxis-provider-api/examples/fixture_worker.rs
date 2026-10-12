//! Protocol fixture only: never installed as a provider or used as a fallback.
//! stdout is exclusively framed protocol data; diagnostics never echo requests.
use praxis_provider_api::{
    worker, ChatAttempt, ChatProvider, ChatRequest, ChatResponse, ErrorKind, ProviderContinuation,
    ProviderError, StreamDelta, WireCapabilities, WireLimits,
};

struct FixtureProvider {
    mode: String,
}
#[async_trait::async_trait]
impl ChatProvider for FixtureProvider {
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        unreachable!("fixture uses one attempt")
    }
    async fn chat_attempt(
        &self,
        _: ChatRequest,
        _: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        let count = if self.mode == "overflow" {
            worker::DELTA_QUEUE_CAPACITY + 1
        } else {
            2
        };
        for i in 0..count {
            on_delta.unwrap()(StreamDelta::Text {
                text: i.to_string(),
            });
        }
        if self.mode == "error" {
            return Err(ProviderError::new(ErrorKind::Transport));
        }
        Ok(ChatAttempt {
            response: ChatResponse {
                content: Some("fixture answer".into()),
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if run().await.is_err() {
        eprintln!("fixture worker protocol failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mode = match args.as_slice() {
        [] => "success",
        [mode] if matches!(mode.as_str(), "success" | "error" | "overflow") => mode.as_str(),
        _ => return Err("invalid fixture configuration".into()),
    };
    let limits = WireLimits::default();
    let capabilities = WireCapabilities {
        supports_output_limit: false,
        supports_list_models: false,
        supports_embeddings: false,
    };
    let provider = FixtureProvider { mode: mode.into() };
    let config = worker::SessionConfig {
        provider_id: "fixture".into(),
        capabilities,
        limits,
        deadline: std::time::Duration::from_secs(5),
    };
    worker::serve_once(
        &provider,
        &config,
        tokio::io::stdin(),
        tokio::io::stdout(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    Ok(())
}
