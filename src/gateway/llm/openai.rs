//! Transitional native facade preserving kernel branding and historical paths.
//! Request formatting and HTTP implementation live in the independent package.
use super::provider::{ChatRequest, ChatResponse, LLMProvider, ModelInfo};
use async_trait::async_trait;
use praxis_provider_api::ProviderError;

pub struct OpenAIProvider(praxis_provider_openai::OpenAIProvider);

impl OpenAIProvider {
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        Self(praxis_provider_openai::OpenAIProvider::with_client(
            api_key,
            model,
            base_url,
            super::http::client(),
        ))
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.0.chat(request).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.0.list_models().await
    }
    fn name(&self) -> &str {
        self.0.name()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
