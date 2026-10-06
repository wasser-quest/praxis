use super::provider::*;
use async_trait::async_trait;
use crate::gateway::llm::error::ProviderError;

/// pgpu free router provider. Talks to the OpenAI-compatible endpoint at
/// `/free/v1/chat/completions` on the pgpu dashboard port. This bypasses the
/// GPU router slot wake/wait logic entirely.
pub struct FreeRouterProvider {
    api_key: Option<String>,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl FreeRouterProvider {
    pub fn new(api_key: Option<String>, model: String, base_url: String) -> Self {
        // The free router endpoint is at /free/v1 on the pgpu dashboard
        let free_url = format!("{}/free/v1", base_url.trim_end_matches('/'));
        Self {
            api_key,
            model,
            base_url: free_url,
            client: super::http::client(),
        }
    }
}

#[async_trait]
impl LLMProvider for FreeRouterProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let mut body = serde_json::json!({
            "model": request.model.as_deref().unwrap_or(&self.model),
            "messages": request.messages,
            "tools": request.tools,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
        });

        // Add thinking/reasoning_effort if specified (free router passes through to provider)
        if let Some(t) = request.thinking {
            if let Some(level) = t.level() {
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": true,
                    "reasoning_effort": level,
                });
                body["reasoning_effort"] = serde_json::json!(level);
            } else if t == ThinkingMode::Off {
                body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
            }
        }

        let mut req = self.client.post(format!("{}/chat/completions", self.base_url)).json(&body);
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }

        tracing::debug!(?body, "FreeRouter request body");

        let resp = req.send().await.map_err(super::error::ProviderError::from_reqwest)?;
        let status = resp.status();
        let response_text = resp.text().await.unwrap_or_default();
        
        if !status.is_success() {
            tracing::error!(status = %status, body = %response_text, "FreeRouter error response");
        } else {
            tracing::debug!(body = %response_text, "FreeRouter success response");
        }
        
        let data: serde_json::Value = serde_json::from_str(&response_text)?;
        let choice = &data["choices"][0];
        let message = &choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());
        let reasoning_content = message["reasoning_content"].as_str().map(|s| s.to_string());
        let tool_calls = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"].as_str().unwrap_or_default().to_string(),
                        arguments: tc["function"]["arguments"].as_str().unwrap_or_default().to_string(),
                    },
                })
                .collect()
        });

        Ok(ChatResponse {
            reasoning_content,
            content,
            tool_calls,
            finish_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage::openai(&data["usage"]),
        })
    }

    fn name(&self) -> &str {
        "free_router"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        self.client.get(&url).send().await.is_ok()
    }
}