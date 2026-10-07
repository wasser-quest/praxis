//! Ollama chat transport (`/api/chat`).
//!
//! Two capabilities beyond wire transport, both learned from production logs:
//!
//! * **Context budgeting** (`ollama_budget`): Ollama clamps generation to
//!   `num_ctx - prompt` and reports `done_reason="length"` far below the
//!   requested `num_predict`, so the runtime window is set explicitly and the
//!   prompt is fitted into its reserved half.
//! * **Reasoning continuation**: a think-only step (budget exhausted during
//!   thinking, or the model stopping after reasoning) is continued with an
//!   explicit instruction instead of being faked as an answer or dropped as a
//!   token-limit failure. Same contract as the Codex provider: the host owns
//!   the attempt budget; the adapter only reports what happened.
use super::error::{ErrorKind, ProviderError};
use super::provider::*;
use async_trait::async_trait;

#[cfg(test)]
#[path = "ollama_stream_tests.rs"]
mod voice_stream_tests;

#[cfg(test)]
#[path = "ollama_output_limit_tests.rs"]
mod output_limit_tests;

#[path = "ollama_budget.rs"]
mod budget;

/// Instruction appended after a preserved reasoning step. Mirrors the Codex
/// provider's nudge; request-local only, never stored as conversation history.
const CONTINUE_NUDGE: &str = "Continue the original task from the preceding reasoning. Return the next required function call or the final answer, not only a reasoning summary. Do not repeat actions already recorded in tool results.";
/// Same replay safety cap as the Codex continuation.
const MAX_CONTINUATION_BYTES: usize = 32 * 1024 * 1024;

pub struct OllamaProvider {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
    /// Operator window (`OLLAMA_NUM_CTX`), clamped to the model's real context.
    num_ctx: Option<u64>,
    /// Probed model context lengths, cached per model (None = probe failed).
    windows: tokio::sync::Mutex<std::collections::HashMap<String, Option<u64>>>,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            model,
            api_key,
            client: super::http::client(),
            num_ctx: None,
            windows: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Request `options.num_ctx` explicitly (clamped to the model's context).
    pub fn num_ctx(mut self, num_ctx: Option<u64>) -> Self {
        self.num_ctx = num_ctx;
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Runtime window to request. `None` keeps legacy behavior (server default,
    /// no output clamp) when neither the operator nor the server tells us more.
    async fn window_for(&self, model: &str) -> Option<u64> {
        let mut windows = self.windows.lock().await;
        if !windows.contains_key(model) {
            let probed =
                budget::probe_context_window(&self.client, &self.base_url, self.api_key.as_deref(), model)
                    .await;
            windows.insert(model.to_string(), probed);
        }
        let probed = windows.get(model).copied().flatten();
        match (self.num_ctx, probed) {
            (Some(requested), probed) => {
                let ceiling = probed.unwrap_or(u64::MAX).max(budget::MIN_WINDOW);
                Some(requested.clamp(budget::MIN_WINDOW, ceiling))
            }
            (None, Some(context)) => Some(context.min(budget::DEFAULT_WINDOW).max(budget::MIN_WINDOW)),
            (None, None) => None,
        }
    }

    async fn send_json(&self, body: &serde_json::Value) -> Result<reqwest::Response, ProviderError> {
        let mut request = self.client.post(self.url("/api/chat")).json(body);
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.is_empty()) {
            request = request.header("Authorization", format!("Bearer {}", key));
        }
        request.send().await.map_err(ProviderError::from_reqwest)
    }

    /// NDJSON streaming attempt. Thinking and tool calls are forwarded as
    /// display-only deltas; only content text counts as emitted answer text.
    async fn stream_body(
        &self,
        body: &serde_json::Value,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<Raw, ProviderError> {
        let resp = self.send_json(body).await?;
        let resp = super::http::checked(resp).await?;

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut state = OllamaStreamState::default();
        tracing::info!("[STREAM] Ollama byte stream started");

        let on_token = |text: String| on_delta(StreamDelta::Text { text });
        while let Some(chunk) = stream.next().await {
            state.push(&chunk.map_err(ProviderError::from_reqwest)?, &on_token)?;
            for delta in state.pending.drain(..) {
                on_delta(delta);
            }
            if state.done {
                break;
            }
        }
        state.finish(&on_token)?;
        for delta in state.pending.drain(..) {
            on_delta(delta);
        }
        let raw = state.into_raw();
        tracing::info!(content_bytes = raw.content.len(), tool_call_count = raw.tool_calls.len(), thinking_bytes = raw.thinking.len(), done_reason = raw.done_reason.unwrap_or("unspecified"), completion_tokens = ?raw.usage.as_ref().map(|u| u.completion_tokens), prompt_tokens = ?raw.usage.as_ref().map(|u| u.prompt_tokens), "[STREAM] Ollama response collected");
        Ok(raw)
    }

    async fn json_body(&self, body: &serde_json::Value) -> Result<Raw, ProviderError> {
        let response = super::http::checked(self.send_json(body).await?).await?;
        let status = response.status().as_u16();
        let data: serde_json::Value = response.json().await.map_err(ProviderError::from_reqwest)?;
        let raw = parse_raw(status, &data)?;
        tracing::info!(content_bytes = raw.content.len(), tool_call_count = raw.tool_calls.len(), thinking_bytes = raw.thinking.len(), done_reason = raw.done_reason.unwrap_or("unspecified"), completion_tokens = ?raw.usage.as_ref().map(|u| u.completion_tokens), prompt_tokens = ?raw.usage.as_ref().map(|u| u.prompt_tokens), "Ollama response collected");
        Ok(raw)
    }

    /// What one network attempt means: a continuable reasoning step, a
    /// completed answer/tool batch, or a typed failure. A truncated batch
    /// (`done_reason="length"` with content or tools) is never committed.
    fn outcome(
        &self,
        raw: Raw,
        replay: &mut Vec<serde_json::Value>,
        tool_context: bool,
        wire_predict: Option<u64>,
    ) -> Result<ChatAttempt, ProviderError> {
        let think_only =
            raw.tool_calls.is_empty() && raw.content.is_empty() && !raw.thinking.trim().is_empty();
        if raw.done_reason == Some("length") {
            let bound = budget_bound(raw.usage.as_ref(), wire_predict);
            // The incident case: the window (not num_predict) was binding.
            // Continuing keeps the thinking; replaying with a bigger allowance
            // cannot change the outcome and is reserved for real cutoffs.
            if think_only && tool_context && !bound {
                return continuation_attempt(replay, &raw);
            }
            return Err(ProviderError {
                cause: Some(if bound {
                    "Ollama exhausted num_predict; thinking and tool arguments share this allowance"
                } else {
                    "Ollama stopped with output budget left: the model context window left no room for output (raise OLLAMA_NUM_CTX or reduce history)"
                }),
                ..ProviderError::new(ErrorKind::OutputLimit)
            });
        }
        if think_only && tool_context {
            return continuation_attempt(replay, &raw);
        }
        Ok(ChatAttempt {
            response: completed_response(raw, tool_context),
            continuation: None,
        })
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
        let model = request.model.as_deref().unwrap_or(&self.model);
        let tool_context = request.tools.is_some();
        let mut replay: Vec<serde_json::Value> = match continuation {
            Some(ProviderContinuation::Ollama { messages }) => messages.clone(),
            _ => Vec::new(),
        };

        let window = self.window_for(model).await;
        let mut body = serde_json::json!({
            "model": model,
            "messages": build_ollama_messages(&request),
            "stream": on_delta.is_some(),
        });
        let wire_predict = add_options(&mut body, &request, window);
        // Ollama-API: think = false | true | "low"|"medium"|"high"|"xhigh"
        // (Stufen-Strings durchlaufen den Fork bis ins Qwen3.8-Template).
        if let Some(t) = request.thinking {
            body["think"] = match t.level() {
                Some(lvl) => serde_json::json!(lvl),
                None => serde_json::json!(false),
            };
        }
        if !replay.is_empty() {
            body["messages"]
                .as_array_mut()
                .expect("messages array")
                .extend(replay.iter().cloned());
        }
        if let Some(window) = window {
            // The reduction ladder drops old turns first; the continuation
            // tail is the newest input and survives.
            let omitted = budget::fit_body(&mut body, window, wire_predict.unwrap_or(window / 2))?;
            if omitted {
                if let Some(on_delta) = on_delta {
                    on_delta(StreamDelta::TemplateOmitted);
                }
            }
        }

        let raw = match on_delta {
            Some(on_delta) => self.stream_body(&body, on_delta).await?,
            None => self.json_body(&body).await?,
        };
        self.outcome(raw, &mut replay, tool_context, wire_predict)
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let attempt = self.chat_attempt(request, None, None).await?;
        require_answer(attempt)
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        let attempt = self
            .chat_attempt(request, None, Some(&|delta| {
                if let StreamDelta::Text { text } = delta {
                    on_token(text);
                }
            }))
            .await?;
        require_answer(attempt)
    }

    async fn chat_stream_events(
        &self,
        request: ChatRequest,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<ChatResponse, ProviderError> {
        let attempt = self.chat_attempt(request, None, Some(on_delta)).await?;
        require_answer(attempt)
    }

    async fn list_models(&self) -> Result<Vec<super::provider::ModelInfo>, ProviderError> {
        let mut request = self.client.get(self.url("/api/tags"));
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                request = request.header("Authorization", format!("Bearer {}", key));
            }
        }
        let resp = request.send().await.map_err(ProviderError::from_reqwest)?;
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
        let mut request = self
            .client
            .get(self.url("/api/tags"))
            .timeout(std::time::Duration::from_secs(10));
        if let Some(key) = self.api_key.as_deref().filter(|key| !key.is_empty()) {
            request = request.bearer_auth(key);
        }
        request
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    }
}

/// One network attempt's raw outcome, before any interpretation.
struct Raw {
    content: String,
    thinking: String,
    tool_calls: Vec<ToolCall>,
    done_reason: Option<&'static str>,
    usage: Option<Usage>,
}

/// chat/chat_stream cannot continue; never dress a reasoning step as an answer.
fn require_answer(attempt: ChatAttempt) -> Result<ChatResponse, ProviderError> {
    if attempt.continuation.is_some() {
        return Err(ProviderError::with_cause(
            ErrorKind::ReasoningOnly,
            "Ollama returned a reasoning step; continue the call to get an answer or tool calls",
        ));
    }
    Ok(attempt.response)
}

/// A cutoff is a real output limit only when the generated tokens actually
/// consumed the requested allowance. `done_reason="length"` at a fraction of it
/// means the context window left no room; more `num_predict` cannot help there.
/// Unknown counters keep the legacy output-limit recovery (bigger allowance).
fn budget_bound(usage: Option<&Usage>, wire_predict: Option<u64>) -> bool {
    match (usage.map(|usage| usage.completion_tokens), wire_predict) {
        (Some(generated), Some(predicted)) if predicted > 0 => {
            u64::from(generated).saturating_mul(4) >= predicted.saturating_mul(3)
        }
        _ => true,
    }
}

fn continuation_attempt(
    replay: &mut Vec<serde_json::Value>,
    raw: &Raw,
) -> Result<ChatAttempt, ProviderError> {
    replay.push(serde_json::json!({
        "role": "assistant",
        "content": format!("<thinking>\n{}\n</thinking>", raw.thinking.trim()),
    }));
    replay.push(serde_json::json!({ "role": "system", "content": CONTINUE_NUDGE }));
    if serde_json::to_vec(replay.as_slice()).map_or(0, |bytes| bytes.len()) > MAX_CONTINUATION_BYTES {
        return Err(ProviderError::with_cause(
            ErrorKind::ReasoningOnly,
            "Ollama continuation exceeded the 32 MiB safety limit",
        ));
    }
    // Host contract (routing): a continuable step must carry finish_reason
    // "reasoning", no content, no tool calls and no emitted answer text.
    Ok(ChatAttempt {
        response: ChatResponse {
            content: None,
            reasoning_content: Some(raw.thinking.trim().to_string()),
            tool_calls: None,
            finish_reason: Some("reasoning".to_string()),
            usage: raw.usage.clone(),
        },
        continuation: Some(ProviderContinuation::Ollama {
            messages: replay.clone(),
        }),
    })
}

fn completed_response(raw: Raw, tool_context: bool) -> ChatResponse {
    let thinking = raw.thinking.trim();
    let has_tools = !raw.tool_calls.is_empty();
    let content = if !raw.content.is_empty() {
        Some(raw.content)
    } else if !thinking.is_empty() && !tool_context {
        // Plain chat only: a think-only reply is usable output, never an error.
        // Tool contexts continue the step instead of faking an answer.
        Some(format!("[Denkspur]\n{}", thinking))
    } else {
        None
    };
    ChatResponse {
        content,
        reasoning_content: (!thinking.is_empty()).then(|| thinking.to_string()),
        tool_calls: has_tools.then_some(raw.tool_calls),
        finish_reason: Some(if has_tools { "tool_calls" } else { "stop" }.to_string()),
        usage: raw.usage,
    }
}

/// Output options plus the exact `num_predict` on the wire (the cutoff
/// diagnosis compares against it). With a known window the output gets at most
/// half of it: `num_predict` covers thinking AND tool arguments, and the other
/// half stays guaranteed for tools + history.
fn add_options(
    body: &mut serde_json::Value,
    request: &ChatRequest,
    window: Option<u64>,
) -> Option<u64> {
    let mut options = serde_json::Map::new();
    let requested = request.max_tokens.map(u64::from).filter(|tokens| *tokens > 0);
    let wire_predict = match window {
        Some(window) => {
            options.insert("num_ctx".into(), serde_json::json!(window));
            let output_cap = (window / 2).max(1);
            Some(requested.unwrap_or(output_cap).min(output_cap))
        }
        None => requested,
    };
    if let Some(predict) = wire_predict {
        options.insert("num_predict".into(), serde_json::json!(predict));
    }
    if let Some(temperature) = request.temperature {
        options.insert("temperature".into(), serde_json::json!(temperature));
    }
    if !options.is_empty() {
        body["options"] = serde_json::Value::Object(options);
    }
    if let Some(ref tools) = request.tools {
        let ollama_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(ollama_tools);
    }
    wire_predict
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

fn parse_raw(status: u16, data: &serde_json::Value) -> Result<Raw, ProviderError> {
    if data.get("error").is_some_and(|error| !error.is_null()) {
        return Err(classify_payload(status, data));
    }
    let message = &data["message"];
    Ok(Raw {
        content: message["content"].as_str().unwrap_or_default().to_string(),
        thinking: message["thinking"].as_str().unwrap_or_default().to_string(),
        tool_calls: parse_tool_calls_from_message(message).unwrap_or_default(),
        done_reason: parse_done_reason(data),
        usage: parse_usage(data),
    })
}

/// Map in-band Ollama error payloads onto actionable kinds. The provider
/// message is inspected for classification only and never copied into errors
/// or logs (same rule as the shared HTTP layer).
fn classify_payload(status: u16, data: &serde_json::Value) -> ProviderError {
    let mut error = super::http::payload_error(status, data);
    let message = data
        .pointer("/error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if message.contains("not found") || message.contains("no such model") || message.contains("try pulling") {
        error.kind = ErrorKind::Configuration;
        error.cause = Some(
            "the selected Ollama model is not available on this server; check OLLAMA_MODEL or settings.model",
        );
    } else if message.contains("context")
        && (message.contains("length")
            || message.contains("exceed")
            || message.contains("larger")
            || message.contains("maximum"))
    {
        error.kind = ErrorKind::ContextWindow;
        error.cause = Some(
            "the request exceeded the model context window; reduce history/tools or raise OLLAMA_NUM_CTX",
        );
    } else if message.contains("memory") || message.contains("vram") {
        error.kind = ErrorKind::Unavailable;
        error.cause = Some(
            "the server could not load the model with this context; lower OLLAMA_NUM_CTX or free GPU memory",
        );
    }
    error
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
    /// Display-only deltas (reasoning previews, tool-call previews) drained by
    /// the caller after each chunk. Text answer deltas go through `on_token`.
    pending: Vec<StreamDelta>,
}

impl OllamaStreamState {
    fn into_raw(self) -> Raw {
        Raw {
            content: self.content,
            thinking: self.thinking,
            tool_calls: self.tool_calls,
            done_reason: self.done_reason,
            usage: self.usage,
        }
    }

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
            return Err(classify_payload(200, &data).into());
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
            if !thinking_delta.is_empty() {
                // Display-only (host gates it); never emitted as answer text.
                self.pending.push(StreamDelta::Reasoning { text: thinking_delta.to_string() });
            }
        }
        if let Some(calls) = parse_tool_calls_from_message(message) {
            for call in &calls {
                self.pending.push(StreamDelta::ToolCall {
                    index: self.tool_calls.len(),
                    id: Some(call.id.clone()),
                    name: Some(call.function.name.clone()),
                    arguments: Some(call.function.arguments.clone()),
                });
            }
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
