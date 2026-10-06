use super::provider::*;
use super::error::{ErrorKind, ProviderError};
use async_trait::async_trait;

#[cfg(test)]
#[path = "ollama_stream_tests.rs"]
mod voice_stream_tests;

#[cfg(test)]
#[path = "ollama_output_limit_tests.rs"]
mod output_limit_tests;

pub struct OllamaProvider {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            model,
            api_key,
            client: super::http::client(),
        }
    }

    pub async fn chat_streaming(
        &self,
        request: ChatRequest,
        on_token: impl Fn(String),
    ) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/api/chat", self.base_url);
        let messages = build_ollama_messages(&request);
        let model = request.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        add_tools(&mut body, &request);
        // Ollama-API: think = false | true | "low"|"medium"|"high"|"xhigh"
        // (Stufen-Strings durchlaufen den Fork bis ins Qwen3.8-Template).
        if let Some(t) = request.thinking {
            body["think"] = match t.level() {
                Some(lvl) => serde_json::json!(lvl),
                None => serde_json::json!(false),
            };
        }

        tracing::debug!(message_count = messages.len(), "Ollama streaming request prepared");

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }
        let resp = req.send().await.map_err(ProviderError::from_reqwest)?;
        let resp = super::http::checked(resp).await?;

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut state = OllamaStreamState::default();
        tracing::info!("[STREAM] Ollama byte stream started");

        while let Some(chunk) = stream.next().await {
            state.push(&chunk.map_err(ProviderError::from_reqwest)?, &on_token)?;
            if state.done {
                break;
            }
        }
        state.finish(&on_token)?;

        tracing::info!(content_bytes = state.content.len(), tool_call_count = state.tool_calls.len(), thinking_bytes = state.thinking_bytes, done_reason = state.done_reason.unwrap_or("unspecified"), completion_tokens = ?state.usage.as_ref().map(|u| u.completion_tokens), "[STREAM] Ollama response collected");
        check_completion(state.done_reason)?;
        let has_tools = !state.tool_calls.is_empty();
        let think_usable = !state.thinking.trim().is_empty();
        let reasoning = (state.content.is_empty() && think_usable)
            .then(|| state.thinking.trim().to_string());
        let content = if !state.content.is_empty() {
            Some(state.content)
        } else if think_usable {
            // Think phase consumed the budget without producing an answer:
            // surface the reasoning so the turn is usable.
            Some(format!("[Denkspur]
{}", state.thinking.trim()))
        } else {
            None
        };
        Ok(ChatResponse {
            reasoning_content: reasoning,
            content,
            tool_calls: if has_tools { Some(state.tool_calls) } else { None },
            finish_reason: Some(if has_tools { "tool_calls" } else { "stop" }.to_string()),
            usage: state.usage,
        })
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    async fn chat_stream(&self, request: ChatRequest, on_token: &(dyn Fn(String) + Send + Sync)) -> Result<ChatResponse, ProviderError> {
        self.chat_streaming(request, on_token).await
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/api/chat", self.base_url);

        let messages = build_ollama_messages(&request);
        let model = request.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        add_tools(&mut body, &request);
        // Ollama-API: think = false | true | "low"|"medium"|"high"|"xhigh"
        // (Stufen-Strings durchlaufen den Fork bis ins Qwen3.8-Template).
        if let Some(t) = request.thinking {
            body["think"] = match t.level() {
                Some(lvl) => serde_json::json!(lvl),
                None => serde_json::json!(false),
            };
        }

        tracing::debug!(message_count = messages.len(), "Ollama request prepared");

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }
        let resp = req.send().await.map_err(ProviderError::from_reqwest)?;
        let data = super::http::json(resp).await?;
        parse_ollama_response(&data)
    }

    async fn list_models(&self) -> Result<Vec<super::provider::ModelInfo>, ProviderError> {
        let url = format!("{}/api/tags", self.base_url);
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }
        let resp = req.send().await.map_err(ProviderError::from_reqwest)?;
        let resp = super::http::checked(resp).await?;
        let data: serde_json::Value = resp.json().await?;
        let mut models: Vec<super::provider::ModelInfo> = data["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let id = model["name"].as_str().filter(|id| !id.is_empty())?;
                Some(super::provider::ModelInfo {
                    id: id.to_string(),
                    label: Some(id.to_string()),
                })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let mut request = self.client.get(&url).timeout(std::time::Duration::from_secs(10));
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.is_empty()) {
            request = request.bearer_auth(key);
        }
        request.send().await.map(|response| response.status().is_success()).unwrap_or(false)
    }
}

fn build_ollama_messages(request: &ChatRequest) -> Vec<serde_json::Value> {
    request.messages.iter().map(|m| {
        let mut msg = serde_json::json!({
            "role": m.role,
            "content": m.content.as_deref().unwrap_or("")
        });
        if let Some(ref parts) = m.content_parts {
            let mut images: Vec<String> = Vec::new();
            for p in parts {
                if let ContentPart::ImageUrl { image_url } = p {
                    if let Some(b64) = image_url.url.strip_prefix("data:").and_then(|s| s.find(",").map(|i| &s[i+1..])) {
                        images.push(b64.to_string());
                    } else if let Some(path) = image_url.url.strip_prefix("file://") {
                        if let Ok(bytes) = std::fs::read(path) {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                            images.push(b64);
                        }
                    } else if !image_url.url.starts_with("http") {
                        if let Ok(bytes) = std::fs::read(&image_url.url) {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                            images.push(b64);
                        }
                    }
                }
            }
            if !images.is_empty() {
                msg["images"] = serde_json::json!(images);
            }
        }
        if let Some(ref tool_calls) = m.tool_calls {
            let ollama_calls: Vec<serde_json::Value> = tool_calls.iter().map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.function.name,
                        "arguments": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or_default()
                    }
                })
            }).collect();
            msg["tool_calls"] = serde_json::json!(ollama_calls);
        }
        if m.role == "tool" {
            if let Some(ref tool_name) = m.tool_name {
                msg["tool_name"] = serde_json::json!(tool_name);
            }
        }
        msg
    }).collect()
}

fn add_tools(body: &mut serde_json::Value, request: &ChatRequest) {
    // Honor the output bound used by token reservations on both Ollama paths.
    let mut options = serde_json::Map::new();
    if let Some(max_tokens) = request.max_tokens {
        options.insert("num_predict".into(), serde_json::json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        options.insert("temperature".into(), serde_json::json!(temperature));
    }
    if !options.is_empty() { body["options"] = serde_json::Value::Object(options); }
    if let Some(ref tools) = request.tools {
        let ollama_tools: Vec<serde_json::Value> = tools.iter().map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters
                }
            })
        }).collect();
        body["tools"] = serde_json::json!(ollama_tools);
    }
}

#[derive(Default)]
struct OllamaStreamState {
    buffer: Vec<u8>,
    content: String,
    // Count thinking for diagnostics, but never retain, emit or speak it.
    thinking_bytes: usize,
    thinking: String,
    tool_calls: Vec<ToolCall>,
    done: bool,
    done_reason: Option<&'static str>,
    usage: Option<Usage>,
}

impl OllamaStreamState {
    fn push(&mut self, bytes: &[u8], on_token: &impl Fn(String)) -> anyhow::Result<()> {
        if self.done {
            return Ok(());
        }
        // Keep raw bytes until a complete NDJSON record is available. Decoding
        // each network chunk separately corrupts split French/Japanese UTF-8.
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            self.process_line(std::str::from_utf8(&line).map_err(|_| ProviderError::new(ErrorKind::InvalidResponse))?, on_token)?;
            if self.done {
                self.buffer.clear();
                break;
            }
        }
        Ok(())
    }

    fn finish(&mut self, on_token: &impl Fn(String)) -> anyhow::Result<()> {
        if !self.done && !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.process_line(std::str::from_utf8(&line).map_err(|_| ProviderError::new(ErrorKind::InvalidResponse))?, on_token)?;
        }
        if !self.done {
            return Err(ProviderError { cause: Some("missing final done record"), ..ProviderError::new(ErrorKind::Interrupted) }.into());
        }
        Ok(())
    }

    fn process_line(&mut self, line: &str, on_token: &impl Fn(String)) -> anyhow::Result<()> {
        if line.trim().is_empty() {
            return Ok(());
        }
        let data: serde_json::Value = serde_json::from_str(line).map_err(|_| ProviderError::new(ErrorKind::InvalidResponse))?;
        if data.get("error").is_some_and(|error| !error.is_null()) {
            return Err(super::http::payload_error(200, &data).into());
        }
        let done = data.get("done").and_then(|value| value.as_bool()).unwrap_or(false);
        if done {
            self.usage = parse_usage(&data);
            self.done_reason = parse_done_reason(&data);
        }
        let message = &data["message"];
        if let Some(thinking_delta) = message["thinking"].as_str() {
            self.thinking.push_str(thinking_delta);
            self.thinking_bytes = self.thinking_bytes.saturating_add(thinking_delta.len());
        }
        if let Some(calls) = parse_tool_calls_from_message(message) {
            self.tool_calls.extend(calls);
        }

        // Native streams send deltas and normally end with empty content. Some
        // Cloud models send a full final snapshot; emit only its unseen suffix.
        // Never erase accumulated text (and therefore the pending TTS reply).
        if let Some(content) = message["content"].as_str().filter(|text| !text.is_empty()) {
            let delta = if done && !self.content.is_empty() {
                content.strip_prefix(self.content.as_str()).unwrap_or(content)
            } else {
                content
            };
            if !delta.is_empty() {
                self.content.push_str(delta);
                on_token(delta.to_string());
            }
        }
        // Tool calls may arrive before the final record. They are not a done flag.
        self.done = done;
        Ok(())
    }
}

fn parse_ollama_response(data: &serde_json::Value) -> Result<ChatResponse, ProviderError> {
    let message = &data["message"];
    let content = message["content"].as_str().map(|s| s.to_string());
    let tool_calls = parse_tool_calls_from_message(message);
    let done_reason = parse_done_reason(data);
    let usage = parse_usage(data);
    tracing::info!(content_bytes = content.as_ref().map_or(0, String::len), tool_call_count = tool_calls.as_ref().map_or(0, Vec::len), thinking_bytes = message["thinking"].as_str().map_or(0, str::len), done_reason = done_reason.unwrap_or("unspecified"), completion_tokens = ?usage.as_ref().map(|u| u.completion_tokens), "Ollama response collected");
    check_completion(done_reason)?;
    let has_tools = tool_calls.as_ref().is_some_and(|calls| !calls.is_empty());

    // Think-only responses: surface the reasoning instead of failing the turn.
    let thinking_text = message["thinking"].as_str().unwrap_or_default();
    let content = if content.as_deref().is_none_or(|text| text.trim().is_empty())
        && !thinking_text.trim().is_empty()
    {
        Some(format!("[Denkspur]\n{}", thinking_text.trim()))
    } else {
        content
    };
    Ok(ChatResponse {
        reasoning_content: None,
        content,
        tool_calls,
        finish_reason: Some(if has_tools { "tool_calls" } else { "stop" }.to_string()),
        usage,
    })
}

// Only allowlisted metadata enters diagnostics, never arbitrary provider text.
fn parse_done_reason(data: &serde_json::Value) -> Option<&'static str> {
    data.get("done_reason").and_then(|value| value.as_str()).map(|reason| match reason {
        "length" => "length",
        "stop" => "stop",
        "load" => "load",
        "unload" => "unload",
        _ => "unknown",
    })
}

fn check_completion(done_reason: Option<&str>) -> anyhow::Result<()> {
    if done_reason == Some("length") {
        // num_predict includes thinking AND tool arguments. Even a valid tool
        // parsed before the cutoff may be part of an incomplete batch: discard
        // the entire attempt rather than treating it as successful completion.
        return Err(ProviderError {
            cause: Some("Ollama exhausted num_predict; thinking and tool arguments share this allowance"),
            ..ProviderError::new(ErrorKind::OutputLimit)
        }.into());
    }
    Ok(())
}

fn parse_usage(data: &serde_json::Value) -> Option<Usage> {
    Usage::from_counts(&data["prompt_eval_count"], &data["eval_count"])
}

fn parse_tool_calls_from_message(message: &serde_json::Value) -> Option<Vec<ToolCall>> {
    message["tool_calls"].as_array().map(|calls| {
        tracing::debug!(target: "ollama", "Found {} tool calls in response", calls.len());
        calls.iter().filter_map(|tc| {
            let func = tc.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let args = func.get("arguments")?.clone();
            tracing::debug!(target: "ollama", tool = %name, args_bytes = args.to_string().len(), "Ollama tool call parsed");
            Some(ToolCall {
                id: format!("call_{}", uuid::Uuid::new_v4()),
                function: FunctionCall {
                    name,
                    arguments: args.to_string(),
                },
            })
        }).collect()
    })
}
