//! Native OpenAI/llama.cpp SSE. Tool fragments are previews, not executable calls.
use crate::{
    error::{ErrorKind, ProviderError},
    provider::*, sse,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_TOOL_CALL_INDEX: u64 = 128;

#[derive(Default)]
pub(crate) struct Accumulator {
    content: String,
    reasoning: String,
    calls: BTreeMap<usize, ToolCall>,
    finish: Option<String>,
    usage: Option<Usage>,
    bytes: usize,
}

impl Accumulator {
    pub(crate) fn push(
        &mut self,
        data: &Value,
        on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
    ) -> Result<(), ProviderError> {
        if data.get("error").is_some() {
            return Err(crate::errors::classify(200, data));
        }
        if let Some(usage) = data.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Usage::openai(usage);
        }
        for choice in data["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            if let Some(finish) = choice["finish_reason"].as_str() {
                self.finish = Some(finish.to_owned());
            }
            let delta = &choice["delta"];
            for (key, reasoning) in [("content", false), ("reasoning_content", true)] {
                if let Some(text) = delta[key].as_str().filter(|text| !text.is_empty()) {
                    self.bytes += text.len();
                    if reasoning {
                        self.reasoning.push_str(text);
                        on_delta(StreamDelta::Reasoning { text: text.to_owned() });
                    } else {
                        self.content.push_str(text);
                        on_delta(StreamDelta::Text { text: text.to_owned() });
                    }
                }
            }
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = call["index"].as_u64().ok_or_else(|| ProviderError::new(ErrorKind::InvalidResponse))?;
                if index >= MAX_TOOL_CALL_INDEX {
                    return Err(ProviderError::new(ErrorKind::InvalidResponse));
                }
                let index = index as usize;
                let id = call["id"].as_str().map(str::to_owned);
                let name = call["function"]["name"].as_str().map(str::to_owned);
                let arguments = call["function"]["arguments"].as_str().map(str::to_owned);
                let entry = self.calls.entry(index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    function: FunctionCall {
                        name: String::new(),
                        arguments: String::new(),
                    },
                });
                for (target, part) in [
                    (&mut entry.id, &id),
                    (&mut entry.function.name, &name),
                    (&mut entry.function.arguments, &arguments),
                ] {
                    if let Some(part) = part {
                        self.bytes += part.len();
                        target.push_str(part);
                    }
                }
                on_delta(StreamDelta::ToolCall { index, id, name, arguments });
            }
        }
        if self.bytes > MAX_STREAM_BYTES {
            return Err(ProviderError::new(ErrorKind::OutputLimit));
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<ChatResponse, ProviderError> {
        if self.finish.is_none() {
            return Err(ProviderError::new(ErrorKind::Interrupted));
        }
        Ok(ChatResponse {
            content: (!self.content.is_empty()).then(|| std::mem::take(&mut self.content)),
            reasoning_content: (!self.reasoning.is_empty())
                .then(|| std::mem::take(&mut self.reasoning)),
            tool_calls: (!self.calls.is_empty()).then(|| std::mem::take(&mut self.calls).into_values().collect()),
            finish_reason: self.finish.take(),
            usage: self.usage.take(),
        })
    }
}

pub(crate) async fn receive(
    response: reqwest::Response,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<ChatResponse, ProviderError> {
    let mut response = crate::errors::checked(response).await?;
    let mut decoder = sse::Decoder::default();
    let mut acc = Accumulator::default();
    let mut done = false;
    while let Some(chunk) = response.chunk().await.map_err(ProviderError::from_reqwest)? {
        for event in decoder.push(&chunk)? {
            if event.data.trim() == "[DONE]" {
                done = true;
                break;
            }
            let data: Value = match serde_json::from_str(&event.data) {
                Ok(data) => data,
                Err(_) => return Err(ProviderError::new(ErrorKind::InvalidResponse)),
            };
            acc.push(&data, on_delta)?;
        }
        if done {
            break;
        }
    }
    if !done {
        return Err(ProviderError::new(ErrorKind::Interrupted));
    }
    acc.finish()
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
