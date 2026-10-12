//! Transitional native facade preserving kernel branding and historical paths.
//! Request formatting and HTTP implementation live in the independent package.
use super::provider::{ChatRequest, ChatResponse, LLMProvider};
use async_trait::async_trait;
pub use praxis_provider_anthropic::{
    build_anthropic_messages, build_anthropic_tools, parse_anthropic_response,
};
use praxis_provider_api::ProviderError;

pub struct AnthropicProvider(praxis_provider_anthropic::AnthropicProvider);

impl AnthropicProvider {
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        Self(praxis_provider_anthropic::AnthropicProvider::with_client(
            api_key,
            model,
            base_url,
            super::http::client(),
        ))
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.0.chat(request).await
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
    async fn native_anthropic_keeps_branding_identity_and_url() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prefix/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content":[{"type":"text","text":"answer"}],"stop_reason":"end_turn"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = AnthropicProvider::new(
            "test-only".into(),
            "fixture".into(),
            format!("{}/prefix", server.uri()),
        );
        assert_eq!(provider.name(), "anthropic");
        assert!(provider.as_any().is::<AnthropicProvider>());
        assert!(provider.supports_output_limit());
        assert_eq!(
            provider.list_models().await.unwrap_err().kind,
            praxis_provider_api::ErrorKind::Unsupported
        );
        let request = praxis_provider_api::WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: Some(200),
            model: None,
            thinking: None,
        };
        assert_eq!(
            provider
                .chat(request.into())
                .await
                .unwrap()
                .content
                .as_deref(),
            Some("answer")
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].headers["x-api-key"], "test-only");
        assert!(requests[0].headers["user-agent"]
            .to_str()
            .unwrap()
            .contains("Praxis"));
        assert!(!requests[0].headers.contains_key("http-referer"));
        assert!(!requests[0].headers.contains_key("x-title"));
    }
}
