use super::provider::*;
use async_trait::async_trait;

pub struct OpenRouterProvider {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            client: super::http::client(),
        }
    }
}

#[async_trait]
impl LLMProvider for OpenRouterProvider {
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        let model = request.model.as_deref().unwrap_or(&self.model);

        let mut messages: Vec<serde_json::Value> = Vec::new();
        for m in &request.messages {
            let mut msg = serde_json::json!({ "role": m.role });
            if let Some(ref parts) = m.content_parts {
                if m.role == "tool" {
                    msg["content"] = serde_json::json!(m.content.as_deref().unwrap_or(""));
                    if let Some(ref tcid) = m.tool_call_id {
                        msg["tool_call_id"] = serde_json::json!(tcid);
                    }
                    messages.push(msg);
                    let mut img_arr: Vec<serde_json::Value> = Vec::new();
                    img_arr.push(serde_json::json!({ "type": "text", "text": "Here is the screenshot from the VM:" }));
                    for p in parts {
                        if let super::provider::ContentPart::ImageUrl { image_url } = p {
                            img_arr.push(serde_json::json!({ "type": "image_url", "image_url": { "url": image_url.url, "detail": image_url.detail.as_deref().unwrap_or("auto") } }));
                        }
                    }
                    messages.push(serde_json::json!({ "role": "user", "content": img_arr }));
                    continue;
                } else {
                    let mut arr: Vec<serde_json::Value> = Vec::new();
                    if let Some(ref text) = m.content {
                        if !text.is_empty() {
                            arr.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    for p in parts {
                        match p {
                            super::provider::ContentPart::Text { text } => arr.push(serde_json::json!({ "type": "text", "text": text })),
                            super::provider::ContentPart::ImageUrl { image_url } => arr.push(serde_json::json!({ "type": "image_url", "image_url": { "url": image_url.url, "detail": image_url.detail.as_deref().unwrap_or("auto") } })),
                        }
                    }
                    msg["content"] = serde_json::json!(arr);
                }
            } else {
                msg["content"] = serde_json::json!(m.content.as_deref().unwrap_or(""));
            }
            if let Some(ref tool_calls) = m.tool_calls {
                let calls: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.function.name, "arguments": tc.function.arguments }
                    })
                }).collect();
                msg["tool_calls"] = serde_json::json!(calls);
            }
            if let Some(ref tcid) = m.tool_call_id {
                msg["tool_call_id"] = serde_json::json!(tcid);
            }
            messages.push(msg);
        }

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "tools": request.tools,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
        });
        // Extended reasoning: OpenRouter normalizes `reasoning.exclude` (hide
        // the trace) and `reasoning.enabled` for capable models.
        match request.thinking {
            Some(t) if t.level().is_none() => {
                body["reasoning"] = serde_json::json!({ "exclude": true, "enabled": false });
            }
            Some(t) => {
                body["reasoning"] = serde_json::json!({ "enabled": true, "effort": t.level() });
            }
            None => {}
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .headers(crate::branding::openrouter_headers())
            .json(&body)
            .send()
            .await.map_err(super::error::ProviderError::from_reqwest)?;

        let data = super::http::json(resp).await?;
        let choice = &data["choices"][0];
        let message = &choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    },
                })
                .collect()
        });

        Ok(ChatResponse {
            reasoning_content: None,
            content,
            tool_calls,
            finish_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage::openai(&data["usage"]),
        })
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .headers(crate::branding::openrouter_headers())
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}
