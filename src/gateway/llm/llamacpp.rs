use super::provider::*;
use async_trait::async_trait;
use crate::gateway::llm::error::ProviderError;

/// llama.cpp server provider.
///
/// llama.cpp's built-in HTTP server (`./server`) exposes an OpenAI-compatible
/// chat completions endpoint at `/v1/chat/completions`. This provider talks
/// to it using the same request/response shape as OpenAI, but is named
/// "llamacpp" so users can select it explicitly with `USE_PROVIDER=llamacpp`.
pub struct LlamaCppProvider {
    api_key: Option<String>,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl LlamaCppProvider {
    pub fn new(api_key: Option<String>, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            client: super::http::client(),
        }
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
    for m in messages.drain(..) {
        if m.role == "system" {
            if let Some(t) = m.content.as_ref() {
                if !t.is_empty() {
                    system_texts.push(t.clone());
                }
            }
            if system_parts.is_none() && m.content_parts.as_ref().is_some_and(|p| !p.is_empty()) {
                system_parts = m.content_parts;
            }
        } else {
            rest.push(m);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: Some(content.into()),
            reasoning_content: None,
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    #[test]
    fn mid_history_system_messages_merge_into_leading_slot() {
        // Reproduces the 2026-09-20 GPU-router incident: Praxis sent a
        // system notice mid-history and Qwen3.6's template rejected with 500.
        let mut msgs = vec![
            msg("system", "Du bist ein Assistent."),
            msg("user", "Frage 1"),
            msg("assistant", "Antwort 1"),
            msg("system", "nachträgliche Notiz"),
            msg("user", "Frage 2"),
        ];
        normalize_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content.as_deref(), Some("Du bist ein Assistent.\n\nnachträgliche Notiz"));
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content.as_deref(), Some("Frage 1"));
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[3].content.as_deref(), Some("Frage 2"));
    }

    #[test]
    fn no_system_messages_stays_untouched() {
        let mut msgs = vec![msg("user", "hi"), msg("assistant", "hey")];
        normalize_system_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn tool_chain_order_is_preserved() {
        let mut msgs = vec![
            msg("system", "sys"),
            msg("user", "q"),
            msg("assistant", "a"),
            msg("system", "notice"),
            msg("tool", "tool-result"),
            msg("user", "next"),
        ];
        normalize_system_messages(&mut msgs);
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "tool", "user"]);
    }
}

#[path = "llamacpp_stream.rs"]
mod stream;
#[path = "llamacpp_budget.rs"]
mod budget;

impl LlamaCppProvider {
    fn request_body(&self, request: ChatRequest) -> serde_json::Value {

        // llama.cpp chat templates (e.g. Qwen3.6 GGUF) hard-reject system
        // messages that are not the FIRST message: 'Jinja Exception: System
        // message must be at the beginning' -> HTTP 500. Praxis legitimately
        // injects system-role notices mid-history (prompt-change notice,
        // compaction summary, omitted-history-images notice), which cloud
        // providers tolerate. Normalize for llama.cpp: merge all system
        // messages into a single leading system message, original order kept.
        let mut request = request;
        normalize_system_messages(&mut request.messages);
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
        // Qwen3.8 thinking levels: llama.cpp nimmt Stufen über
        // chat_template_kwargs (enable_thinking + reasoning_effort — das
        // Qwen3.8-Template kennt off/low/medium/high/xhigh). Der Level geht
        // zusätzlich top-level als reasoning_effort raus (je nach Server-
        // Build wird eines von beiden gelesen; llama.cpp ignoriert Unknowns).
        match request.thinking {
            Some(t) if t.level().is_none() => {
                body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
            }
            Some(t) => {
                body["chat_template_kwargs"] = serde_json::json!({
                    "enable_thinking": true,
                    "reasoning_effort": t.level(),
                });
                body["reasoning_effort"] = serde_json::json!(t.level());
            }
            None => {}
        }

        body
    }

    fn post_body(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut req = self.client.post(format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'))).json(body);

        // GPU-Router (pgpu) vor dem LLM-Proxy: `X-Router-Wait` hält die
        // Verbindung auf kaltem Slot bis healthy (impliziter Wake), statt
        // sofort mit 503 zu antworten. 20.09.: ohne den Header failte der
        // erste Chat nach Idle-Stopp 5× am Retry, weil der 1–2-min-Neustart
        // länger dauerte als Backoff+Budget. Harmlos gegen jeden anderen
        // Server; ohne GPU_ROUTER_URL bleibt wait_s 0 und der Header bleibt weg.
        let router_wait = crate::gpu_router::wait_s();
        if router_wait > 0 {
            req = req.header(crate::gpu_router::HEADER_WAIT, router_wait);
        }

        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }

        req
    }
}

#[async_trait]
impl LLMProvider for LlamaCppProvider {
    async fn chat_stream_events(&self, request: ChatRequest, on_delta: &(dyn Fn(StreamDelta) + Send + Sync)) -> Result<ChatResponse, ProviderError> {
        let (mut body, omitted) = self.fit_body(self.request_body(request)).await?;
        if omitted { on_delta(StreamDelta::TemplateOmitted); }
        body["stream"] = serde_json::json!(true);
        body["stream_options"] = serde_json::json!({"include_usage":true});
        let response = self.post_body(&body).send().await.map_err(super::error::ProviderError::from_reqwest)?;
        stream::receive(response, on_delta).await
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let (body, omitted) = self.fit_body(self.request_body(request)).await?;
        let resp = self.post_body(&body).send().await.map_err(super::error::ProviderError::from_reqwest)?;
        let data = super::http::json(resp).await?;
        let choice = &data["choices"][0];
        let message = &choice["message"];

        let mut content = message["content"].as_str().map(|s| s.to_string());
        if omitted && message["tool_calls"].as_array().is_none_or(|c| c.is_empty()) {
            if let Some(text) = content.as_mut().filter(|t|!t.is_empty()) {
                text.push_str("\n\n"); text.push_str(budget::TEMPLATE_MARKER);
            }
        }
        let reasoning_content = message["reasoning_content"].as_str().map(|s| s.to_string());
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
            reasoning_content: reasoning_content,
            content,
            tool_calls,
            finish_reason: choice["finish_reason"].as_str().map(|s| s.to_string()),
            usage: Usage::openai(&data["usage"]),
        })
    }

    async fn list_models(&self) -> Result<Vec<super::provider::ModelInfo>, ProviderError> {
        let url = format!("{}/v1/models", self.base_url);
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.api_key {
            if !key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }
        let resp = req.send().await.map_err(ProviderError::from_reqwest)?;
        let resp = super::http::checked(resp).await?;
        let data: serde_json::Value = resp.json().await?;
        let mut models: Vec<super::provider::ModelInfo> = data["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let id = model["id"].as_str().filter(|id| !id.is_empty())?;
                Some(super::provider::ModelInfo {
                    id: id.to_string(),
                    label: model["id"].as_str().map(str::to_string),
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
        let url = format!("{}/v1/models", self.base_url);
        self.client.get(&url).send().await.is_ok()
    }
}
