//! llama.cpp server provider (independent package).
//!
//! llama.cpp's built-in HTTP server (`./server`) exposes an OpenAI-compatible
//! chat completions endpoint at `/v1/chat/completions`. This adapter talks to
//! it using the same request/response shape as OpenAI, and is named "llamacpp"
//! so users can select it explicitly with `PRAXIS_REGISTRY_CHAT_PROVIDER=llamacpp`.
use crate::{
    error::{ErrorKind, ProviderError},
    http, provider::*, reduction, stream,
};
use async_trait::async_trait;
use serde_json::Value;

pub struct LlamaCppProvider {
    pub(crate) api_key: Option<String>,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) client: reqwest::Client,
}

impl LlamaCppProvider {
    pub fn new(api_key: Option<String>, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            client: http::client(),
        }
    }

    /// Merge all system-role messages into ONE leading system message.
    ///
    /// llama.cpp chat templates hard-reject a system message anywhere but index
    /// 0 (Qwen3.6 GGUF: `Jinja Exception: System message must be at the
    /// beginning` -> HTTP 500). Praxis injects system-role notices mid-history
    /// (prompt-change notice, compaction summary, omitted-images notice) which
    /// cloud providers tolerate. Non-system messages keep their original
    /// relative order (tool chains stay attached to their tool_calls).
    fn normalize_system_messages(messages: &mut Vec<ChatMessage>) {
        let mut system_texts: Vec<String> = Vec::new();
        let mut system_parts: Option<Vec<ContentPart>> = None;
        let mut rest: Vec<ChatMessage> = Vec::new();
        for message in messages.drain(..) {
            if message.role == "system" {
                if let Some(text) = message.content.as_ref().filter(|text| !text.is_empty()) {
                    system_texts.push(text.clone());
                }
                if system_parts.is_none()
                    && message.content_parts.as_ref().is_some_and(|parts| !parts.is_empty())
                {
                    system_parts = message.content_parts;
                }
            } else {
                rest.push(message);
            }
        }
        if system_texts.is_empty() && system_parts.is_none() {
            *messages = rest;
            return;
        }
        let merged = ChatMessage {
            role: "system".into(),
            content: (!system_texts.is_empty()).then(|| system_texts.join("\n\n")),
            reasoning_content: None,
            content_parts: system_parts,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        rest.insert(0, merged);
        *messages = rest;
    }

    fn request_body(&self, mut request: ChatRequest) -> Value {
        Self::normalize_system_messages(&mut request.messages);
        let model = request.model.as_deref().unwrap_or(&self.model).to_owned();

        let mut messages: Vec<Value> = Vec::new();
        for message in &request.messages {
            let mut msg = serde_json::json!({ "role": message.role });
            if let Some(parts) = message.content_parts.as_ref() {
                if message.role == "tool" {
                    msg["content"] = serde_json::json!(message.content.as_deref().unwrap_or(""));
                    if let Some(tool_call_id) = message.tool_call_id.as_ref() {
                        msg["tool_call_id"] = serde_json::json!(tool_call_id);
                    }
                    messages.push(msg);
                    let mut images: Vec<Value> =
                        Vec::new();
                    images.push(serde_json::json!({
                        "type": "text",
                        "text": "Here is the screenshot from the VM:"
                    }));
                    for part in parts {
                        if let ContentPart::ImageUrl { image_url } = part {
                            images.push(serde_json::json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": image_url.url,
                                    "detail": image_url.detail.as_deref().unwrap_or("auto")
                                }
                            }));
                        }
                    }
                    messages.push(serde_json::json!({ "role": "user", "content": images }));
                    continue;
                }
                let mut arr: Vec<Value> = Vec::new();
                if let Some(text) = message.content.as_ref().filter(|text| !text.is_empty()) {
                    arr.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for part in parts {
                    match part {
                        ContentPart::Text { text } => {
                            arr.push(serde_json::json!({ "type": "text", "text": text }))
                        }
                        ContentPart::ImageUrl { image_url } => arr.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": image_url.url,
                                "detail": image_url.detail.as_deref().unwrap_or("auto")
                            }
                        })),
                    }
                }
                msg["content"] = serde_json::json!(arr);
            } else {
                msg["content"] = serde_json::json!(message.content.as_deref().unwrap_or(""));
            }
            if let Some(tool_calls) = message.tool_calls.as_ref() {
                msg["tool_calls"] = serde_json::json!(tool_calls
                    .iter()
                    .map(|call| serde_json::json!({
                        "id": call.id,
                        // llama-server rejects tool_calls without an explicit type.
                        "type": "function",
                        "function": { "name": call.function.name, "arguments": call.function.arguments }
                    }))
                    .collect::<Vec<_>>());
            }
            if let Some(tool_call_id) = message.tool_call_id.as_ref() {
                msg["tool_call_id"] = serde_json::json!(tool_call_id);
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
        // Thinking levels travel as chat_template_kwargs (enable_thinking +
        // reasoning_effort — the Qwen3 templates know off/low/medium/high/xhigh)
        // and additionally top-level as reasoning_effort: depending on the
        // server build one of the two is read; llama.cpp ignores unknowns.
        match request.thinking {
            Some(thinking) if thinking.level().is_none() => {
                body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
            }
            Some(thinking) => {
                let level = thinking.level();
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": true,
                    "reasoning_effort": level
                });
                body["reasoning_effort"] = serde_json::json!(level);
            }
            None => {}
        }

        body
    }

    /// Only retry confirmed context overflow, before any streaming output.
    /// Each retry changes the request; no tool is executed or replayed here.
    async fn send_fitted(
        &self,
        body: &mut Value,
        omitted: &mut bool,
    ) -> Result<reqwest::Response, ProviderError> {
        for attempt in 0..2 {
            let response = self.post_body(body).send().await.map_err(ProviderError::from_reqwest)?;
            match http::checked(response).await {
                Ok(response) => return Ok(response),
                Err(error) if error.kind == ErrorKind::ContextWindow && attempt == 0 => {
                    let mut changed = false;
                    for _ in 0..reduction::MAX_PASSES {
                        if !reduction::reduce(body) {
                            break;
                        }
                        changed = true;
                    }
                    if !*omitted {
                        if let Some(system) = body["messages"]
                            .as_array_mut()
                            .and_then(|messages| messages.iter_mut().find(|message| message["role"] == "system"))
                        {
                            system["content"] =
                                reduction::budget_system(&reduction::system_text(&system["content"]), reduction::MINIMAL_SYSTEM)
                                    .into();
                            *omitted = true;
                            changed = true;
                        }
                    }
                    if body["max_tokens"].as_u64().is_some_and(|n| n > 128) {
                        body["max_tokens"] = serde_json::json!(128);
                        changed = true;
                    }
                    if changed {
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(ProviderError::new(ErrorKind::ContextWindow))
    }

    fn post_body(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(body);
        if let Some(key) = self.api_key.as_ref().filter(|key| !key.is_empty()) {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        request
    }
}

#[async_trait]
impl crate::provider::LLMProvider for LlamaCppProvider {
    async fn chat_stream_events(
        &self,
        request: ChatRequest,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        let (mut body, mut omitted) = self.fit_body(self.request_body(request)).await?;
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage": true});
        let response = self.send_fitted(&mut body, &mut omitted).await?;
        if omitted {
            on_delta(StreamDelta::TemplateOmitted);
        }
        stream::receive(response, on_delta).await
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let (mut body, mut omitted) = self.fit_body(self.request_body(request)).await?;
        let response = self.send_fitted(&mut body, &mut omitted).await?;
        let data = http::json(response).await?;
        let choice = &data["choices"][0];
        let message = &choice["message"];

        let mut content = message["content"].as_str().map(str::to_owned);
        if omitted && message["tool_calls"].as_array().is_none_or(|calls| calls.is_empty()) {
            if let Some(text) = content.as_mut().filter(|text| !text.is_empty()) {
                text.push_str("\n\n");
                text.push_str(reduction::TEMPLATE_MARKER);
            }
        }
        let tool_calls = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .map(|call| ToolCall {
                    id: call["id"].as_str().unwrap_or_default().to_owned(),
                    function: FunctionCall {
                        name: call["function"]["name"].as_str().unwrap_or_default().to_owned(),
                        arguments: call["function"]["arguments"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    },
                })
                .collect()
        });

        Ok(ChatResponse {
            reasoning_content: message["reasoning_content"].as_str().map(str::to_owned),
            content,
            tool_calls,
            finish_reason: choice["finish_reason"].as_str().map(str::to_owned),
            usage: Usage::openai(&data["usage"]),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let request = match self.api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            Some(key) => self.client.get(&url).bearer_auth(key),
            None => self.client.get(&url),
        };
        let response = request.send().await.map_err(ProviderError::from_reqwest)?;
        let response = http::checked(response).await?;
        let data = http::json(response).await?;
        let mut models: Vec<ModelInfo> = data["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let id = model["id"].as_str().filter(|id| !id.is_empty())?;
                Some(ModelInfo {
                    id: id.to_owned(),
                    label: Some(id.to_owned()),
                })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    fn name(&self) -> &str {
        "llamacpp"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        self.client.get(&url).send().await.is_ok()
    }
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
