//! Provider attempts, history budgets and live speed share runtime counters.
use super::{
    llm::provider::{ChatRequest, ChatResponse, StreamDelta},
    GatewayState,
};
use crate::db::{contexts::Context, Database};
use serde_json::{json, Value};
use std::{sync::Mutex, time::Instant};

pub fn limits(db: &Database, ctx: &Context) -> anyhow::Result<Value> {
    let (messages, tokens) = db.get_messages_with_token_budget(&ctx.user_id, usize::MAX)?;
    let threshold = super::compaction::threshold(&ctx.settings);
    let older_turns = messages
        .iter()
        .rposition(|m| m.role == "user")
        .is_some_and(|i| i > 0);
    let claimed = super::task_control::compaction_claimed(&ctx.user_id);
    Ok(
        json!({"history_tokens_estimate":tokens,"history_limit":ctx.settings.history_token_limit.unwrap_or(crate::db::messages::DEFAULT_HISTORY_TOKENS),
        "compaction_enabled":ctx.settings.compaction_enabled,"compaction_threshold":threshold,
        "compaction_due":ctx.settings.compaction_enabled && tokens > threshold && older_turns && !claimed,
        "compaction_attempted_this_task":claimed,
        "older_complete_turns":older_turns,"model_context_limit":null,"estimate":true}),
    )
}

pub struct CallTrace {
    pub db: Database,
    pub ctx: Context,
    pub id: String,
    pub limits: Value,
}
impl CallTrace {
    pub fn attempt(
        &self,
        request: &ChatRequest,
        provider: &str,
        number: u64,
    ) -> anyhow::Result<i64> {
        let id = self.db.record_model_attempt(
            &self.ctx,
            request,
            provider,
            &self.id,
            number,
            &self.limits,
        )?;
        crate::runtime::events::send(&self.ctx.user_id, "generation", &json!({"request_id":id,"status":"started","provider":provider,
            "model":request.model,"attempt":number,"limits":self.limits,"output_limit":request.max_tokens,
            "request_tokens_estimate":super::llm::resilience::estimated_tokens(request)}).to_string());
        Ok(id)
    }
    pub fn finish(
        &self,
        id: i64,
        status: &str,
        response: Option<&ChatResponse>,
        elapsed_ms: u64,
        first_token_ms: Option<u64>,
    ) -> anyhow::Result<()> {
        self.db.finish_model_attempt(
            id,
            status,
            response.and_then(|r| r.usage.as_ref()),
            elapsed_ms,
            first_token_ms,
            response.and_then(|r| r.finish_reason.as_deref()),
        )?;
        crate::runtime::events::send(&self.ctx.user_id, "generation", &json!({"request_id":id,"status":status,"elapsed_ms":elapsed_ms,
            "first_token_ms":first_token_ms,"usage":response.and_then(|r| r.usage.as_ref()),
            "tokens_per_sec":response.and_then(|r| r.usage.as_ref()).filter(|_| elapsed_ms > 0).map(|u|f64::from(u.completion_tokens)*1000.0/elapsed_ms as f64),"estimated":false}).to_string());
        Ok(())
    }
}

#[derive(Default)]
struct Counts {
    characters: usize,
    first_token_ms: Option<u64>,
    last_emit_ms: u64,
}
pub struct Generation {
    start: Instant,
    counts: Mutex<Counts>,
}
impl Generation {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            counts: Mutex::new(Counts::default()),
        }
    }
    pub fn first_token_ms(&self) -> Option<u64> {
        self.counts.lock().ok().and_then(|c| c.first_token_ms)
    }
    pub fn delta(&self, user: Option<&str>, request_id: Option<i64>, delta: &StreamDelta) {
        let count = match delta {
            StreamDelta::Text { text } => text.chars().count(),
            StreamDelta::ToolCall { arguments, .. } => {
                arguments.as_ref().map_or(0, |a| a.chars().count())
            }
            _ => 0,
        };
        if count == 0 {
            return;
        }
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        counts.characters = counts.characters.saturating_add(count);
        let first = counts.first_token_ms.is_none();
        counts.first_token_ms.get_or_insert(elapsed_ms);
        if !first && elapsed_ms.saturating_sub(counts.last_emit_ms) < 250 {
            return;
        }
        counts.last_emit_ms = elapsed_ms;
        let estimated = counts.characters.div_ceil(4);
        if let Some(user) = user {
            crate::runtime::events::send(user, "generation", &json!({"request_id":request_id,"status":"generating",
            "elapsed_ms":elapsed_ms,"first_token_ms":counts.first_token_ms,"output_tokens_estimate":estimated,
            "tokens_per_sec":if elapsed_ms > 0 {estimated as f64 * 1000.0 / elapsed_ms as f64} else {0.0},"estimated":true}).to_string());
        }
    }
}

pub async fn chat(
    state: &GatewayState,
    ctx: &Context,
    request: ChatRequest,
    provider: Option<&str>,
) -> anyhow::Result<ChatResponse> {
    let trace = CallTrace {
        db: state.db.clone(),
        ctx: ctx.clone(),
        id: uuid::Uuid::new_v4().to_string(),
        limits: limits(&state.db, ctx)?,
    };
    state
        .llm
        .get()
        .streaming_chat_traced(request, provider, &ctx.user_id, &trace)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_telemetry_uses_real_compaction_threshold_and_requires_older_turns() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let mut ctx = db.load_context("telemetry").unwrap();
        ctx.settings.compaction_enabled = true;
        ctx.settings.compaction_token_limit = Some(1000);
        ctx.settings.history_token_limit = Some(100);
        db.save_context(&ctx).unwrap();
        db.add_message(
            "telemetry",
            &crate::db::messages::Message::user("x".repeat(300)),
        )
        .unwrap();
        let l = limits(&db, &ctx).unwrap();
        assert_eq!(l["compaction_threshold"], 50);
        assert_eq!(l["compaction_due"], false);
        db.add_message(
            "telemetry",
            &crate::db::messages::Message::assistant("done".into()),
        )
        .unwrap();
        db.add_message(
            "telemetry",
            &crate::db::messages::Message::user("next".into()),
        )
        .unwrap();
        assert_eq!(limits(&db, &ctx).unwrap()["compaction_due"], true);
        ctx.settings.compaction_enabled = false;
        assert_eq!(limits(&db, &ctx).unwrap()["compaction_due"], false);
    }

    struct TracedProvider;
    #[async_trait::async_trait]
    impl super::super::llm::provider::LLMProvider for TracedProvider {
        async fn chat(&self, _: ChatRequest) -> anyhow::Result<ChatResponse> {
            unreachable!("trace fixture uses streaming")
        }
        async fn chat_stream(
            &self,
            request: ChatRequest,
            delta: &(dyn Fn(String) + Send + Sync),
        ) -> anyhow::Result<ChatResponse> {
            assert_eq!(
                request.messages[0].content.as_deref(),
                Some("Actual system instructions")
            );
            delta("hello".into());
            Ok(ChatResponse {
                content: Some("hello".into()),
                reasoning_content: None,
                tool_calls: None,
                finish_reason: Some("stop".into()),
                usage: Some(super::super::llm::provider::Usage {
                    prompt_tokens: 20,
                    completion_tokens: 2,
                    total_tokens: 22,
                }),
            })
        }
        fn name(&self) -> &str {
            "traced-fixture"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn execution_trace_captures_provider_attempt_prompt_and_reported_usage() {
        use super::super::llm::{provider::ChatMessage, resilience::ResilienceConfig, LLMRouter};
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let ctx = db.load_context("trace-fixture").unwrap();
        db.save_context(&ctx).unwrap();
        let trace = CallTrace {
            db: db.clone(),
            ctx: ctx.clone(),
            id: "logical-call".into(),
            limits: limits(&db, &ctx).unwrap(),
        };
        let router = LLMRouter::with_providers(
            vec![Box::new(TracedProvider)],
            "traced-fixture".into(),
            vec![],
            ResilienceConfig::default(),
        );
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: "system".into(),
                content: Some("Actual system instructions".into()),
                reasoning_content: None,
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            }],
            tools: None,
            temperature: None,
            max_tokens: Some(100),
            model: Some("fixture-model".into()),
            vision_provider: None,
            vision_model: None,
            thinking: None,
        };
        let response = router
            .streaming_chat_traced(request, None, &ctx.user_id, &trace)
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("hello"));
        let events = db.execution_events(&ctx.user_id, 100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["system_prompts"][0], "Actual system instructions");
        assert_eq!(events[0]["payload"]["usage"]["completion_tokens"], 2);
        assert_eq!(events[0]["payload"]["provider"], "traced-fixture");
        assert_eq!(events[0]["payload"]["output_limit"], 100);
        assert_eq!(events[0]["payload"]["status"], "completed");
        assert!(events[0]["payload"]["first_token_ms"].is_number());
        assert!(db.get_messages(&ctx.user_id, 100).unwrap().is_empty());
    }

    struct CutoffProvider(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl super::super::llm::provider::LLMProvider for CutoffProvider {
        async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
            let first = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
            assert_eq!(request.max_tokens, Some(if first { 16 } else { 32 }));
            Ok(ChatResponse {
                content: (!first).then(|| "Finished".into()),
                reasoning_content: None,
                tool_calls: None,
                finish_reason: Some(if first { "length" } else { "stop" }.into()),
                usage: Some(super::super::llm::provider::Usage {
                    prompt_tokens: 20,
                    completion_tokens: if first { 16 } else { 2 },
                    total_tokens: if first { 36 } else { 22 },
                }),
            })
        }
        fn name(&self) -> &str {
            "cutoff-fixture"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn execution_trace_records_retries_actual_allowances_and_failed_usage() {
        use super::super::llm::{resilience::ResilienceConfig, LLMRouter};
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let ctx = db.load_context("trace-retry-fixture").unwrap();
        db.save_context(&ctx).unwrap();
        let trace = CallTrace {
            db: db.clone(),
            ctx: ctx.clone(),
            id: "retry-call".into(),
            limits: limits(&db, &ctx).unwrap(),
        };
        let router = LLMRouter::with_providers(
            vec![Box::new(CutoffProvider(Default::default()))],
            "cutoff-fixture".into(),
            vec![],
            ResilienceConfig::default(),
        );
        router
            .streaming_chat_traced(
                ChatRequest {
                    messages: vec![],
                    tools: None,
                    temperature: None,
                    max_tokens: Some(16),
                    model: None,
                    vision_provider: None,
                    vision_model: None,
                    thinking: None,
                },
                None,
                &ctx.user_id,
                &trace,
            )
            .await
            .unwrap();
        let events = db.execution_events(&ctx.user_id, 100).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0]["payload"]["logical_call"],
            events[1]["payload"]["logical_call"]
        );
        assert_eq!(events[0]["payload"]["status"], "failed");
        assert_eq!(events[0]["payload"]["output_limit"], 16);
        assert_eq!(events[0]["payload"]["usage"]["completion_tokens"], 16);
        assert_eq!(events[0]["payload"]["finish_reason"], "length");
        assert_eq!(events[1]["payload"]["status"], "completed");
        assert_eq!(events[1]["payload"]["attempt"], 2);
        assert_eq!(events[1]["payload"]["output_limit"], 32);
        assert_eq!(events[1]["payload"]["usage"]["completion_tokens"], 2);
    }
}
