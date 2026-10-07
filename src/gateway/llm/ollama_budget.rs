//! Ollama request preflight: keep prompt + output inside the runtime window.
//!
//! Incident-shaped failure this prevents: Ollama runs a model with a runtime
//! context far below its trained one (observed: 8192 for a 262144-token model
//! on 0.35) and clamps generation to `num_ctx - prompt`, reporting
//! `done_reason="length"` at a handful of tokens no matter how large
//! `num_predict` was. The reduction ladder below is the proven llama.cpp one.
use crate::gateway::llm::error::{ErrorKind, ProviderError};
use crate::gateway::llm::llamacpp::budget::{reduce, MINIMAL_SYSTEM};
use serde_json::Value;

/// Window requested when the operator did not choose one. Large enough for a
/// full agent turn (prompt half + output half) on every served model we saw.
pub(super) const DEFAULT_WINDOW: u64 = 32_768;
/// Observed server floor: a requested 256-token window was loaded as 2048.
pub(super) const MIN_WINDOW: u64 = 2_048;
/// Slack for chat template overhead not visible in the message payload.
const MARGIN: u64 = 64;

/// Model's trained context length from `GET /api/tags` (`details.context_length`).
/// Best effort and cached by the caller: servers without it keep the legacy
/// behavior (no `num_ctx`, no output clamp). GET keeps POST-based request
/// accounting in tests and tooling undisturbed.
pub(super) async fn probe_context_window(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Option<u64> {
    let mut request = client
        .get(format!("{}/api/tags", base_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5));
    if let Some(key) = api_key.as_deref().filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let data: Value = response.json().await.ok()?;
    data["models"].as_array()?.iter().find_map(|entry| {
        (entry["name"].as_str()? == model)
            .then(|| entry.pointer("/details/context_length").and_then(Value::as_u64))
            .flatten()
    })
}

/// Deliberately approximate (bytes/3), matching `resilience::estimated_tokens`.
/// Overestimating ASCII prompts slightly is safe: it only triggers the ladder
/// earlier. Images count as flat 4096-token budgets like the TPM accounting.
pub(super) fn estimate_prompt_tokens(body: &Value) -> u64 {
    let mut bytes = 0u64;
    let mut images = 0u64;
    if let Some(messages) = body["messages"].as_array() {
        for message in messages {
            bytes = bytes.saturating_add(
                message["content"].as_str().map_or(0, |text| text.len() as u64) + 32,
            );
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    bytes = bytes.saturating_add(serde_json::to_vec(call).map_or(0, |raw| raw.len() as u64));
                }
            }
            images += message["images"].as_array().map_or(0, Vec::len) as u64;
        }
    }
    if let Some(tools) = body.get("tools") {
        bytes = bytes.saturating_add(serde_json::to_vec(tools).map_or(0, |raw| raw.len() as u64));
    }
    (bytes / 3)
        .saturating_add(images.saturating_mul(4096))
        .max(1)
}

/// Fit `prompt + output + margin` into `window`. The output half is already
/// reserved by `num_predict`; the ladder shrinks only the prompt side:
/// tool schemas, then saved tool outputs (durable `read_tool_result` handles
/// stay intact), then whole old turns, then the minimal system prompt.
/// Returns whether the configured template had to be omitted.
pub(super) fn fit_body(body: &mut Value, window: u64, output: u64) -> Result<bool, ProviderError> {
    let budget = window.saturating_sub(output).saturating_sub(MARGIN);
    let mut template_omitted = false;
    for _ in 0..34 {
        if estimate_prompt_tokens(body) <= budget {
            return Ok(template_omitted);
        }
        if !reduce(body) {
            if !template_omitted {
                if let Some(system) = body["messages"]
                    .as_array_mut()
                    .and_then(|messages| messages.iter_mut().find(|m| m["role"] == "system"))
                {
                    system["content"] = MINIMAL_SYSTEM.into();
                    template_omitted = true;
                    continue;
                }
            }
            break;
        }
    }
    Err(ProviderError::new(ErrorKind::ContextWindow))
}
