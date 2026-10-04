//! Shared outbound admission, pacing, token reservations and interruptible waits.
//! Every *attempt* (including retries/fallbacks) passes the same provider gate.
use super::{
    error::{ErrorKind, ProviderError},
    provider::{ChatRequest, ContentPart},
};
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ResilienceConfig {
    pub max_attempts: u64,
    /// Initial output allowance for application chat/agent tool-loop requests.
    pub max_output_tokens: u64,
    /// Previous-task image messages eligible for the agent's latest-two image window.
    pub history_image_messages: u64,
    /// Ceiling for output-limit recovery, not an override of the first request.
    pub retry_max_output_tokens: u64,
    pub max_concurrent: u64,
    pub max_queue: u64,
    pub requests_per_minute: u64,
    pub tokens_per_minute: u64,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub request_timeout_ms: u64,
    pub total_timeout_ms: u64,
}
impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            max_output_tokens: 4096,
            history_image_messages: 2,
            retry_max_output_tokens: 16_384,
            max_concurrent: 1,
            max_queue: 32,
            requests_per_minute: 0,
            tokens_per_minute: 0,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
            request_timeout_ms: 180_000,
            total_timeout_ms: 300_000,
        }
    }
}
impl ResilienceConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }
    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let default = Self::default();
        let number = |key, fallback| {
            get(key)
                .map(|s| s.parse().unwrap_or(u64::MAX))
                .unwrap_or(fallback)
        };
        Self {
            max_attempts: number("LLM_MAX_ATTEMPTS", default.max_attempts),
            max_output_tokens: number("LLM_MAX_OUTPUT_TOKENS", default.max_output_tokens),
            history_image_messages: number("LLM_HISTORY_IMAGE_MESSAGES", default.history_image_messages),
            retry_max_output_tokens: number("LLM_RETRY_MAX_OUTPUT_TOKENS", default.retry_max_output_tokens),
            max_concurrent: number("LLM_MAX_CONCURRENT", default.max_concurrent),
            max_queue: number("LLM_MAX_QUEUE", default.max_queue),
            requests_per_minute: number("LLM_REQUESTS_PER_MINUTE", default.requests_per_minute),
            tokens_per_minute: number("LLM_TOKENS_PER_MINUTE", default.tokens_per_minute),
            initial_backoff_ms: number("LLM_RETRY_BASE_MS", default.initial_backoff_ms),
            max_backoff_ms: number("LLM_RETRY_MAX_MS", default.max_backoff_ms),
            request_timeout_ms: number("LLM_REQUEST_TIMEOUT_MS", default.request_timeout_ms),
            total_timeout_ms: number("LLM_TOTAL_TIMEOUT_MS", default.total_timeout_ms),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        for (key, value, min, max) in [
            ("LLM_MAX_ATTEMPTS", self.max_attempts, 1, 20),
            ("LLM_MAX_OUTPUT_TOKENS", self.max_output_tokens, 1, 1_048_576),
            ("LLM_HISTORY_IMAGE_MESSAGES", self.history_image_messages, 0, 2),
            ("LLM_RETRY_MAX_OUTPUT_TOKENS", self.retry_max_output_tokens, 0, 1_048_576),
            ("LLM_MAX_CONCURRENT", self.max_concurrent, 1, 64),
            ("LLM_MAX_QUEUE", self.max_queue, 0, 10_000),
            (
                "LLM_REQUESTS_PER_MINUTE",
                self.requests_per_minute,
                0,
                1_000_000,
            ),
            (
                "LLM_TOKENS_PER_MINUTE",
                self.tokens_per_minute,
                0,
                1_000_000_000,
            ),
            ("LLM_RETRY_BASE_MS", self.initial_backoff_ms, 1, 300_000),
            ("LLM_RETRY_MAX_MS", self.max_backoff_ms, 1, 300_000),
            (
                "LLM_REQUEST_TIMEOUT_MS",
                self.request_timeout_ms,
                1,
                86_400_000,
            ),
            ("LLM_TOTAL_TIMEOUT_MS", self.total_timeout_ms, 1, 86_400_000),
        ] {
            anyhow::ensure!(
                (min..=max).contains(&value),
                "{key} must be an integer in {min}..={max}"
            );
        }
        anyhow::ensure!(
            self.initial_backoff_ms <= self.max_backoff_ms,
            "LLM_RETRY_BASE_MS must not exceed LLM_RETRY_MAX_MS"
        );
        Ok(())
    }
    /// Never blindly repeat a token-exhausted request with the same allowance.
    /// None/zero has no usable bound to grow; a ceiling of zero disables recovery.
    pub(crate) fn next_output_limit(&self, current: Option<u32>) -> Option<u32> {
        let current = current.filter(|tokens| *tokens > 0)?;
        let next = (u64::from(current) * 2)
            .min(self.retry_max_output_tokens)
            .min(u64::from(u32::MAX)) as u32;
        (next > current).then_some(next)
    }

    pub fn backoff(&self, attempt: usize) -> Duration {
        use rand::Rng;
        let ceiling = self
            .initial_backoff_ms
            .saturating_mul(1u64 << attempt.saturating_sub(1).min(20))
            .min(self.max_backoff_ms);
        // Equal jitter avoids both synchronization and a zero-delay retry storm.
        Duration::from_millis(rand::thread_rng().gen_range((ceiling / 2).max(1)..=ceiling))
    }
}

pub async fn interruptible<T>(
    deadline: Instant,
    cancel: &CancellationToken,
    work: impl Future<Output = T>,
) -> Result<T, ProviderError> {
    if cancel.is_cancelled() {
        return Err(ProviderError::new(ErrorKind::Cancelled));
    }
    if Instant::now() >= deadline {
        return Err(ProviderError::new(ErrorKind::Deadline));
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::new(ErrorKind::Cancelled)),
        _ = tokio::time::sleep_until(deadline) => Err(ProviderError::new(ErrorKind::Deadline)),
        result = work => Ok(result),
    }
}

pub fn progress(user: Option<&str>, message: &str) {
    if let Some(user) = user {
        crate::runtime::events::send(user, "feedback", message);
    }
}

/// Deliberately approximate; not a model tokenizer. Reserve prompt + maximum
/// completion for every attempt, then reconcile real usage when available.
pub fn estimated_tokens(request: &ChatRequest) -> u64 {
    let mut bytes = 0u64;
    let mut images = 0u64;
    for message in &request.messages {
        bytes = bytes.saturating_add(message.content.as_ref().map_or(0, |s| s.len() as u64) + 32);
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                bytes = bytes.saturating_add(
                    call.function.arguments.len() as u64 + call.function.name.len() as u64 + 32,
                );
            }
        }
        if let Some(parts) = &message.content_parts {
            for part in parts {
                match part {
                    ContentPart::Text { text } => bytes = bytes.saturating_add(text.len() as u64),
                    ContentPart::ImageUrl { .. } => images += 1,
                }
            }
        }
    }
    if let Some(tools) = &request.tools {
        bytes = bytes.saturating_add(serde_json::to_vec(tools).map_or(0, |s| s.len() as u64));
    }
    (bytes / 3)
        .saturating_add(images.saturating_mul(4096))
        .saturating_add(u64::from(request.max_tokens.unwrap_or(4096)))
        .max(1)
}

struct TokenCharge {
    id: u64,
    at: Instant,
    tokens: u64,
}
struct Schedule {
    next_request: Instant,
    cooldown: Option<Instant>, // None means Retry-After exceeded representable time.
    cooldown_cause: CooldownCause,
    charges: VecDeque<TokenCharge>,
    sequence: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum CooldownCause {
    RateLimit,
    TransientFailure,
}
impl CooldownCause {
    fn description(self) -> &'static str {
        match self {
            Self::RateLimit => "provider rate limit",
            Self::TransientFailure => "provider retry backoff",
        }
    }
}
pub(crate) struct ProviderGate {
    admission: Arc<Semaphore>,
    active: Arc<Semaphore>,
    schedule: Mutex<Schedule>,
    policy: ResilienceConfig,
}
pub(crate) struct AttemptPermit {
    _active: OwnedSemaphorePermit,
    pub reservation: u64,
}
impl ProviderGate {
    pub fn new(policy: &ResilienceConfig) -> Self {
        // Construction is infallible even for invalid programmatic config;
        // the router validates it before permitting any network request.
        let concurrent = policy.max_concurrent.clamp(1, 64) as usize;
        Self {
            admission: Arc::new(Semaphore::new(
                concurrent + policy.max_queue.min(10_000) as usize,
            )),
            active: Arc::new(Semaphore::new(concurrent)),
            schedule: Mutex::new(Schedule {
                next_request: Instant::now(),
                cooldown: Some(Instant::now()),
                cooldown_cause: CooldownCause::TransientFailure,
                charges: VecDeque::new(),
                sequence: 0,
            }),
            policy: policy.clone(),
        }
    }
    pub fn admit(&self) -> Result<OwnedSemaphorePermit, ProviderError> {
        self.admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::new(ErrorKind::QueueFull))
    }
    pub fn defer(&self, delay: Duration, cause: CooldownCause) {
        let mut state = self.schedule.lock().unwrap_or_else(|e| e.into_inner());
        match (state.cooldown, Instant::now().checked_add(delay)) {
            (Some(old), Some(new)) if new > old => {
                state.cooldown = Some(new);
                state.cooldown_cause = cause;
            }
            (Some(_), None) => {
                state.cooldown = None;
                state.cooldown_cause = cause;
            }
            _ => {}
        }
    }
    pub async fn acquire(
        &self,
        tokens: u64,
        user: Option<&str>,
    ) -> Result<AttemptPermit, ProviderError> {
        if self.policy.tokens_per_minute > 0 && tokens > self.policy.tokens_per_minute {
            return Err(ProviderError::new(ErrorKind::TokenBudget));
        }
        if self.active.available_permits() == 0 {
            progress(
                user,
                "LLM queued: waiting for the provider's active request.",
            );
        }
        let permit = self
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ProviderError::new(ErrorKind::Cancelled))?;
        loop {
            let now = Instant::now();
            let wait;
            let reason;
            {
                let mut state = self.schedule.lock().unwrap_or_else(|e| e.into_inner());
                while state
                    .charges
                    .front()
                    .is_some_and(|charge| now.duration_since(charge.at) >= Duration::from_secs(60))
                {
                    state.charges.pop_front();
                }
                let mut ready = state.cooldown.ok_or_else(|| ProviderError::new(ErrorKind::Deadline))?;
                let mut cause = state.cooldown_cause.description();
                if state.next_request > ready {
                    ready = state.next_request;
                    cause = "configured request pacing";
                }
                if self.policy.tokens_per_minute > 0 {
                    let mut used = state
                        .charges
                        .iter()
                        .fold(0u64, |sum, charge| sum.saturating_add(charge.tokens));
                    for charge in &state.charges {
                        if used.saturating_add(tokens) <= self.policy.tokens_per_minute {
                            break;
                        }
                        let token_ready = charge.at + Duration::from_secs(60);
                        if token_ready > ready {
                            ready = token_ready;
                            cause = "configured token-per-minute budget";
                        }
                        used = used.saturating_sub(charge.tokens);
                    }
                }
                if ready <= now {
                    if self.policy.requests_per_minute > 0 {
                        state.next_request = now
                            + Duration::from_secs_f64(
                                60.0 / self.policy.requests_per_minute as f64,
                            );
                    }
                    state.sequence = state.sequence.wrapping_add(1);
                    let id = state.sequence;
                    if self.policy.tokens_per_minute > 0 {
                        state.charges.push_back(TokenCharge {
                            id,
                            at: now,
                            tokens,
                        });
                    }
                    return Ok(AttemptPermit {
                        _active: permit,
                        reservation: id,
                    });
                }
                wait = ready;
                reason = cause;
            }
            progress(
                user,
                &format!(
                    "LLM waiting {:.1}s for {reason}.",
                    wait.duration_since(now).as_secs_f64(),
                ),
            );
            tokio::time::sleep_until(wait).await;
            // Recheck under the lock: another attempt may have extended a shared
            // cooldown, even while this job was already waiting.
        }
    }
    pub fn reconcile(&self, reservation: u64, actual_tokens: u64) {
        if self.policy.tokens_per_minute == 0 {
            return;
        }
        let mut state = self.schedule.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(charge) = state
            .charges
            .iter_mut()
            .find(|charge| charge.id == reservation)
        {
            charge.tokens = actual_tokens;
        }
    }
}
