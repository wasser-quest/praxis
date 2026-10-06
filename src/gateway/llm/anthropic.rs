use super::provider::*;
use async_trait::async_trait;
use crate::gateway::llm::error::ProviderError;

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
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
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);

        let (system_prompt, messages) = build_anthropic_messages(&request.messages);

        let model = request.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        // Extended thinking: Anthropic requires an explicit token budget and
        // forbids temperature overrides alongside it.
        match request.thinking {
            Some(t) if t.level().is_some() => {
                // Stufen → Budget-Scala: low 1k, medium 2k, high 4k, xhigh 8k
                // (Anthropic kennt keine Level, nur Token-Budget).
                let budget = request.max_tokens.map(|m| (m / 2).max(1024)).unwrap_or(match t.level() {
                    Some("low") => 1024,
                    Some("medium") => 2048,
                    Some("xhigh") => 8192,
                    _ => 4096,
                }).min(match t.level() {
                    Some("low") => 2048,
                    Some("medium") => 4096,
                    Some("xhigh") => 16384,
                    _ => 8192,
                });
                body["thinking"] = serde_json::json!({ "type": "enabled", "budget_tokens": budget });
                body.as_object_mut().map(|o| o.remove("temperature"));
            }
            Some(super::provider::ThinkingMode::Off) => {
                body["thinking"] = serde_json::json!({ "type": "disabled" });
            }
            _ => {}
        }

        if !system_prompt.is_empty() {
            body["system"] = serde_json::json!(system_prompt);
        }

        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::json!(build_anthropic_tools(tools));
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
        parse_anthropic_response(&data)
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn build_anthropic_messages(messages: &[ChatMessage]) -> (String, Vec<serde_json::Value>) {
    let mut system_prompt = String::new();
    let mut out: Vec<serde_json::Value> = Vec::new();

    for m in messages {
        if m.role == "system" {
            if let Some(content) = m.content.as_deref().filter(|text| !text.is_empty()) {
                if !system_prompt.is_empty() { system_prompt.push_str("\n\n"); }
                system_prompt.push_str(content);
            }
            continue;
        }
        if m.role == "tool" {
            let content_value = if let Some(ref parts) = m.content_parts {
                let mut arr: Vec<serde_json::Value> = Vec::new();
                if let Some(ref text) = m.content {
                    if !text.is_empty() {
                        arr.push(serde_json::json!({"type": "text", "text": text}));
                    }
                }
                for p in parts {
                    match p {
                        ContentPart::Text { text } => {
                            arr.push(serde_json::json!({"type": "text", "text": text}))
                        }
                        ContentPart::ImageUrl { image_url } => {
                            // Anthropic uses source blocks for images
                            if let Some(base64_data) = image_url
                                .url
                                .strip_prefix("data:")
                                .and_then(|s| s.find(",").map(|i| &s[i + 1..]))
                            {
                                let media_type = image_url
                                    .url
                                    .split(';')
                                    .next()
                                    .unwrap_or("image/png")
                                    .strip_prefix("data:")
                                    .unwrap_or("image/png");
                                arr.push(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": base64_data
                                    }
                                }));
                            }
                        }
                    }
                }
                serde_json::json!(arr)
            } else {
                serde_json::json!(m.content.as_deref().unwrap_or(""))
            };
            let tool_result_content = serde_json::json!([{
                "type": "tool_result",
                "tool_use_id": m.tool_call_id.as_deref().unwrap_or(""),
                "content": content_value
            }]);
            out.push(serde_json::json!({ "role": "user", "content": tool_result_content }));
            continue;
        }
        if m.role == "assistant" {
            if let Some(ref tool_calls) = m.tool_calls {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if let Some(ref text) = m.content {
                    if !text.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": text}));
                    }
                }
                for tc in tool_calls {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": args
                    }));
                }
                out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
                continue;
            }
        }
        if let Some(ref parts) = m.content_parts {
            let mut arr: Vec<serde_json::Value> = Vec::new();
            if let Some(ref text) = m.content {
                if !text.is_empty() {
                    arr.push(serde_json::json!({"type": "text", "text": text}));
                }
            }
            for p in parts {
                match p {
                    ContentPart::Text { text } => {
                        arr.push(serde_json::json!({"type": "text", "text": text}))
                    }
                    ContentPart::ImageUrl { image_url } => {
                        if let Some(base64_data) = image_url
                            .url
                            .strip_prefix("data:")
                            .and_then(|s| s.find(",").map(|i| &s[i + 1..]))
                        {
                            let media_type = image_url
                                .url
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .strip_prefix("data:")
                                .unwrap_or("image/png");
                            arr.push(serde_json::json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": base64_data
                                }
                            }));
                        }
                    }
                }
            }
            out.push(serde_json::json!({ "role": m.role, "content": arr }));
        } else {
            out.push(serde_json::json!({
                "role": m.role,
                "content": m.content.as_deref().unwrap_or("")
            }));
        }
    }

    (system_prompt, out)
}

pub fn build_anthropic_tools(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.function.name,
                "description": t.function.description,
                "input_schema": t.function.parameters
            })
        })
        .collect()
}

pub fn parse_anthropic_response(data: &serde_json::Value) -> Result<ChatResponse, ProviderError> {
    let mut text_content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(content) = data["content"].as_array() {
        for block in content {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        if !text_content.is_empty() {
                            text_content.push('\n');
                        }
                        text_content.push_str(text);
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        function: FunctionCall {
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            arguments: block["input"].to_string(),
                        },
                    });
                }
                _ => {}
            }
        }
    }

    let content = if text_content.is_empty() {
        None
    } else {
        Some(text_content)
    };
    let tool_calls = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };

    Ok(ChatResponse {
        reasoning_content: None,
        content,
        tool_calls,
        finish_reason: data["stop_reason"].as_str().map(|s| s.to_string()),
        usage: parse_usage(&data["usage"]),
    })
}

fn parse_usage(value: &serde_json::Value) -> Option<Usage> {
    let mut usage = Usage::from_counts(&value["input_tokens"], &value["output_tokens"])?;
    // Anthropic's input_tokens excludes cache hits/writes. Those two top-level
    // counters are disjoint; cache_creation's duration breakdown is NOT extra.
    // https://platform.claude.com/docs/en/build-with-claude/prompt-caching
    for field in ["cache_creation_input_tokens", "cache_read_input_tokens"] {
        if let Some(count) = value.get(field) {
            usage.prompt_tokens = usage.prompt_tokens.checked_add(Usage::counter(count)?)?;
        }
    }
    usage.total_tokens = usage.prompt_tokens.checked_add(usage.completion_tokens)?;
    Some(usage)
}
