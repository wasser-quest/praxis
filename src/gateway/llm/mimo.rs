use super::provider::*;
use crate::config::ApiMode;
use async_trait::async_trait;
use crate::gateway::llm::error::ProviderError;

pub struct MiMoProvider {
    api_key: String,
    model: String,
    base_url: String,
    api_mode: ApiMode,
    client: reqwest::Client,
}

impl MiMoProvider {
    pub fn new(api_key: String, model: String, base_url: String, api_mode: ApiMode) -> Self {
        Self {
            api_key,
            model,
            base_url,
            api_mode,
            client: super::http::client(),
        }
    }
}

#[async_trait]
impl LLMProvider for MiMoProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        match self.api_mode {
            ApiMode::Anthropic => self.chat_anthropic(request).await,
            ApiMode::OpenAI => self.chat_openai(request).await,
        }
    }

    fn name(&self) -> &str {
        "mimo"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl MiMoProvider {
    async fn chat_openai(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);

        // Log content_parts presence for debugging
        for (i, m) in request.messages.iter().enumerate() {
            if m.content_parts.is_some() {
                let count = m.content_parts.as_ref().map(|p| p.len()).unwrap_or(0);
                tracing::info!(
                    "MiMo msg[{}] role={} has {} content_parts",
                    i,
                    m.role,
                    count
                );
            }
        }

        let mut messages: Vec<serde_json::Value> = Vec::new();
        for m in &request.messages {
            let mut msg = serde_json::json!({ "role": m.role });

            if let Some(ref parts) = m.content_parts {
                if m.role == "tool" {
                    // MiMo ignores images in tool messages.
                    // Send text as tool message, then image as separate user message.
                    msg["content"] = serde_json::json!(m.content.as_deref().unwrap_or(""));
                    if let Some(ref tcid) = m.tool_call_id {
                        msg["tool_call_id"] = serde_json::json!(tcid);
                    }
                    messages.push(msg);

                    // Add image as user message
                    let mut img_arr: Vec<serde_json::Value> = Vec::new();
                    img_arr.push(serde_json::json!({ "type": "text", "text": "Here is the screenshot from the VM:" }));
                    for p in parts {
                        if let super::provider::ContentPart::ImageUrl { image_url } = p {
                            img_arr.push(serde_json::json!({ "type": "image_url", "image_url": { "url": image_url.url } }));
                        }
                    }
                    messages.push(serde_json::json!({ "role": "user", "content": img_arr }));
                    continue;
                } else {
                    // User/assistant message with images - use array format
                    let mut arr: Vec<serde_json::Value> = Vec::new();
                    if let Some(ref text) = m.content {
                        if !text.is_empty() {
                            arr.push(serde_json::json!({ "type": "text", "text": text }));
                        }
                    }
                    for p in parts {
                        match p {
                            super::provider::ContentPart::Text { text } => arr.push(serde_json::json!({ "type": "text", "text": text })),
                            super::provider::ContentPart::ImageUrl { image_url } => arr.push(serde_json::json!({ "type": "image_url", "image_url": { "url": image_url.url } })),
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

        let model = request.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
        });

        tracing::debug!(message_count = messages.len(), "MiMo request prepared");

        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::json!(tools);
        }

        if let Some(max_tokens) = request.max_tokens {
            body["max_completion_tokens"] = serde_json::json!(max_tokens);
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await.map_err(super::error::ProviderError::from_reqwest)?;

        let data = super::http::json(resp).await?;
        let choice = &data["choices"][0];
        let message = &choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls = parse_openai_tool_calls(message);

        Ok(ChatResponse {
            reasoning_content: None,
            content,
            tool_calls,
            finish_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage::openai(&data["usage"]),
        })
    }

    async fn chat_anthropic(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);

        let (system_prompt, messages) =
            super::anthropic::build_anthropic_messages(&request.messages);

        let model = request.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        if !system_prompt.is_empty() {
            body["system"] = serde_json::json!(system_prompt);
        }

        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::json!(super::anthropic::build_anthropic_tools(tools));
        }

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await.map_err(super::error::ProviderError::from_reqwest)?;

        let data = super::http::json(resp).await?;
        super::anthropic::parse_anthropic_response(&data)
    }
}

fn parse_openai_tool_calls(message: &serde_json::Value) -> Option<Vec<ToolCall>> {
    message["tool_calls"].as_array().map(|calls| {
        calls
            .iter()
            .filter_map(|tc| {
                Some(ToolCall {
                    id: tc["id"].as_str().unwrap_or("call_0").to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"].as_str()?.to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("{}")
                            .to_string(),
                    },
                })
            })
            .collect()
    })
}
