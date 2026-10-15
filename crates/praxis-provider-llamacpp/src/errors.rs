//! llama.cpp error payloads mapped onto actionable kinds.
//!
//! llama-server reports errors as `{"error": {"code", "message"}}` or, in older
//! builds and some proxies, as `{"error": "text"}`, sometimes with HTTP 200 or
//! 500. The message is inspected for classification only and never copied into
//! errors or logs (same rule as the shared HTTP layer).
use crate::error::{ErrorKind, ProviderError};
use crate::http;
use serde_json::Value;

const CONTEXT_CODES: &[&str] = &["out_of_context", "exceed_context_size_error", "context_length_exceeded"];
const CONTEXT_PHRASES: &[&str] = &[
    "exceeds the available context",
    "exceeds the context",
    "context size exceeded",
    "n_keep is larger than n_ctx",
    "input is too large to process",
    "context shift is disabled",
];
const MODEL_PHRASES: &[&str] = &["model not found", "no such model", "failed to load model", "unknown model"];
const MEMORY_PHRASES: &[&str] = &["vram", "out of memory", "failed to allocate", "insufficient memory"];

pub(crate) fn classify(status: u16, data: &Value) -> ProviderError {
    let mut error = http::payload_error(status, data);
    // Credentials, quota and rate limits are decided by status/code alone.
    if matches!(error.kind, ErrorKind::Authentication | ErrorKind::QuotaExhausted | ErrorKind::RateLimited) {
        return error;
    }
    let code = data.pointer("/error/code").and_then(Value::as_str).unwrap_or_default();
    let message = data
        .pointer("/error/message")
        .or_else(|| data.get("error"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let has = |phrases: &[&str]| phrases.iter().any(|phrase| message.contains(phrase));
    if CONTEXT_CODES.contains(&code) || has(CONTEXT_PHRASES) {
        error.kind = ErrorKind::ContextWindow;
        error.cause = Some("the request exceeded the server's context window (n_ctx)");
    } else if has(MODEL_PHRASES) {
        error.kind = ErrorKind::Configuration;
        error.cause = Some("the selected model is not available on this llama.cpp server");
    } else if has(MEMORY_PHRASES) {
        error.kind = ErrorKind::Unavailable;
        error.cause = Some("the server ran out of memory; lower the context size or free GPU memory");
    }
    error
}

pub(crate) async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    http::checked_with(response, classify).await
}

pub(crate) async fn json(response: reqwest::Response) -> Result<Value, ProviderError> {
    http::json_with(response, classify).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn llama_payloads_map_to_actionable_kinds() {
        for (status, body, kind) in [
            (200, json!({"error": "model not found"}), ErrorKind::Configuration),
            (500, json!({"error": {"code": "out_of_context", "message": "x"}}), ErrorKind::ContextWindow),
            (400, json!({"error": {"message": "the request exceeds the available context size"}}), ErrorKind::ContextWindow),
            (200, json!({"error": "insufficient VRAM memory"}), ErrorKind::Unavailable),
            (429, json!({"error": "model not found"}), ErrorKind::RateLimited),
            (500, json!({"error": "boom"}), ErrorKind::Unavailable),
        ] {
            assert_eq!(classify(status, &body).kind, kind, "{status} {body}");
        }
    }
}
