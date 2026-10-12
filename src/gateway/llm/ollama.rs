//! Native facade preserving Praxis attribution and the historical Ollama API.
//! HTTP transport, streaming and context budgeting live in the independent package.
use super::provider::*;
use async_trait::async_trait;
use praxis_provider_api::ProviderError;

#[cfg(test)]
#[path = "ollama_output_limit_tests.rs"]
mod output_limit_tests;

pub struct OllamaProvider(praxis_provider_ollama::OllamaProvider);

impl OllamaProvider {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self(praxis_provider_ollama::OllamaProvider::with_client(
            base_url,
            model,
            api_key,
            super::http::client(),
        ))
    }

    pub fn num_ctx(mut self, num_ctx: Option<u64>) -> Self {
        self.0 = self.0.num_ctx(num_ctx);
        self
    }
}

// Keep existing native regression fixtures' explicit no-proxy client injection.
#[cfg(test)]
impl std::ops::Deref for OllamaProvider {
    type Target = praxis_provider_ollama::OllamaProvider;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
#[cfg(test)]
impl std::ops::DerefMut for OllamaProvider {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    async fn chat_attempt(
        &self,
        request: ChatRequest,
        continuation: Option<&ProviderContinuation>,
        on_delta: Option<&(dyn Fn(StreamDelta) + Send + Sync)>,
    ) -> Result<ChatAttempt, ProviderError> {
        self.0.chat_attempt(request, continuation, on_delta).await
    }
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.0.chat(request).await
    }
    async fn chat_stream(
        &self,
        request: ChatRequest,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        self.0.chat_stream(request, on_token).await
    }
    async fn chat_stream_events(
        &self,
        request: ChatRequest,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        self.0.chat_stream_events(request, on_delta).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.0.list_models().await
    }
    async fn health_check(&self) -> bool {
        self.0.health_check().await
    }
    fn name(&self) -> &str {
        self.0.name()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod facade_tests {
    use super::*;
    #[tokio::test]
    async fn native_ollama_keeps_branding_identity_and_url() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/prefix/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":[]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let provider = OllamaProvider::new(
            format!("{}/prefix///", server.uri()),
            "fixture".into(),
            Some("test-only".into()),
        );
        assert_eq!(provider.name(), "ollama");
        assert!(provider.as_any().is::<OllamaProvider>());
        assert!(provider.list_models().await.unwrap().is_empty());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].headers["authorization"], "Bearer test-only");
        assert!(requests[0].headers["user-agent"]
            .to_str()
            .unwrap()
            .contains("Praxis"));
    }
}
