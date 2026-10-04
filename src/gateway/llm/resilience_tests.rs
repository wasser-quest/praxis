//! Offline contract tests: no credentials, live provider, service or database.
use super::{
    error::{ErrorKind, ProviderError},
    provider::*,
    resilience::ResilienceConfig,
    LLMRouter,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum Step {
    Reply,
    Fail(ProviderError),
    Slow(Duration),
    Usage(u32),
    Partial,
}

type Calls = Arc<Mutex<Vec<(Instant, ChatRequest, bool)>>>;
struct Scripted {
    name: &'static str,
    steps: Mutex<VecDeque<Step>>,
    calls: Calls,
}
impl Scripted {
    fn boxed(name: &'static str, steps: Vec<Step>) -> (Box<dyn LLMProvider>, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(Self {
                name,
                steps: Mutex::new(steps.into()),
                calls: calls.clone(),
            }),
            calls,
        )
    }
    async fn run(
        &self,
        request: ChatRequest,
        stream: bool,
        token: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<ChatResponse> {
        self.calls
            .lock()
            .unwrap()
            .push((Instant::now(), request, stream));
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Reply);
        match step {
            Step::Reply => {}
            Step::Fail(e) => return Err(e.into()),
            Step::Slow(delay) => tokio::time::sleep(delay).await,
            Step::Usage(tokens) => {
                let mut r = reply();
                r.usage = Some(Usage {
                    prompt_tokens: tokens,
                    completion_tokens: 0,
                    total_tokens: tokens,
                });
                return Ok(r);
            }
            Step::Partial => {
                token("partial".into());
                return Err(ProviderError::new(ErrorKind::Interrupted).into());
            }
        }
        Ok(reply())
    }
}
#[async_trait::async_trait]
impl LLMProvider for Scripted {
    fn name(&self) -> &str {
        self.name
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.run(request, false, &|_| {}).await
    }
    async fn chat_stream(
        &self,
        request: ChatRequest,
        token: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<ChatResponse> {
        self.run(request, true, token).await
    }
}
fn reply() -> ChatResponse {
    ChatResponse {
        reasoning_content: None,
        content: Some("done".into()),
        tool_calls: None,
        finish_reason: Some("stop".into()),
        usage: None,
    }
}
fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage {
    reasoning_content: None,
            role: "user".into(),
            content: Some("synthetic request".into()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }],
        tools: None,
        temperature: None,
        max_tokens: Some(10),
        model: None,
        vision_provider: None,
        vision_model: None,
        thinking: None,
    }
}
fn policy() -> ResilienceConfig {
    ResilienceConfig {
        initial_backoff_ms: 1000,
        max_backoff_ms: 8000,
        request_timeout_ms: 120_000,
        total_timeout_ms: 300_000,
        ..Default::default()
    }
}
fn router(provider: Box<dyn LLMProvider>, policy: ResilienceConfig) -> LLMRouter {
    let name = provider.name().to_string();
    LLMRouter::with_providers(vec![provider], name, vec![], policy)
}

#[tokio::test(start_paused = true)]
async fn resilience_retries_same_request_and_honors_retry_after() {
    let mut error = ProviderError::new(ErrorKind::RateLimited);
    error.retry_after = Some(Duration::from_secs(12));
    let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(error), Step::Reply]);
    let r = router(p, policy());
    assert_eq!(
        r.chat(request(), None).await.unwrap().content.as_deref(),
        Some("done")
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].0.duration_since(calls[0].0) >= Duration::from_secs(12));
    assert_eq!(
        serde_json::to_value(&calls[0].1).unwrap(),
        serde_json::to_value(&calls[1].1).unwrap()
    );
}

#[tokio::test(start_paused = true)]
async fn resilience_permanent_errors_do_not_retry_or_fallback() {
    for kind in [
        ErrorKind::Authentication,
        ErrorKind::QuotaExhausted,
        ErrorKind::InvalidRequest,
        ErrorKind::InvalidResponse,
    ] {
        let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(ProviderError::new(kind))]);
        let (f, fallback_calls) = Scripted::boxed("fallback", vec![]);
        let r = LLMRouter::with_providers(
            vec![p, f],
            "primary".into(),
            vec!["fallback".into()],
            policy(),
        );
        let e = r.chat(request(), None).await.unwrap_err().to_string();
        assert!(e.contains("primary"), "{e}");
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(fallback_calls.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_retry_budget_is_global_including_explicit_fallback() {
    let steps = vec![Step::Fail(ProviderError::new(ErrorKind::Unavailable)); 10];
    let (p, calls) = Scripted::boxed("primary", steps.clone());
    let (f, fcalls) = Scripted::boxed("fallback", steps);
    let r = LLMRouter::with_providers(
        vec![p, f],
        "primary".into(),
        vec!["fallback".into()],
        policy(),
    );
    let e = r.chat(request(), None).await.unwrap_err().to_string();
    assert_eq!(
        calls.lock().unwrap().len() + fcalls.lock().unwrap().len(),
        5
    );
    assert!(!fcalls.lock().unwrap().is_empty());
    assert!(e.contains("primary") && e.contains("fallback"), "{e}");
}

#[tokio::test(start_paused = true)]
async fn resilience_no_implicit_fallback_and_missing_provider_is_configuration_error() {
    let (p, calls) = Scripted::boxed("ollama", vec![]);
    let r = LLMRouter::with_providers(vec![p], "openai".into(), vec![], policy());
    assert!(r.validate_configuration().is_err());
    let e = r.chat(request(), None).await.unwrap_err().to_string();
    assert!(e.contains("not configured"), "{e}");
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn resilience_fallback_does_not_receive_primary_model_override() {
    let (p, _) = Scripted::boxed(
        "primary",
        vec![Step::Fail(ProviderError::new(ErrorKind::Unavailable)); 5],
    );
    let (f, calls) = Scripted::boxed("fallback", vec![]);
    let r = LLMRouter::with_providers(
        vec![p, f],
        "primary".into(),
        vec!["fallback".into()],
        policy(),
    );
    let mut req = request();
    req.model = Some("primary-only-model".into());
    r.chat(req, None).await.unwrap();
    assert!(calls.lock().unwrap()[0].1.model.is_none());
}

#[tokio::test(start_paused = true)]
async fn resilience_streaming_honors_vision_routing() {
    let (p, calls) = Scripted::boxed("primary", vec![]);
    let (v, vcalls) = Scripted::boxed("vision", vec![]);
    let r = LLMRouter::with_providers(vec![p, v], "primary".into(), vec![], policy());
    let mut req = request();
    req.vision_provider = Some("vision".into());
    req.vision_model = Some("vision-model".into());
    req.messages[0].content_parts = Some(vec![ContentPart::ImageUrl {
        image_url: ImageUrlDetail {
            url: "data:image/png;base64,AA==".into(),
            detail: None,
        },
    }]);
    r.streaming_chat(req, None, "resilience-vision")
        .await
        .unwrap();
    assert!(calls.lock().unwrap().is_empty());
    let calls = vcalls.lock().unwrap();
    assert_eq!(calls[0].1.model.as_deref(), Some("vision-model"));
    assert!(calls[0].2);
}

#[tokio::test(start_paused = true)]
async fn resilience_partial_stream_is_not_replayed_or_committed() {
    let user = "resilience-partial";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let (p, calls) = Scripted::boxed("primary", vec![Step::Partial, Step::Reply]);
    let r = router(p, policy());
    let e = r
        .streaming_chat(request(), None, user)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("partial"), "{e}");
    assert_eq!(calls.lock().unwrap().len(), 1);
    let mut abort = false;
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "assistant");
        abort |= event.event == "stream_abort";
    }
    assert!(abort);
}

#[tokio::test(start_paused = true)]
async fn resilience_requests_longer_than_thirty_seconds_can_succeed() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(65))]);
    let r = router(p, policy());
    let start = Instant::now();
    r.chat(request(), None).await.unwrap();
    assert!(start.elapsed() >= Duration::from_secs(65));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn resilience_total_deadline_includes_attempts_and_backoff() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(100)); 10]);
    let r = router(
        p,
        ResilienceConfig {
            request_timeout_ms: 10_000,
            total_timeout_ms: 25_000,
            ..policy()
        },
    );
    let start = Instant::now();
    assert!(r.chat(request(), None).await.is_err());
    assert!(start.elapsed() <= Duration::from_secs(25));
    assert!(calls.lock().unwrap().len() <= 3);
}

#[tokio::test(start_paused = true)]
async fn resilience_cancellation_interrupts_inflight_and_backoff() {
    for step in [
        Step::Slow(Duration::from_secs(100)),
        Step::Fail(ProviderError::new(ErrorKind::RateLimited)),
    ] {
        let (p, calls) = Scripted::boxed("primary", vec![step]);
        let r = router(p, policy());
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let start = Instant::now();
        let e = r
            .chat_controlled(request(), None, None, &cancellation)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("cancelled"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_concurrency_and_request_pacing_are_shared() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(3)); 4]);
    let r = Arc::new(router(
        p,
        ResilienceConfig {
            requests_per_minute: 12,
            ..policy()
        },
    ));
    let mut jobs = vec![];
    for _ in 0..4 {
        let r = r.clone();
        jobs.push(tokio::spawn(async move {
            r.chat(request(), None).await.unwrap()
        }));
    }
    for job in jobs {
        job.await.unwrap();
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for pair in calls.windows(2) {
        assert!(pair[1].0.duration_since(pair[0].0) >= Duration::from_secs(5));
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_retry_after_beyond_deadline_prevents_another_call() {
    let mut e = ProviderError::new(ErrorKind::RateLimited);
    e.retry_after = Some(Duration::from_secs(600));
    let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(e)]);
    let r = router(p, policy());
    assert!(r.chat(request(), None).await.is_err());
    assert_eq!(calls.lock().unwrap().len(), 1);
    // A separate task must honor the same cooldown too.
    assert!(r.chat(request(), None).await.is_err());
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn resilience_invalid_policy_values_are_rejected() {
    for p in [
        ResilienceConfig {
            max_attempts: 0,
            ..policy()
        },
        ResilienceConfig {
            max_concurrent: 0,
            ..policy()
        },
        ResilienceConfig {
            total_timeout_ms: 0,
            ..policy()
        },
        ResilienceConfig {
            max_backoff_ms: 0,
            ..policy()
        },
    ] {
        assert!(p.validate().is_err());
    }
    assert!(
        ResilienceConfig::from_lookup(|_| Some("not-a-number".into()))
            .validate()
            .is_err()
    );
    assert!(ResilienceConfig::from_lookup(|_| None).validate().is_ok());
    for attempt in 1..100 {
        let p = policy();
        let delay = p.backoff(attempt);
        assert!(!delay.is_zero());
        assert!(delay <= Duration::from_millis(p.max_backoff_ms));
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_bounded_queue_and_cancel_release_permits() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(100))]);
    let r = Arc::new(router(
        p,
        ResilienceConfig {
            max_queue: 0,
            ..policy()
        },
    ));
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let router = r.clone();
    let first =
        tokio::spawn(async move { router.chat_controlled(request(), None, None, &c).await });
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(r
        .chat(request(), None)
        .await
        .unwrap_err()
        .to_string()
        .contains("queue full"));
    cancel.cancel();
    assert!(first
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert!(r.chat(request(), None).await.is_ok());
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn resilience_queued_request_can_be_cancelled_without_an_attempt() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(10))]);
    let r = Arc::new(router(p, policy()));
    let router = r.clone();
    let first = tokio::spawn(async move { router.chat(request(), None).await.unwrap() });
    tokio::task::yield_now().await;
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        c.cancel();
    });
    assert!(r
        .chat_controlled(request(), None, None, &cancel)
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert_eq!(calls.lock().unwrap().len(), 1);
    first.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn resilience_token_limit_applies_to_retries_and_oversized_requests_fail_fast() {
    let tokens = super::resilience::estimated_tokens(&request());
    let (p, calls) = Scripted::boxed(
        "primary",
        vec![Step::Fail(ProviderError::new(ErrorKind::Unavailable))],
    );
    let r = router(
        p,
        ResilienceConfig {
            tokens_per_minute: tokens,
            ..policy()
        },
    );
    r.chat(request(), None).await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].0.duration_since(calls[0].0) >= Duration::from_secs(60));
    drop(calls);
    let mut huge = request();
    huge.max_tokens = Some(100_000);
    assert!(r
        .chat(huge, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("LLM_TOKENS_PER_MINUTE"));
}

#[tokio::test(start_paused = true)]
async fn resilience_actual_usage_refunds_token_reservation() {
    let tokens = super::resilience::estimated_tokens(&request());
    let (p, calls) = Scripted::boxed("primary", vec![Step::Usage(1), Step::Reply]);
    let r = router(
        p,
        ResilienceConfig {
            tokens_per_minute: tokens + 1,
            ..policy()
        },
    );
    r.chat(request(), None).await.unwrap();
    r.chat(request(), None).await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].0, calls[1].0);
}

#[tokio::test(start_paused = true)]
async fn resilience_cooldown_is_shared_across_parallel_tasks() {
    let mut error = ProviderError::new(ErrorKind::RateLimited);
    error.retry_after = Some(Duration::from_secs(12));
    let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(error)]);
    let r = Arc::new(router(p, policy()));
    let router = r.clone();
    let first = tokio::spawn(async move { router.chat(request(), None).await.unwrap() });
    tokio::task::yield_now().await;
    r.chat(request(), None).await.unwrap();
    first.await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for call in calls.iter().skip(1) {
        assert!(call.0.duration_since(calls[0].0) >= Duration::from_secs(12));
    }
}

#[test]
fn resilience_long_task_output_and_time_budgets_are_configurable_and_bounded() {
    let config = ResilienceConfig::from_lookup(|key| match key {
        "LLM_MAX_OUTPUT_TOKENS" => Some("16384".into()),
        "LLM_HISTORY_IMAGE_MESSAGES" => Some("0".into()),
        "LLM_RETRY_MAX_OUTPUT_TOKENS" => Some("32768".into()),
        "LLM_REQUEST_TIMEOUT_MS" => Some("600000".into()),
        "LLM_TOTAL_TIMEOUT_MS" => Some("1800000".into()),
        _ => None,
    });
    config.validate().unwrap();
    assert_eq!(config.max_output_tokens, 16_384);
    assert_eq!(config.history_image_messages, 0);
    for value in ["3", "-1", "invalid"] {
        let invalid = ResilienceConfig::from_lookup(|key| (key == "LLM_HISTORY_IMAGE_MESSAGES").then(|| value.into()));
        assert!(invalid.validate().unwrap_err().to_string().contains("LLM_HISTORY_IMAGE_MESSAGES"));
    }
    assert_eq!(config.next_output_limit(Some(16_384)), Some(32_768));
    assert_eq!(config.request_timeout_ms, 600_000);
    assert_eq!(config.total_timeout_ms, 1_800_000);
    assert_eq!(ResilienceConfig::default().max_output_tokens, 4096);
    for value in ["", "0", "-1", "not-a-number", "1048577"] {
        let invalid = ResilienceConfig::from_lookup(|key| (key == "LLM_MAX_OUTPUT_TOKENS").then(|| value.into()));
        assert!(invalid.validate().unwrap_err().to_string().contains("LLM_MAX_OUTPUT_TOKENS"));
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_long_task_budget_allows_slow_calls_but_keeps_cancellation() {
    for cancelled in [false, true] {
        let (p, calls) = Scripted::boxed("primary", vec![Step::Slow(Duration::from_secs(240))]);
        let r = router(p, ResilienceConfig {
            max_output_tokens: 16_384,
            request_timeout_ms: 600_000,
            total_timeout_ms: 1_800_000,
            ..policy()
        });
        assert_eq!(r.task_output_tokens(), 16_384);
        let cancel = CancellationToken::new();
        if cancelled {
            let token = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(181)).await;
                token.cancel();
            });
        }
        let start = Instant::now();
        let result = r.chat_controlled(request(), None, None, &cancel).await;
        if cancelled {
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            assert_eq!(start.elapsed(), Duration::from_secs(181));
        } else {
            assert_eq!(result.unwrap().content.as_deref(), Some("done"));
            assert_eq!(start.elapsed(), Duration::from_secs(240));
        }
        assert_eq!(calls.lock().unwrap().len(), 1, "never replay the slow request before its configured deadline");
    }
}

#[test]
fn resilience_output_limit_configuration_and_arithmetic_are_bounded() {
    let defaults = ResilienceConfig::from_lookup(|_| None);
    assert_eq!(defaults.retry_max_output_tokens, 16_384);
    assert_eq!(defaults.next_output_limit(Some(4096)), Some(8192));
    assert_eq!(defaults.next_output_limit(Some(8192)), Some(16_384));
    for bound in [None, Some(0), Some(16_384), Some(u32::MAX)] {
        assert_eq!(defaults.next_output_limit(bound), None);
    }
    for value in ["not-a-number", "-1", "1048577"] {
        let config = ResilienceConfig::from_lookup(|key| {
            (key == "LLM_RETRY_MAX_OUTPUT_TOKENS").then(|| value.into())
        });
        assert!(config.validate().unwrap_err().to_string().contains("LLM_RETRY_MAX_OUTPUT_TOKENS"));
    }
    assert!(!ErrorKind::OutputLimit.retryable(), "needs a larger allowance, not blind replay");
}

#[tokio::test(start_paused = true)]
async fn resilience_output_limit_recovery_obeys_deadline_and_cancellation() {
    for cancelled in [false, true] {
        let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(ProviderError::new(ErrorKind::OutputLimit)), Step::Slow(Duration::from_secs(1))]);
        let r = router(p, ResilienceConfig { total_timeout_ms: 200, ..policy() });
        let cancel = CancellationToken::new();
        if cancelled {
            let token = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                token.cancel();
            });
        }
        let start = Instant::now();
        let error = r.chat_controlled(request(), None, None, &cancel).await.unwrap_err().to_string();
        assert!(error.contains(if cancelled { "cancelled" } else { "time budget exhausted" }), "{error}");
        assert!(start.elapsed() <= Duration::from_millis(200));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_output_limit_without_a_growable_bound_never_retries_or_falls_back() {
    for bound in [None, Some(0), Some(16_384), Some(u32::MAX)] {
        let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(ProviderError::new(ErrorKind::OutputLimit))]);
        let (f, fallback_calls) = Scripted::boxed("fallback", vec![]);
        let r = LLMRouter::with_providers(vec![p, f], "primary".into(), vec!["fallback".into()], policy());
        let mut req = request();
        req.max_tokens = bound;
        let error = r.chat(req, None).await.unwrap_err().to_string();
        assert!(error.contains("output token limit"), "{error}");
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(fallback_calls.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_output_expansion_does_not_create_a_provider_cooldown() {
    let (p, calls) = Scripted::boxed("primary", vec![Step::Fail(ProviderError::new(ErrorKind::OutputLimit)), Step::Reply]);
    let r = router(p, ResilienceConfig {initial_backoff_ms:20_000,max_backoff_ms:20_000,total_timeout_ms:5000,..policy()});
    let user = "output-expansion-no-rate-limit";
    let mut events = crate::runtime::events::get_or_create(user).subscribe();
    let start = Instant::now();
    let mut req = request();
    req.max_tokens = Some(4096);
    let response = r.chat_controlled(req, None, Some(user), &CancellationToken::new()).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("done"));
    assert_eq!(start.elapsed(), Duration::ZERO);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1.max_tokens, Some(8192));
    while let Ok(event) = events.try_recv() {
        assert!(!event.data.contains("LLM waiting"), "{}", event.data);
    }
}

#[tokio::test(start_paused = true)]
async fn resilience_wait_messages_distinguish_local_pacing_from_provider_failures() {
    for (index, cause) in ["configured request pacing", "configured token-per-minute budget", "provider rate limit", "provider retry backoff"].into_iter().enumerate() {
        let user = format!("wait-cause-{index}");
        let mut events = crate::runtime::events::get_or_create(&user).subscribe();
        let mut config = policy();
        let steps = match index {
            0 => { config.requests_per_minute = 6; vec![Step::Reply, Step::Reply] }
            1 => { config.tokens_per_minute = super::resilience::estimated_tokens(&request()); vec![Step::Reply, Step::Reply] }
            _ => {
                let mut error = ProviderError::new(if index == 2 { ErrorKind::RateLimited } else { ErrorKind::Unavailable });
                error.retry_after = Some(Duration::from_secs(12));
                vec![Step::Fail(error), Step::Reply]
            }
        };
        let (provider, calls) = Scripted::boxed("primary", steps);
        let router = router(provider, config);
        router.chat_controlled(request(), None, Some(&user), &CancellationToken::new()).await.unwrap();
        if index < 2 {
            router.chat_controlled(request(), None, Some(&user), &CancellationToken::new()).await.unwrap();
        }
        assert_eq!(calls.lock().unwrap().len(), 2);
        let mut waits = vec![];
        while let Ok(event) = events.try_recv() {
            if event.data.contains("LLM waiting") { waits.push(event.data); }
        }
        assert!(!waits.is_empty(), "missing wait event for {cause}");
        assert!(waits.iter().all(|event| event.contains(cause)), "{waits:?}");
    }
}

#[tokio::test]
async fn usage_metrics_continuation_must_not_present_partial_sum_as_complete() {
    struct Continued { calls: std::sync::atomic::AtomicUsize, samples: Vec<Option<Usage>> }
    #[async_trait::async_trait]
    impl LLMProvider for Continued {
        fn name(&self) -> &str { "usage-fixture" }
        fn as_any(&self) -> &dyn std::any::Any { self }
        async fn chat(&self, _: ChatRequest) -> anyhow::Result<ChatResponse> { unreachable!() }
        async fn chat_attempt(&self, _: ChatRequest, _: Option<&ProviderContinuation>, _: Option<&(dyn Fn(StreamDelta)+Send+Sync)>) -> anyhow::Result<ChatAttempt> {
            let index = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let first = index + 1 < self.samples.len();
            let mut response = reply();
            response.usage = self.samples[index].clone();
            if first { response.content = None; response.finish_reason = Some("reasoning".into()); }
            Ok(ChatAttempt {response,continuation:first.then_some(ProviderContinuation::Codex {input:vec![]})})
        }
    }
    let known = Some(Usage {prompt_tokens:7,completion_tokens:5,total_tokens:12});
    let huge = Some(Usage {prompt_tokens:u32::MAX,completion_tokens:0,total_tokens:u32::MAX});
    for streamed in [false, true] {
        for (samples, expected) in [
            (vec![None, known.clone()], None),
            (vec![known.clone(), None], None),
            (vec![known.clone(), None, known.clone()], None),
            (vec![known.clone(), known.clone(), known.clone()], Some(36)),
            (vec![huge.clone(), known.clone()], None),
        ] {
            let r = LLMRouter::with_providers(vec![Box::new(Continued {calls:0.into(),samples})],
                "usage-fixture".into(),vec![],ResilienceConfig {max_attempts:3,..Default::default()});
            let response = if streamed { r.streaming_chat(request(), None, "usage-continuation-fixture").await }
                else { r.chat(request(), None).await }.unwrap();
            assert_eq!(response.usage.map(|u|u.total_tokens), expected,
                "only a complete, exact logical-call sum is available");
            crate::runtime::events::remove("usage-continuation-fixture");
        }
    }
}
