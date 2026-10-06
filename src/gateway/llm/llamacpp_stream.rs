//! Native OpenAI/llama.cpp SSE. Tool fragments are previews, not executable calls.
use super::*;
use crate::gateway::llm::{error::{ErrorKind, ProviderError}, http};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
struct Accumulator {
    content: String,
    reasoning: String,
    calls: BTreeMap<usize, ToolCall>,
    finish: Option<String>,
    usage: Option<Usage>,
    bytes: usize,
}
impl Accumulator {
    fn push(&mut self, data: Value, on_delta: &(dyn Fn(StreamDelta) + Send + Sync)) -> anyhow::Result<()> {
        if data.get("error").is_some() { return Err(http::payload_error(200, &data).into()); }
        if let Some(usage) = data.get("usage").filter(|u| u.is_object()) {
            self.usage = Usage::openai(usage);
        }
        for choice in data["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 { continue; }
            if let Some(finish) = choice["finish_reason"].as_str() { self.finish = Some(finish.into()); }
            let delta = &choice["delta"];
            for (key, reasoning) in [("content", false), ("reasoning_content", true)] {
                if let Some(text) = delta[key].as_str().filter(|s| !s.is_empty()) {
                    self.bytes += text.len();
                    if reasoning { self.reasoning.push_str(text); on_delta(StreamDelta::Reasoning { text:text.into() }); }
                    else { self.content.push_str(text); on_delta(StreamDelta::Text { text:text.into() }); }
                }
            }
            for tc in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = tc["index"].as_u64().ok_or_else(|| ProviderError::new(ErrorKind::InvalidResponse))?;
                if index >= 128 { return Err(ProviderError::new(ErrorKind::InvalidResponse).into()); }
                let index = index as usize;
                let id = tc["id"].as_str().map(String::from);
                let name = tc["function"]["name"].as_str().map(String::from);
                let arguments = tc["function"]["arguments"].as_str().map(String::from);
                let call = self.calls.entry(index).or_insert_with(|| ToolCall {
                    id:String::new(), function:FunctionCall {name:String::new(),arguments:String::new()},
                });
                for (target, part) in [(&mut call.id,&id), (&mut call.function.name,&name), (&mut call.function.arguments,&arguments)] {
                    if let Some(part) = part { self.bytes += part.len(); target.push_str(part); }
                }
                on_delta(StreamDelta::ToolCall {index,id,name,arguments});
            }
        }
        if self.bytes > 8 * 1024 * 1024 { return Err(ProviderError::new(ErrorKind::OutputLimit).into()); }
        Ok(())
    }
    fn finish(self) -> Result<ChatResponse, ProviderError> {
        if self.finish.is_none() { return Err(ProviderError::new(ErrorKind::Interrupted).into()); }
        Ok(ChatResponse {
            content: (!self.content.is_empty()).then_some(self.content),
            reasoning_content: (!self.reasoning.is_empty()).then_some(self.reasoning),
            tool_calls: (!self.calls.is_empty()).then(|| self.calls.into_values().collect()),
            finish_reason: self.finish, usage:self.usage,
        })
    }
}

pub(super) async fn receive(response: reqwest::Response, on_delta: &(dyn Fn(StreamDelta) + Send + Sync)) -> Result<ChatResponse, ProviderError> {
    let mut response = http::checked(response).await?;
    let mut decoder = crate::sse::Decoder::default();
    let mut acc = Accumulator::default();
    let mut done = false;
    while let Some(chunk) = response.chunk().await.map_err(ProviderError::from_reqwest)? {
        for event in decoder.push(&chunk).map_err(|_| ProviderError::new(ErrorKind::InvalidResponse))? {
            if event.data.trim() == "[DONE]" { done = true; break; }
            let data: Value = serde_json::from_str(&event.data).map_err(|_| ProviderError::new(ErrorKind::InvalidResponse))?;
            acc.push(data, on_delta)?;
        }
        if done { break; }
    }
    if !done { return Err(ProviderError::new(ErrorKind::Interrupted).into()); }
    acc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[test]
    fn native_stream_preserves_interleaved_tool_fragments_and_reasoning() {
        let mut acc = Accumulator::default();
        let events = Mutex::new(Vec::new());
        let on = |e| events.lock().unwrap().push(e);
        for delta in [
            serde_json::json!({"reasoning_content":"Denke…"}),
            serde_json::json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{\"path\":"}},{"index":1,"id":"b","function":{"name":"get_context","arguments":"{"}}]}),
            serde_json::json!({"tool_calls":[{"index":1,"function":{"arguments":"}"}},{"index":0,"function":{"arguments":"\"🙂\"}"}}]}),
        ] { acc.push(serde_json::json!({"choices":[{"delta":delta}]}), &on).unwrap(); }
        assert_eq!(events.lock().unwrap().len(), 5, "previews available before finish");
        acc.push(serde_json::json!({"choices":[{"finish_reason":"tool_calls"}]}), &on).unwrap();
        let response = acc.finish().unwrap();
        assert_eq!(response.reasoning_content.as_deref(), Some("Denke…"));
        let calls = response.tool_calls.unwrap();
        assert_eq!(calls[0].function.arguments, "{\"path\":\"🙂\"}");
        assert_eq!(calls[1].function.arguments, "{}");
    }
    #[test]
    fn native_stream_requires_explicit_completion() {
        assert!(Accumulator::default().finish().is_err());
    }
    #[test]
    fn usage_metrics_llamacpp_stream_preserves_unknown_and_deduplicates_snapshots() {
        for (usage, expected) in [
            (serde_json::json!({"completion_tokens":5}), serde_json::Value::Null),
            (serde_json::json!({"prompt_tokens":7,"completion_tokens":5}),
                serde_json::json!({"prompt_tokens":7,"completion_tokens":5,"total_tokens":12})),
        ] {
            let mut acc = Accumulator::default();
            for _ in 0..2 {
                acc.push(serde_json::json!({"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":usage}), &|_| {}).unwrap();
            }
            assert_eq!(serde_json::to_value(acc.finish().unwrap().usage).unwrap(), expected);
        }
    }

}
