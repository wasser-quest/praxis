//! Exact llama.cpp request preflight. Durable tool snapshots remain untouched.
use crate::reduction::MAX_PASSES;
use crate::{error::{ErrorKind, ProviderError}, reduction};
use serde_json::{json, Value};

pub(crate) use crate::reduction::MINIMAL_SYSTEM;

pub(crate) const MARGIN: u64 = 64;
pub(crate) const DEFAULT_OUTPUT: u64 = 4096;
pub(crate) const MIN_CONTEXT: u64 = 256;

impl crate::adapter::LlamaCppProvider {
    async fn budget_json(&self, path: &str, body: Option<Value>) -> Result<Value, ProviderError> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let request = if let Some(body) = body {
            self.client.post(url).json(&body)
        } else {
            self.client.get(url)
        };
        let request = match self.api_key.as_deref().filter(|key| !key.is_empty()) {
            Some(key) => request.bearer_auth(key),
            None => request,
        };
        // The preflight is best effort: older or proxied servers lack these
        // endpoints, and a failed or oversized probe must not block the chat
        // call, which reports real errors itself.
        let Ok(response) = request.send().await else {
            return Ok(Value::Null);
        };
        Ok(crate::errors::json(response).await.unwrap_or(Value::Null))
    }

    pub(crate) async fn fit_body(&self, mut body: Value) -> Result<(Value, bool), ProviderError> {
        let props = self.budget_json("/props", None).await?;
        let ctx = props
            .pointer("/default_generation_settings/n_ctx")
            .or_else(|| props.get("n_ctx"))
            .and_then(Value::as_u64)
            .filter(|n| *n >= MIN_CONTEXT)
            .or_else(|| {
                std::env::var("LLAMACPP_CONTEXT_SIZE")
                    .ok()?
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n >= MIN_CONTEXT)
            });
        let Some(ctx) = ctx else {
            return Ok((body, false));
        };
        let output = body["max_tokens"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_OUTPUT)
            .min(ctx / 4)
            .max(1);
        body["max_tokens"] = json!(output);
        let mut template_omitted = false;
        for _ in 0..MAX_PASSES {
            let rendered = self.budget_json("/apply-template", Some(body.clone())).await?;
            // Byte estimates are not token counts. A proxy lacking native
            // preflight must reach send_fitted's confirmed-overflow recovery,
            // not reject valid requests or discard policy on an estimate.
            let Some(prompt) = rendered["prompt"].as_str() else {
                return Ok((body, template_omitted));
            };
            let tokenized = self
                .budget_json(
                    "/tokenize",
                    Some(json!({"content":prompt,"add_special":true,"parse_special":true})),
                )
                .await?;
            let Some(tokens) = tokenized["tokens"]
                .as_array()
                .map(|tokens| tokens.len() as u64)
            else {
                return Ok((body, template_omitted));
            };
            if tokens.saturating_add(output).saturating_add(MARGIN) <= ctx {
                return Ok((body, template_omitted));
            }
            if !reduction::reduce(&mut body) {
                if !template_omitted {
                    if let Some(system) = body["messages"]
                        .as_array_mut()
                        .and_then(|messages| messages.iter_mut().find(|message| message["role"] == "system"))
                    {
                        system["content"] =
                            reduction::budget_system(&reduction::system_text(&system["content"]), MINIMAL_SYSTEM)
                                .into();
                        template_omitted = true;
                        continue;
                    }
                }
                break;
            }
        }
        Err(ProviderError::new(ErrorKind::ContextWindow))
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
