//! The common provider interfaces. Adapters report what happened; every
//! policy — budgets, rate limits, backoff, compaction and fallback — stays
//! host-owned, and a retry never replays a committed tool action.
use crate::chat::{ChatAttempt, ChatRequest, ChatResponse, ProviderContinuation, StreamDelta};
use crate::errors::ProviderError;
use async_trait::async_trait;

/// A model a provider can serve. Host UIs choose; providers only enumerate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    /// Display label when the provider knows one (e.g. "qwen3:8b").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Streaming chat with usage accounting. Adapters without native streaming
/// return one complete response; retry, cancellation and rate-limit policy are
/// identical for both paths and are applied by the host.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError>;
    async fn chat_stream(
        &self,
        request: ChatRequest,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        let _ = on_token;
        self.chat(request).await
    }
    async fn chat_stream_events(
        &self,
        request: ChatRequest,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        self.chat_stream(request, &|text| on_delta(StreamDelta::Text { text }))
            .await
    }
    /// One network attempt, not necessarily the end of a logical call. The
    /// host owns continuation budgets, pacing and cancellation; adapters must
    /// not hide additional inference requests inside a single attempt.
    async fn chat_attempt(
        &self,
        request: ChatRequest,
        _continuation: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        let response = match on_delta {
            Some(on_delta) => self.chat_stream_events(request, on_delta).await?,
            None => self.chat(request).await?,
        };
        Ok(ChatAttempt {
            response,
            continuation: None,
        })
    }
    /// Models this provider can serve. `Unsupported` when it cannot enumerate.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Err(ProviderError::with_cause(
            crate::errors::ErrorKind::Unsupported,
            "this provider does not enumerate models",
        ))
    }
    /// Whether increasing `ChatRequest.max_tokens` changes the wire request.
    /// Subscription endpoints with a fixed output budget must not be replayed
    /// unchanged by the host's output-limit recovery.
    fn supports_output_limit(&self) -> bool {
        true
    }
    fn name(&self) -> &str;
    fn as_any(&self) -> &dyn std::any::Any;
    async fn health_check(&self) -> bool {
        true
    }
}

/// Vector embeddings for retrieval. Batched calls may fall back to one-by-one
/// inside the adapter; the host does not retry partial batches.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ProviderError>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
}
