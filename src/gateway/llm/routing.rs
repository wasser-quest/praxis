use super::{
    error::{CallFailure, ErrorKind, ProviderError},
    provider::{ChatAttempt, ChatRequest, ChatResponse, ContentPart, LLMProvider, ProviderContinuation, ToolCall, StreamDelta, Usage},
    resilience::{self, interruptible, ProviderGate, ResilienceConfig},
    LLMRouter,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

impl LLMRouter {
    pub(crate) fn with_providers(
        providers: Vec<Box<dyn LLMProvider>>,
        default_provider: String,
        fallback_providers: Vec<String>,
        policy: ResilienceConfig,
    ) -> Self {
        let gates = providers
            .iter()
            .map(|p| (p.name().to_string(), Arc::new(ProviderGate::new(&policy))))
            .collect::<HashMap<_, _>>();
        Self {
            providers,
            default_provider,
            fallback_providers,
            policy,
            gates,
        }
    }

    pub fn validate_configuration(&self) -> anyhow::Result<()> {
        self.validate_request_configuration(None)
    }

    /// The trusted session/workflow may select a usable provider even when the
    /// installation default has not been configured yet. No network request.
    pub fn validate_request_configuration(&self, provider: Option<&str>) -> anyhow::Result<()> {
        Self::validate_provider_inventory(&self.policy, &self.default_provider, &self.fallback_providers, &self.provider_names(), provider)
    }

    pub(crate) fn validate_provider_inventory(
        policy: &ResilienceConfig, default: &str, fallbacks: &[String],
        providers: &[String], selected: Option<&str>,
    ) -> anyhow::Result<()> {
        policy.validate()?;
        for name in std::iter::once(selected.unwrap_or(default)).chain(fallbacks.iter().map(String::as_str)) {
            if !providers.iter().any(|provider| provider == name) {
                return Err(missing_provider(name, providers));
            }
        }
        Ok(())
    }

    /// Models a configured provider can serve — the common provider interface
    /// (`praxis-provider-api`). Policy stays here: a listing never enables,
    /// disables or selects a provider for inference.
    pub async fn list_models(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<super::provider::ModelInfo>, super::error::ProviderError> {
        use super::error::{ErrorKind, ProviderError};
        let name = provider.unwrap_or(self.default_provider.as_str());
        self.providers
            .iter()
            .find(|candidate| candidate.name() == name)
            .ok_or_else(|| {
                ProviderError::with_cause(ErrorKind::Configuration, "provider is not configured")
            })?
            .list_models()
            .await
    }

    /// Configured first-attempt bound for application task requests, including
    /// every tool continuation. Explicit low-level requests/summaries keep their
    /// own max_tokens; retries still obey the separate growth ceiling.
    pub fn task_output_tokens(&self) -> u32 {
        // validate_configuration/route reject invalid settings before inference.
        self.policy.max_output_tokens.min(u64::from(u32::MAX)) as u32
    }

    pub fn history_image_messages(&self) -> usize {
        self.policy.history_image_messages.min(2) as usize
    }

    fn find_provider(&self, name: &str) -> Result<&dyn LLMProvider, anyhow::Error> {
        self.providers
            .iter()
            .find(|p| p.name() == name)
            .map(|p| p.as_ref())
            .ok_or_else(|| missing_provider(name, &self.provider_names()))
    }

    pub async fn chat(
        &self,
        request: ChatRequest,
        provider: Option<&str>,
    ) -> anyhow::Result<ChatResponse> {
        self.chat_controlled(request, provider, None, &CancellationToken::new())
            .await
    }

    pub async fn streaming_chat(
        &self,
        request: ChatRequest,
        provider: Option<&str>,
        user_id: &str,
    ) -> anyhow::Result<ChatResponse> {
        let cancel = crate::gateway::task_control::cancellation(user_id).unwrap_or_default();
        self.chat_controlled(request, provider, Some(user_id), &cancel)
            .await
    }

    /// One logical LLM call. This is deliberately BELOW all tool execution:
    /// retries can never replay a terminal command, file write or upload.
    pub async fn chat_controlled(
        &self,
        request: ChatRequest,
        provider: Option<&str>,
        user: Option<&str>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<ChatResponse> {
        let result = self.route(request, provider, user, cancel, None).await;
        if let Err(error) = &result {
            if let Some(user) = user {
                crate::runtime::events::send(user, "stream_abort", "{}");
            }
            resilience::progress(user, &error.to_string());
        }
        result
    }

    pub async fn streaming_chat_traced(&self, request: ChatRequest, provider: Option<&str>, user: &str, trace: &crate::gateway::telemetry::CallTrace) -> anyhow::Result<ChatResponse> {
        let cancel = crate::gateway::task_control::cancellation(user).unwrap_or_default();
        let result = self.route(request, provider, Some(user), &cancel, Some(trace)).await;
        if let Err(error) = &result {
            crate::runtime::events::send(user, "stream_abort", "{}");
            resilience::progress(Some(user), &error.to_string());
        }
        result
    }

    async fn route(
        &self,
        mut request: ChatRequest,
        provider: Option<&str>,
        user: Option<&str>,
        cancel: &CancellationToken,
        trace: Option<&crate::gateway::telemetry::CallTrace>,
    ) -> anyhow::Result<ChatResponse> {
        self.policy.validate()?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(self.policy.total_timeout_ms);
        let base_provider = provider.unwrap_or(&self.default_provider);
        let has_images = request.messages.iter().any(|m| {
            m.content_parts.as_ref().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|p| matches!(p, ContentPart::ImageUrl { .. }))
            })
        });
        let primary = if has_images {
            let selected = request.vision_provider.as_deref().unwrap_or(base_provider);
            if selected != base_provider {
                request.model = None;
            }
            if let Some(model) = &request.vision_model {
                request.model = Some(model.clone());
            }
            selected.to_string()
        } else {
            base_provider.to_string()
        };
        let mut candidates = vec![primary];
        for name in &self.fallback_providers {
            if !candidates.contains(name) {
                candidates.push(name.clone());
            }
        }
        // Missing/typo'd providers are configuration errors, not a reason to
        // silently transmit a user's conversation to a different account.
        for name in &candidates {
            self.find_provider(name)?;
        }

        let mut attempts = 0usize;
        let mut failures = Vec::new();
        let mut terminal = ProviderError::new(ErrorKind::Unavailable);
        let max_attempts = self.policy.max_attempts as usize;
        for (index, name) in candidates.iter().enumerate() {
            if attempts >= max_attempts {
                break;
            }
            let p = self.find_provider(name)?;
            let gate = &self.gates[name];
            let _admission = match gate.admit() {
                Ok(permit) => permit,
                Err(error) => {
                    terminal = error;
                    break;
                }
            };
            let mut req = request.clone();
            // Native continuation state belongs only to this logical call and
            // provider, never to durable history or a configured fallback.
            let mut continuation: Option<ProviderContinuation> = None;
            // Zero is the additive identity only before any successful step.
            // Once a step lacks usage, the logical-call total stays unavailable.
            let mut usage = Some(Usage { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 });
            if index > 0 {
                // Use the explicitly configured fallback's own default model.
                req.model = None;
                req.vision_provider = None;
                req.vision_model = None;
                resilience::progress(
                    user,
                    &format!("LLM switching to configured fallback {}.", label(name)),
                );
            }
            // Reserve one attempt for each remaining explicit fallback. The
            // attempt cap and time budget apply to the WHOLE route, not per hop.
            let allowance =
                max_attempts.saturating_sub((candidates.len() - index - 1).min(max_attempts - 1));
            while attempts < allowance {
                // Output-limit recovery can enlarge the next request. Reserve
                // its full allowance through the same gate before sending it.
                let tokens = resilience::estimated_tokens(&req).saturating_add(
                    continuation.as_ref().map_or(0, ProviderContinuation::estimated_tokens),
                );
                let permit = match interruptible(deadline, cancel, gate.acquire(tokens, user)).await
                {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(error)) | Err(error) => {
                        return Err(CallFailure {
                            attempts,
                            failures,
                            terminal: error,
                        }
                        .into());
                    }
                };
                attempts += 1;
                let attempt_start = Instant::now();
                let attempt_deadline = deadline
                    .min(attempt_start + Duration::from_millis(self.policy.request_timeout_ms));
                let mut stream = StreamAttempt::new(user);
                let trace_id = trace.map(|t| t.attempt(&req, name, attempts as u64)).transpose()?;
                let generation = crate::gateway::telemetry::Generation::new();
                if let Some(user) = user { crate::runtime::events::send(user, "stream_start", "{}"); }
                let on_delta = |delta: StreamDelta| {
                    generation.delta(user, trace_id, &delta);
                    if let Some(user) = user {
                        match &delta {
                            StreamDelta::TemplateOmitted => crate::gateway::task_control::note_template_omitted(user),
                            StreamDelta::Text { text } if !text.is_empty() => {
                                stream.emitted.store(true, Ordering::Relaxed);
                                crate::runtime::events::send(user, "char", text);
                            }
                            StreamDelta::Reasoning { text } if !text.is_empty() && crate::gateway::task_control::show_thinking(user) => {
                                stream.previewed.store(true, Ordering::Relaxed);
                                crate::runtime::events::send(user, "reasoning_delta", text);
                            }
                            StreamDelta::ToolCall { .. } => {
                                if let Ok(data) = serde_json::to_string(&delta) {
                                    stream.previewed.store(true, Ordering::Relaxed);
                                    crate::runtime::events::send(user, "tool_call_delta", &data);
                                }
                            }
                            _ => {}
                        }
                    }
                };
                tracing::info!(provider = %label(name), attempt = attempts, "LLM attempt started");
                tracing::debug!(provider = %label(name), attempt = attempts,
                    model = %req.model.as_deref().map(label).unwrap_or_else(|| "provider default".into()),
                    messages = req.messages.len(), tools = req.tools.as_ref().map_or(0, Vec::len),
                    max_output_tokens = ?req.max_tokens, streaming = user.is_some(),
                    timeout_ms = attempt_deadline.duration_since(attempt_start).as_millis() as u64,
                    "LLM request metadata (content omitted)");
                let call = p.chat_attempt(
                    req.clone(), continuation.as_ref(),
                    if user.is_some() { Some(&on_delta) } else { None },
                );
                let mut reported_response = None;
                let result = match interruptible(attempt_deadline, cancel, call).await {
                    Ok(Ok(attempt)) => {
                        // Keep usage/finish metadata even when validation rejects
                        // a cutoff response. Failed output is never executed.
                        reported_response = Some(attempt.response.clone());
                        let response = &attempt.response;
                        tracing::debug!(provider = %label(name), attempt = attempts,
                            content_bytes = response.content.as_ref().map_or(0, String::len),
                            reasoning_bytes = response.reasoning_content.as_ref().map_or(0, String::len),
                            tool_calls = response.tool_calls.as_ref().map_or(0, Vec::len),
                            finish_reason = match response.finish_reason.as_deref() {
                                Some("stop") => "stop", Some("length") => "length",
                                Some("tool_calls") => "tool_calls", Some("reasoning") => "reasoning",
                                None => "missing", _ => "other",
                            }, "LLM response metadata before validation");
                        // Include cutoff attempts too, without promoting reasoning to
                        // final text. This event is separate from disposable status.
                        if let Some(user) = user.filter(|u| crate::gateway::task_control::show_thinking(u)) {
                            if let Some(text) = response.reasoning_content.as_deref().filter(|s| !s.is_empty()) {
                                crate::runtime::events::send(user, "reasoning", text);
                            }
                        }
                        if attempt.continuation.is_some() {
                            if response.finish_reason.as_deref() != Some("reasoning")
                                || response.content.as_deref().is_some_and(|s| !s.trim().is_empty())
                                || response.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty())
                                || stream.emitted.load(Ordering::Relaxed)
                            {
                                Err(ProviderError::with_cause(ErrorKind::InvalidResponse,
                                    "provider supplied an unsafe reasoning continuation; no tools were accepted"))
                            } else {
                                Ok(attempt)
                            }
                        } else {
                            validate_response(attempt.response).map(|response| ChatAttempt { response, continuation: None })
                        }
                    },
                    Ok(Err(error)) => Err(error),
                    Err(mut error) => {
                        if error.kind == ErrorKind::Deadline && attempt_deadline < deadline {
                            error.kind = ErrorKind::Timeout;
                        }
                        Err(error)
                    }
                };
                if let (Some(trace), Some(id)) = (trace, trace_id) {
                    trace.finish(id, if result.is_ok() {"completed"} else {"failed"}, reported_response.as_ref(), attempt_start.elapsed().as_millis() as u64, generation.first_token_ms())?;
                }
                match result {
                    Ok(ChatAttempt { mut response, continuation: next }) => {
                        if let Some(usage) = &response.usage {
                            let actual = u64::from(usage.total_tokens).max(
                                u64::from(usage.prompt_tokens) + u64::from(usage.completion_tokens),
                            );
                            if actual > 0 {
                                gate.reconcile(permit.reservation, actual);
                            }
                        }
                        add_usage(&mut usage, response.usage.take());
                        if let Some(next) = next {
                            // A completed thinking step is neither a failed
                            // preview nor an assistant answer. Keep its display
                            // and re-enter the same attempt/time/rate gates.
                            stream.committed = true;
                            if let Some(user) = user { crate::runtime::events::send(user, "stream_end", "{}"); }
                            if attempts >= allowance {
                                let error = ProviderError::with_cause(ErrorKind::ReasoningOnly,
                                    "reasoning continuation reached the configured LLM attempt limit; no final answer was returned");
                                failures.push((label(name), error.clone()));
                                return Err(CallFailure { attempts, failures, terminal: error }.into());
                            }
                            continuation = Some(next);
                            resilience::progress(user, &format!(
                                "LLM {}: reasoning step completed; continuing ({}/{}).",
                                label(name), attempts + 1, max_attempts,
                            ));
                            continue;
                        }
                        response.usage = usage;
                        if user.is_some_and(crate::gateway::task_control::template_omitted) && response.tool_calls.is_none() {
                            if let Some(text) = response.content.as_mut() {
                                let marker = crate::gateway::task_control::TEMPLATE_OMITTED_MARKER;
                                if !text.ends_with(marker) { text.push_str("\n\n"); text.push_str(marker); }
                            }
                        }
                        stream.committed = true;
                        if let Some(user) = user { crate::runtime::events::send(user, "stream_end", "{}"); }
                        if let (Some(user), Some(text)) =
                            (user, response.content.as_deref().filter(|s| !s.trim().is_empty()))
                        {
                            crate::runtime::events::send(user, "assistant", text);
                        }
                        tracing::info!(provider = %label(name), attempt = attempts, elapsed_ms = attempt_start.elapsed().as_millis() as u64, content_bytes = response.content.as_ref().map_or(0, String::len), tool_call_count = response.tool_calls.as_ref().map_or(0, Vec::len), "LLM attempt succeeded");
                        return Ok(response);
                    }
                    Err(mut error) => {
                        let original_kind = error.kind;
                        let transient_failure = error.kind.retryable();
                        // Once text has reached the UI, a new generation cannot
                        // safely be appended or silently substituted. Discard
                        // the unfinished bubble and leave recovery to the user.
                        if stream.emitted.load(Ordering::Relaxed)
                            && error.kind != ErrorKind::Cancelled
                        {
                            error.kind = ErrorKind::PartialStream;
                        }
                        tracing::warn!(provider = %label(name), attempt = attempts, elapsed_ms = attempt_start.elapsed().as_millis() as u64, kind = ?error.kind, original_kind = ?original_kind, text_emitted = stream.emitted.load(Ordering::Relaxed), error = %error, "LLM attempt failed");
                        failures.push((label(name), error.clone()));
                        // A known output cutoff is recoverable only before any
                        // visible text, with a strictly larger bounded allowance.
                        // This remains below tool execution and inside the same
                        // global attempt/time/rate limits; no hidden adapter retry.
                        let expanded_output = if error.kind == ErrorKind::OutputLimit
                            && p.supports_output_limit()
                            && attempts < allowance
                        {
                            self.policy.next_output_limit(req.max_tokens)
                        } else {
                            None
                        };
                        if let Some(tokens) = expanded_output {
                            req.max_tokens = Some(tokens);
                        }
                        let retryable = error.kind.retryable() || expanded_output.is_some();
                        terminal = error.clone();
                        if transient_failure {
                            let delay = self
                                .policy
                                .backoff(attempts)
                                .max(error.retry_after.unwrap_or_default());
                            // Even the LAST 429 updates shared cooldown so a new
                            // user task cannot immediately hammer this account.
                            let cause = if original_kind == ErrorKind::RateLimited {
                                resilience::CooldownCause::RateLimit
                            } else {
                                resilience::CooldownCause::TransientFailure
                            };
                            gate.defer(delay, cause);
                            if retryable && attempts < allowance {
                                resilience::progress(
                                    user,
                                    &format!(
                                        "LLM {}: retry {}/{} in at least {:.1}s ({error}).",
                                        label(name),
                                        attempts + 1,
                                        max_attempts,
                                        delay.as_secs_f64()
                                    ),
                                );
                            }
                        }
                        if let Some(tokens) = expanded_output {
                            resilience::progress(user, &format!(
                                "LLM {}: retry {}/{} with {tokens} output tokens after output-token cutoff.",
                                label(name), attempts + 1, max_attempts,
                            ));
                        }
                        drop(stream);
                        drop(permit); // no active permit held during retry sleep
                        if !retryable {
                            return Err(CallFailure {
                                attempts,
                                failures,
                                terminal,
                            }
                            .into());
                        }
                    }
                }
            }
        }
        Err(CallFailure {
            attempts,
            failures,
            terminal,
        }
        .into())
    }

    pub async fn health_check(&self) -> bool {
        if self.validate_configuration().is_err() {
            return false;
        }
        let Ok(provider) = self.find_provider(&self.default_provider) else {
            return false;
        };
        tokio::time::timeout(Duration::from_secs(10), provider.health_check())
            .await
            .unwrap_or(false)
    }
}

fn add_usage(total: &mut Option<Usage>, usage: Option<Usage>) {
    *total = total.as_ref().zip(usage.as_ref()).and_then(|(total, usage)| {
        Some(Usage {
            prompt_tokens: total.prompt_tokens.checked_add(usage.prompt_tokens)?,
            completion_tokens: total.completion_tokens.checked_add(usage.completion_tokens)?,
            total_tokens: total.total_tokens.checked_add(usage.total_tokens)?,
        })
    });
}

fn label(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || "-_./".contains(*ch))
        .take(80)
        .collect()
}

fn missing_provider(name: &str, providers: &[String]) -> anyhow::Error {
    anyhow::anyhow!("LLM '{}': {}. Registered: {}", label(name),
        ProviderError::new(ErrorKind::Configuration),
        providers.iter().map(|s| label(s)).collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
#[path = "local_reliability_tests.rs"]
mod local_reliability_tests;

fn validate_response(mut response: ChatResponse) -> Result<ChatResponse, ProviderError> {
    if response
        .tool_calls
        .as_ref()
        .is_some_and(|calls| calls.is_empty())
    {
        response.tool_calls = None;
    }
    // A cutoff is never a final answer or a safe batch of actions. Recovery is
    // bounded by the router's existing attempt, time and output-token ceilings.
    if response.finish_reason.as_deref() == Some("length") {
        return Err(ProviderError::new(ErrorKind::OutputLimit));
    }
    // Reasoning is displayed separately when requested, never promoted to an
    // answer, persisted as assistant/tool history, or spoken by TTS.
    let has_text = response.content.as_deref().is_some_and(|text| !text.trim().is_empty());
    // Repair and validate tool calls. A token-budget cutoff can land in the
    // middle of a call's JSON arguments (finish_reason=length); that is a
    // recoverable output limit, NOT an invalid provider response. Dropping the
    // whole turn as non-retryable was the reported "invalid provider response"
    // at ~130 s: long thinking + tool call truncated at max_tokens.
    let mut repaired: Vec<ToolCall> = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut invalid_call = None;
    for mut call in response.tool_calls.take().unwrap_or_default() {
        if call.id.is_empty() || !ids.insert(call.id.clone()) {
            call.id = format!("call_{}", repaired.len() + 1);
            while ids.contains(&call.id) {
                call.id = format!("{}_{}", call.id, ids.len() + 1);
            }
            ids.insert(call.id.clone());
        }
        if call.function.name.trim().is_empty() {
            invalid_call = Some("provider returned a tool call without a function name; no tools were executed");
            continue;
        }
        if call.function.arguments.trim().is_empty() {
            // llama.cpp omits arguments for no-argument tools.
            call.function.arguments = "{}".to_string();
            repaired.push(call);
            continue;
        }
        if !serde_json::from_str::<serde_json::Value>(&call.function.arguments)
            .is_ok_and(|args| args.is_object())
        {
            // Truncated or malformed: never execute a half-arguments call.
            invalid_call = Some("tool call arguments are not a valid JSON object (possibly truncated); no tools were executed");
            continue;
        }
        repaired.push(call);
    }
    if let Some(cause) = invalid_call {
        return Err(ProviderError::with_cause(ErrorKind::InvalidResponse, cause));
    }
    if repaired.is_empty() && !has_text {
        let cause = if response.reasoning_content.as_deref().is_some_and(|text| !text.trim().is_empty()) {
            "provider returned reasoning only, without an answer or tool calls"
        } else if response.finish_reason.as_deref() == Some("tool_calls") {
            "provider reported tool_calls but returned no tool calls"
        } else {
            "provider returned no answer text or tool calls"
        };
        return Err(ProviderError::with_cause(ErrorKind::InvalidResponse, cause));
    }
    response.tool_calls = (!repaired.is_empty()).then_some(repaired);
    Ok(response)
}

struct StreamAttempt<'a> {
    user: Option<&'a str>,
    emitted: AtomicBool,
    previewed: AtomicBool,
    committed: bool,
}
impl<'a> StreamAttempt<'a> {
    fn new(user: Option<&'a str>) -> Self {
        if let Some(user) = user {
            crate::runtime::events::send(user, "typing", "true");
        }
        Self {
            user,
            emitted: AtomicBool::new(false),
            previewed: AtomicBool::new(false),
            committed: false,
        }
    }
}
impl Drop for StreamAttempt<'_> {
    fn drop(&mut self) {
        if let Some(user) = self.user {
            // Clear failed reasoning/tool previews as well as text. An attempt
            // with no visible output must not stop the UI timer before a retry.
            if !self.committed && (self.emitted.load(Ordering::Relaxed) || self.previewed.load(Ordering::Relaxed)) {
                crate::runtime::events::send(user, "stream_abort", "{}");
            }
            crate::runtime::events::send(user, "typing", "false");
        }
    }
}
