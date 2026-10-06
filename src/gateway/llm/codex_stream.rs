//! Responses SSE framing and validation. Preview fragments are never actions.
use super::super::{
    error::{ErrorKind, ProviderError},
    http,
    provider::*,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
struct Accumulator {
    text: String,
    reasoning: BTreeMap<(u64, u64), String>,
    done_items: Vec<Value>,
    events: usize,
    bytes: usize,
}

fn invalid(cause: &'static str) -> ProviderError {
    ProviderError::with_cause(ErrorKind::InvalidResponse, cause)
}

fn failed(value: &Value) -> ProviderError {
    // Classify only known machine codes. Server messages can echo credentials
    // or prompts, so neither errors nor debug logs may retain them.
    let error = value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
        .unwrap_or(value);
    let mut error = http::payload_error(200, &serde_json::json!({"error": error}));
    error.cause = Some("Codex reported a failed response (response.failed/error)");
    error
}

fn incomplete(response: &Value) -> ProviderError {
    match response.pointer("/incomplete_details/reason").and_then(Value::as_str) {
        Some("max_output_tokens") => ProviderError::with_cause(
            ErrorKind::OutputLimit, "Codex exhausted its output budget; request a shorter response (this endpoint cannot raise max_tokens)",
        ),
        Some("content_filter") => invalid("Codex response blocked by content filtering; rephrase the request"),
        _ => invalid("Codex reported an incomplete response; no tool calls were accepted"),
    }
}

impl Accumulator {
    fn push(
        &mut self,
        event: crate::sse::Event,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<Option<ChatAttempt>, ProviderError> {
        self.events += 1;
        self.bytes = self.bytes.saturating_add(event.data.len());
        if self.bytes > 32 * 1024 * 1024 {
            return Err(invalid("Codex stream exceeded the 32 MiB safety limit"));
        }
        if event.data.trim() == "[DONE]" {
            return Err(ProviderError::with_cause(
                ErrorKind::Interrupted,
                "Codex sent [DONE] without response.completed; no tool calls were accepted",
            ));
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| invalid("Codex SSE event contained malformed JSON"))?;
        // Some proxies retain only the SSE event name, others only JSON type.
        let kind = value["type"].as_str().unwrap_or(&event.event);
        match kind {
            "response.output_text.delta" | "response.refusal.delta" => {
                let delta = value["delta"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex text delta is missing text"))?;
                self.text.push_str(delta);
                on_delta(StreamDelta::Text { text: delta.into() });
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = value["delta"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex reasoning delta is missing text"))?;
                self.reasoning_part(&value, delta, false, on_delta);
            }
            "response.reasoning_summary_text.done" | "response.reasoning_text.done" => {
                let text = value["text"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex completed reasoning part is missing text"))?;
                self.reasoning_part(&value, text, true, on_delta);
            }
            "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
                if let Some(text) = value["part"]["text"].as_str().filter(|s| !s.is_empty()) {
                    self.reasoning_part(&value, text, true, on_delta);
                }
            }
            "response.output_item.added" if value["item"]["type"] == "function_call" => {
                on_delta(StreamDelta::ToolCall {
                    index: output_index(&value)?,
                    id: value["item"]["call_id"].as_str().map(str::to_string),
                    name: value["item"]["name"].as_str().map(str::to_string),
                    arguments: value["item"]["arguments"].as_str().map(str::to_string),
                });
            }
            "response.function_call_arguments.delta" => {
                on_delta(StreamDelta::ToolCall {
                    index: output_index(&value)?,
                    id: None,
                    name: None,
                    arguments: Some(
                        value["delta"]
                            .as_str()
                            .ok_or_else(|| invalid("Codex tool argument delta is missing text"))?
                            .into(),
                    ),
                });
            }
            "response.output_item.done" => {
                if !value["item"].is_object() {
                    return Err(invalid("Codex output_item.done is missing its item"));
                }
                // Retain complete items: Codex can omit them from the terminal
                // response.output. Tools still require successful response
                // completion and validation; preview fragments are never used.
                if value["item"]["type"] == "reasoning" {
                    for (index, part) in value["item"]["summary"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        if let Some(text) = part["text"].as_str() {
                            let location = serde_json::json!({
                                "output_index": value["output_index"], "summary_index": index,
                            });
                            self.reasoning_part(&location, text, true, on_delta);
                        }
                    }
                }
                self.done_items.push(value["item"].clone());
            }
            "response.failed" | "error" => return Err(failed(&value)),
            "response.incomplete" => return Err(incomplete(&value["response"])),
            "response.completed" | "response.done" => {
                let response = &value["response"];
                if !response.is_object() {
                    return Err(invalid(
                        "Codex completion event is missing its response object",
                    ));
                }
                if response.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(failed(&value));
                }
                match response["status"].as_str() {
                    Some("failed") => return Err(failed(&value)),
                    Some("incomplete") => return Err(incomplete(response)),
                    Some("completed") | None => {}
                    _ => return Err(invalid("Codex completion event has a non-completed status")),
                }
                return self.finish(response).map(Some);
            }
            // Lifecycle, usage and new informational event types are harmless.
            _ => {}
        }
        Ok(None)
    }

    fn reasoning_part(
        &mut self,
        value: &Value,
        text: &str,
        done: bool,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) {
        if text.is_empty() {
            return;
        }
        let key = (
            value["output_index"].as_u64().unwrap_or(0),
            value["summary_index"]
                .as_u64()
                .or_else(|| value["content_index"].as_u64())
                .unwrap_or(0),
        );
        let separator = !self.reasoning.is_empty() && !self.reasoning.contains_key(&key);
        let part = self.reasoning.entry(key).or_default();
        let delta = if done {
            text.strip_prefix(part.as_str()).unwrap_or("")
        } else {
            text
        };
        if !delta.is_empty() {
            if separator {
                on_delta(StreamDelta::Reasoning {
                    text: "\n\n".into(),
                });
            }
            on_delta(StreamDelta::Reasoning { text: delta.into() });
        }
        if done {
            *part = text.into();
        } else {
            part.push_str(text);
        }
    }

    fn finish(&self, response: &Value) -> Result<ChatAttempt, ProviderError> {
        let output = match response.get("output") {
            // A populated terminal snapshot is authoritative. Do not merge it
            // with streamed items, which could duplicate tool executions.
            Some(Value::Array(items)) if !items.is_empty() => items,
            // Some Codex streams finish with only status/usage and an empty or
            // omitted output. Only output_item.done payloads may fill that gap;
            // added items and argument deltas remain display-only previews.
            Some(Value::Array(_)) | Some(Value::Null) | None => &self.done_items,
            _ => return Err(invalid("Codex completed response.output is not an array")),
        };
        let mut content = String::new();
        let mut reasoning_parts = Vec::new();
        let mut reasoning_items = Vec::new();
        let mut tool_calls = Vec::new();
        let mut unsupported = false;
        for item in output {
            match item["type"].as_str() {
                Some("message") => {
                    for part in item["content"].as_array().into_iter().flatten() {
                        if let Some(text) =
                            part["text"].as_str().or_else(|| part["refusal"].as_str())
                        {
                            content.push_str(text);
                        }
                    }
                }
                Some("reasoning") => {
                    reasoning_items.push(item.clone());
                    for part in item["summary"].as_array().into_iter().flatten() {
                        if let Some(text) = part["text"].as_str().filter(|s| !s.is_empty()) {
                            reasoning_parts.push(text);
                        }
                    }
                }
                Some("function_call") => {
                    if item
                        .get("status")
                        .is_some_and(|status| status != "completed")
                    {
                        return Err(invalid(
                            "Codex function call is not completed; no tools were executed",
                        ));
                    }
                    let id = item["call_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| {
                            invalid(
                                "Codex function call is missing call_id; no tools were executed",
                            )
                        })?;
                    if tool_calls.iter().any(|call: &ToolCall| call.id == id) {
                        return Err(invalid(
                            "Codex returned duplicate call_ids; no tools were executed",
                        ));
                    }
                    let name = item["name"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                        .ok_or_else(|| {
                            invalid(
                                "Codex function call is missing its name; no tools were executed",
                            )
                        })?;
                    let arguments = item["arguments"].as_str().ok_or_else(|| {
                        invalid("Codex function call arguments are missing or not a string")
                    })?;
                    if !serde_json::from_str::<Value>(arguments).is_ok_and(|args| args.is_object())
                    {
                        return Err(invalid("Codex tool arguments are not a valid JSON object (possibly truncated); no tools were executed"));
                    }
                    tool_calls.push(ToolCall {
                        id: id.into(),
                        function: FunctionCall {
                            name: name.into(),
                            arguments: arguments.into(),
                        },
                    });
                }
                _ => unsupported = true,
            }
        }
        if content.is_empty() {
            content.clone_from(&self.text);
        }
        let reasoning = if reasoning_parts.is_empty() {
            self.reasoning
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            reasoning_parts.join("\n\n")
        };
        let reasoning_only = content.trim().is_empty() && tool_calls.is_empty();
        if reasoning_only
            && (unsupported || (reasoning_items.is_empty() && reasoning.trim().is_empty()))
        {
            return Err(invalid(if unsupported {
                "Codex completed with unsupported output items and no text or function calls"
            } else {
                "Codex completed without any text or function calls"
            }));
        }
        let usage = Usage::responses(&response["usage"]);
        Ok(ChatAttempt {
            response: ChatResponse {
                finish_reason: Some(
                    if reasoning_only {
                        "reasoning"
                    } else if tool_calls.is_empty() {
                        "stop"
                    } else {
                        "tool_calls"
                    }
                    .into(),
                ),
                content: (!content.is_empty()).then_some(content),
                reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                usage,
            },
            continuation: reasoning_only.then_some(ProviderContinuation::Codex {
                input: reasoning_items,
            }),
        })
    }
}

fn output_index(value: &Value) -> Result<usize, ProviderError> {
    value["output_index"]
        .as_u64()
        .filter(|index| *index < 128)
        .map(|index| index as usize)
        .ok_or_else(|| invalid("Codex tool preview has a missing or out-of-range output_index"))
}

pub(super) async fn receive(
    mut response: reqwest::Response,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<ChatAttempt, ProviderError> {
    let headers = response.headers().clone();
    let status = response.status().as_u16();
    let mut acc = Accumulator::default();
    let result = async {
        if headers
            .get(reqwest::header::CONTENT_TYPE)
            .is_some_and(|value| {
                !value
                    .to_str()
                    .unwrap_or("")
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("text/event-stream")
            })
        {
            return Err(invalid(
                "Codex returned a non-SSE response; expected Content-Type text/event-stream",
            ));
        }
        let mut decoder = crate::sse::Decoder::default();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(ProviderError::from_reqwest)?
        {
            // Decode bytes only after a complete line: handles split UTF-8 and
            // CRLF, unlike decoding each HTTP chunk then looking for \n\n.
            for event in decoder.push(&chunk).map_err(|_| {
                invalid("Codex SSE framing is invalid (UTF-8 or frame exceeds 1 MiB)")
            })? {
                if let Some(result) = acc.push(event, on_delta)? {
                    return Ok(result); // completion, not socket EOF, ends the response
                }
            }
        }
        Err(ProviderError::with_cause(
            ErrorKind::Interrupted,
            "Codex stream ended before response.completed; no tool calls were accepted",
        ))
    }
    .await
    .map_err(|mut error| {
        error.status = Some(status);
        http::metadata(&headers, &mut error);
        error
    });
    // Never log event bodies, text, tool arguments or arbitrary server messages.
    tracing::debug!(
        provider = "codex",
        http_status = status,
        events = acc.events,
        decoded_bytes = acc.bytes,
        text_bytes = acc.text.len(),
        reasoning_bytes = acc.reasoning.values().map(String::len).sum::<usize>(),
        done_items = acc.done_items.len(),
        success = result.is_ok(),
        "Codex stream summary"
    );
    Ok(result?)
}

#[cfg(test)]
#[path = "codex_stream_tests.rs"]
mod tests;
