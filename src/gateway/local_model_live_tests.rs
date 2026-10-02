//! Opt-in live-model regression through the actual HTTP gateway and tool executor.
//! Uses an isolated DB, real POML, full built-in discovery schemas, no plugins,
//! and a fail-closed guard on model-selected tools. Never loads production secrets.
use super::*;
use crate::gateway::llm::{
    llamacpp::LlamaCppProvider,
    provider::{ChatResponse, LLMProvider},
    resilience::ResilienceConfig,
    LLMRouter,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

struct ObservedProvider {
    inner: Box<dyn LLMProvider>,
    fixture_file: PathBuf,
    artifacts: PathBuf,
    calls: Arc<AtomicUsize>,
    state_user: Option<(crate::db::Database, String)>,
    routing_trace: Option<Arc<state_machine_live_tests::RoutingTrace>>,
}

#[async_trait::async_trait]
impl LLMProvider for ObservedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.observe(request, None).await
    }
    async fn chat_stream_events(&self, request: ChatRequest,
        on_delta: &(dyn Fn(crate::gateway::llm::provider::StreamDelta) + Send + Sync),
    ) -> anyhow::Result<ChatResponse> {
        self.observe(request, Some(on_delta)).await
    }
}
impl ObservedProvider {
    async fn observe(&self, request: ChatRequest,
        on_delta: Option<&(dyn Fn(crate::gateway::llm::provider::StreamDelta) + Send + Sync)>,
    ) -> anyhow::Result<ChatResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(trace) = &self.routing_trace {
            trace.drain(n)?;
        }
        let prefix = self.artifacts.join(format!("call-{n:02}"));
        std::fs::write(
            prefix.with_extension("request.json"),
            serde_json::to_vec_pretty(&json!({
                "request": request, "thinking": request.thinking,
                "observed_state": self.state_user.as_ref().map(|(db,user)| db.load_context(user)
                    .map(|ctx| json!({"active_state":ctx.active_state,"role":ctx.sm_data.get("role"),"workflow":crate::gateway::prompt::workflow_name(&ctx)}))).transpose()?,
            }))?,
        )?;
        let start = Instant::now();
        let deltas = std::sync::Mutex::new(Vec::new());
        let result = if let Some(on_delta) = on_delta {
            self.inner.chat_stream_events(request, &|delta| {
                deltas.lock().unwrap().push(json!({"elapsed_ms":start.elapsed().as_millis(),"delta":delta}));
                on_delta(delta);
            }).await
        } else { self.inner.chat(request).await };
        std::fs::write(prefix.with_extension("deltas.json"), serde_json::to_vec_pretty(&*deltas.lock().unwrap())?)?;
        std::fs::write(
            prefix.with_extension("response.json"),
            serde_json::to_vec_pretty(&json!({
                "elapsed_ms": start.elapsed().as_millis(),
                "response": result.as_ref().ok(),
                "error": result.as_ref().err().map(|e| format!("{e:#}")),
            }))?,
        )?;
        let response = result?;
        // Keep normal schemas and real model responses. Abort (don't fabricate a
        // tool result) before ANY calls in an unsafe batch reach the executor.
        for call in response.tool_calls.iter().flatten() {
            let args: serde_json::Value = serde_json::from_str(&call.function.arguments)?;
            let safe = match call.function.name.as_str() {
                "search_tools"
                | "get_context"
                | "read_tool_result"
                | "memory_profile_list"
                | "memory_profile_create"
                | "memory_profile_load"
                | "memory_get"
                | "memory_set"
                | "agent_complete"
                | "agent_feedback" => true,
                // This one key cannot escape the synthetic session. Let the
                // real typed/declaration validator reject bad values so the
                // model can recover from its actual error receipt. Do not turn
                // a harmless argument error into a safety-guard abort.
                "set_context" if self.state_user.is_some() => args["key"] == "active_state",
                // An exact, predefined read-only command over our own fixture;
                // no arbitrary generated shell, paths, substitutions or redirects.
                "execute_terminal" if self.state_user.is_none() => args["command"].as_str().is_some_and(|command| {
                    command == format!("grep -n \"Prüfcode:\" {}", self.fixture_file.display())
                }),
                "read_file" => args["path"]
                    .as_str()
                    .is_some_and(|p| std::path::Path::new(p) == self.fixture_file),
                _ => false,
            };
            if !safe {
                std::fs::write(prefix.with_extension("guard.json"),serde_json::to_vec_pretty(&json!({"blocked_tool":call.function.name,"arguments":args,"reason":"outside audited fixture policy"}))?)?;
                anyhow::bail!("Live-test guard blocked tool: {}", call.function.name);
            }
        }
        eprintln!(
            "live call {n}: {:?}, tools={:?}, usage={:?}, elapsed={:?}",
            response.finish_reason,
            response
                .tool_calls
                .as_ref()
                .map(|c| c.iter().map(|t| &t.function.name).collect::<Vec<_>>()),
            response.usage,
            start.elapsed()
        );
        Ok(response)
    }
}

#[tokio::test]
#[ignore = "Paid live inference: requires PRAXIS_LIVE_URL, PRAXIS_LIVE_MODEL, POML_CLI; no production DB"]
async fn local_model_live_tool_calling() -> anyhow::Result<()> {
    // Explicit opt-in only. Do not wake/rent instances or consult a local .env.
    anyhow::ensure!(
        std::env::var_os("GPU_ROUTER_URL").is_none(),
        "Unset GPU_ROUTER_URL: this test must not wake/rent instances"
    );
    let url = std::env::var("PRAXIS_LIVE_URL")?;
    let model = std::env::var("PRAXIS_LIVE_MODEL")?;
    let out = PathBuf::from(std::env::var("PRAXIS_LIVE_ARTIFACTS")?);
    // Refuse to overwrite an unfixed baseline or another test run.
    std::fs::create_dir(&out)?;
    let thinking = std::env::var("PRAXIS_LIVE_THINKING").unwrap_or_else(|_| "off".into());
    anyhow::ensure!(
        ["off", "low"].contains(&thinking.as_str()),
        "thinking must be off or low"
    );
    let case = std::env::var("PRAXIS_LIVE_CASE").unwrap_or_else(|_| "read_file".into());
    anyhow::ensure!(
        ["read_file", "read_tail", "discovery"].contains(&case.as_str()),
        "unknown test case"
    );
    let dir = tempfile::tempdir()?;
    let db = crate::db::Database::new(dir.path())?;
    crate::db::tools::init_default_tools(&db)?;
    let user = format!("live-test-{}", uuid::Uuid::new_v4());
    let marker = format!("fixture_{}", uuid::Uuid::new_v4().simple());
    let fixture_file = dir.path().join("fixture.txt");
    let lines: usize = std::env::var("PRAXIS_LIVE_FILE_LINES")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    anyhow::ensure!(lines <= 10_000, "fixture limited to 10,000 lines");
    let mut file_content = String::new();
    for line in 0..lines {
        file_content.push_str(&format!(
            "Zeile {line:05}: synthetische Testdaten ohne Prüfcode.\n"
        ));
    }
    file_content.push_str(&format!("Prüfcode: {marker}\n"));
    std::fs::write(&fixture_file, &file_content)?;
    let mut ctx = db.load_context(&user)?;
    ctx.settings.provider = Some("llamacpp".into());
    ctx.settings.model = Some(model.clone());
    ctx.settings.thinking_mode = thinking;
    ctx.settings.show_thinking = true;
    let turns: i32 = std::env::var("PRAXIS_LIVE_TURNS")
        .unwrap_or_else(|_| "1".into())
        .parse()?;
    anyhow::ensure!((1..=6).contains(&turns), "turns must be 1..6");
    ctx.settings.max_llm_turns = Some(turns);
    ctx.settings.max_tool_calls = Some(5);
    let compact = std::env::var("PRAXIS_LIVE_COMPACT").as_deref() == Ok("1");
    if compact {
        ctx.settings.compaction_enabled = true;
        ctx.settings.compaction_token_limit = Some(64);
        ctx.settings.compaction_summary = "Earlier decision: use SQLite; no cloud dependency.".into();
    }
    db.save_context(&ctx)?;
    if compact {
        db.add_message(&user, &crate::db::messages::Message::user("Big idea PROJECT_KITE_2026: a reliable offline assistant. Key constraint: never repeat side effects. Next action: verify tool receipts. Budget is limited; do not buy extra services.".into()))?;
        db.add_message(&user, &crate::db::messages::Message::assistant("Verified: durable tool receipts are stored. Open blocker: context overflow after large outputs; this is not proven to be a Jinja defect.".into()))?;
    }
    crate::db::memory_profiles::create_profile(&db, &user, &marker)?;
    let provider = ObservedProvider {
        inner: Box::new(LlamaCppProvider::new(std::env::var("PRAXIS_LIVE_API_KEY").ok(), model, url)),
        fixture_file: fixture_file.clone(),
        artifacts: out.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
        state_user: None,
        routing_trace: None,
    };
    let state = GatewayState {
        db: db.clone(),
        config: crate::config::Config::from_env(),
        secrets: Default::default(),
        llm: crate::gateway::LlmHandle::new(LLMRouter::with_providers(
            vec![Box::new(provider)],
            "llamacpp".into(),
            vec![],
            ResilienceConfig {
                max_attempts: 1,
                ..Default::default()
            },
        )),
        plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        event_tx: tokio::sync::broadcast::channel(16).0,
        start_time: Instant::now(),
    };
    let prompt = if case == "read_file" {
        format!(
            "Lies die Datei {} und nenne den darin gespeicherten Prüfcode exakt.",
            fixture_file.display()
        )
    } else if case == "read_tail" {
        format!("Lies nur die letzte Zeile der großen Datei {} und nenne den dort gespeicherten Prüfcode exakt. Nutze die Ausgabeselektion des Lesewerkzeugs, keine Shell.", fixture_file.display())
    } else {
        "Welche Memory-Profile existieren für mich? Prüfe das mit den verfügbaren Werkzeugen und nenne die exakten Profilnamen.".into()
    };
    std::fs::write(
        out.join("fixture.json"),
        serde_json::to_vec_pretty(&json!({
            "context": ctx, "message": prompt, "expected_marker": marker,
            "file_content": file_content,
            "policy": "full built-in schemas; fail-closed tool guard; one attempt, no fallback; default output budget",
        }))?,
    )?;
    let start = Instant::now();
    let result = fixture_http_chat(state, &user, &prompt).await?;
    let history = db.get_messages(&user, 100)?;
    std::fs::write(
        out.join("result.json"),
        serde_json::to_vec_pretty(&json!({
            "http_result": result, "elapsed_ms": start.elapsed().as_millis(),
            "history": history, "final_context": db.load_context(&user)?,
        }))?,
    )?;
    check_tool_fixture(&db, &user, &history, &result, compact, &case, &marker)?;
    eprintln!("PASS: {case}, elapsed={:?}; artifacts={}",start.elapsed(),out.display());
    Ok(())
}

async fn fixture_http_chat(state: GatewayState, user: &str, prompt: &str) -> anyhow::Result<serde_json::Value> {
    // Real HTTP handler, no public socket and no changes to the deployed gateway.
    let app = axum::Router::new()
        .route(
            "/v1/chat",
            axum::routing::post(crate::gateway::http_handler::chat_handler),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let result = async {
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(360)).build()?
            .post(format!("http://{addr}/v1/chat"))
            .json(&json!({"user_id":user,"message":prompt}))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await
    }
    .await;
    server.abort();
    Ok(result?)
}

fn check_tool_fixture(db:&crate::db::Database, user:&str, history:&[crate::db::messages::Message], result:&serde_json::Value, compact:bool, case:&str, marker:&str) -> anyhow::Result<()> {
    anyhow::ensure!(
        result["success"] == true,
        "Gateway failed: {}",
        result["error"]
    );
    if compact {
        let saved = db.load_context(&user)?;
        anyhow::ensure!(saved.settings.compaction_summary.contains("PROJECT_KITE_2026") && saved.settings.compaction_summary.contains("SQLite"), "Compaction lost big idea or prior decision");
        anyhow::ensure!(!history.iter().any(|m|m.content.starts_with("Big idea PROJECT_KITE_2026")), "Compaction did not remove summarized prefix");
    }
    let tool_names: Vec<_> = history
        .iter()
        .filter_map(|m| m.tool_name.as_deref())
        .collect();
    let expected_tool = if case != "discovery" {
        "read_file"
    } else {
        "memory_profile_list"
    };
    anyhow::ensure!(
        tool_names.contains(&expected_tool),
        "No real {expected_tool} execution: {tool_names:?}"
    );
    if case == "discovery" {
        anyhow::ensure!(
            tool_names.contains(&"search_tools"),
            "No discovery execution"
        );
    }
    anyhow::ensure!(
        result["response"]
            .as_str()
            .is_some_and(|s| s.contains(marker)),
        "Final answer omitted fixture marker"
    );
    for call in history
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
    {
        anyhow::ensure!(
            history
                .iter()
                .filter(|m| m.tool_call_id.as_deref() == Some(&call.id))
                .count()
                == 1,
            "Missing or duplicate receipt for {}",
            call.id
        );
    }
    Ok(())
}

#[path = "state_machine_live_tests.rs"]
mod state_machine_live_tests;
