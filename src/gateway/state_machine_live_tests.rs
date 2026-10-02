//! Opt-in experiment fixture. Reuses the tool benchmark's real gateway,
//! provider observation and fail-closed executor guard; never routes by rubric.
use super::*;
use std::sync::{atomic::AtomicBool, Mutex};

pub(super) struct RoutingTrace {
    receiver: Mutex<tokio::sync::broadcast::Receiver<crate::dashboard::stream::StreamEvent>>,
    events: Mutex<Vec<serde_json::Value>>,
    complete: AtomicBool,
}
impl RoutingTrace {
    fn new(user: &str) -> Self {
        Self {
            receiver: Mutex::new(crate::dashboard::stream::get_or_create(user).subscribe()),
            events: Mutex::new(Vec::new()),
            complete: AtomicBool::new(true),
        }
    }
    pub(super) fn drain(&self, before_call: usize) -> anyhow::Result<()> {
        let mut receiver = self
            .receiver
            .lock()
            .map_err(|_| anyhow::anyhow!("Routing observer unavailable"))?;
        loop {
            match receiver.try_recv() {
                Ok(event) if event.event == "decision_route" => {
                    match serde_json::from_str::<serde_json::Value>(&event.data) {
                        Ok(route) => self
                            .events
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Routing observer unavailable"))?
                            .push(json!({"before_call":before_call,"route":route})),
                        Err(_) => self.complete.store(false, Ordering::SeqCst),
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                    self.complete.store(false, Ordering::SeqCst)
                }
                Err(_) => break,
            }
        }
        Ok(())
    }
    fn save(&self, out: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(
            out.join("decision-routes.json"),
            serde_json::to_vec_pretty(&json!({
                "complete":self.complete.load(Ordering::SeqCst),
                "events":*self.events.lock().map_err(|_| anyhow::anyhow!("Routing observer unavailable"))?,
            }))?,
        )?;
        Ok(())
    }
}

fn copy_directory(source: &std::path::Path, target: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "Experiment runtime must not contain symlinks"
        );
        if kind.is_dir() {
            copy_directory(&entry.path(), &target.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        } else {
            anyhow::bail!("Unsupported experiment runtime file");
        }
    }
    Ok(())
}

struct StateTaskModel<'a> {
    name: &'a str,
    provider: &'a str,
    thinking: &'a str,
}

async fn run_state_task(
    task: &serde_json::Value,
    arm: &str,
    out: &std::path::Path,
    model: StateTaskModel<'_>,
    decision_profile: Option<crate::gateway::decision_profiles::DecisionProfile>,
    inference_kind: &str,
    provider_factory: impl FnOnce(PathBuf) -> Box<dyn LLMProvider>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        ["fixed", "entry", "continuous", "decision"].contains(&arm),
        "Unknown experiment arm"
    );
    anyhow::ensure!(
        (arm == "decision") == decision_profile.is_some(),
        "Decision arm needs its own trusted profile"
    );
    std::fs::create_dir(out)?;
    let dir = tempfile::tempdir()?;
    let runtime = dir.path().join("runtime");
    for name in ["contexts", "templates"] {
        copy_directory(std::path::Path::new(name), &runtime.join(name))?;
    }
    if let Some(profile) = &decision_profile {
        profile.validate()?;
        crate::gateway::decision_profiles::save(
            &runtime.join("decisions"),
            "task-router",
            &serde_json::to_string(profile)?,
        )?;
    }
    let db = crate::db::Database::new(&dir.path().join("data"))?;
    crate::db::tools::init_default_tools(&db)?;
    let user = format!("state-live-{}", uuid::Uuid::new_v4());
    let fixture_file = dir.path().join("task.txt");
    if let Some(text) = task["fixture"].as_str() {
        std::fs::write(&fixture_file, text)?;
    }
    let prompt = task["prompt"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing prompt"))?
        .replace("{fixture}", &fixture_file.to_string_lossy());
    let mut ctx = db.load_context(&user)?;
    ctx.settings.provider = Some(model.provider.into());
    ctx.settings.model = Some(model.name.into());
    ctx.settings.sm_file = Some("20-tasks".into());
    ctx.settings.system_template = Some("20-tasks".into());
    ctx.active_state = Some(task["initial_state"].as_str().unwrap_or("standard").into());
    ctx.settings.active_state = ctx.active_state.clone();
    ctx.settings.thinking_mode = model.thinking.into();
    ctx.settings.show_thinking = true;
    ctx.settings.compaction_enabled = false;
    ctx.settings.max_llm_turns = Some(1); // real chat tool-followup pipeline
    ctx.settings.max_tool_calls = Some(8);
    ctx.settings.path = dir.path().to_string_lossy().into();
    ctx.settings.use_decision_router = arm == "decision";
    ctx.settings.decision_profile = decision_profile.as_ref().map(|_| "task-router".into());
    ctx.custom_data = json!({"state_eval_policy":arm});
    db.save_context(&ctx)?;
    std::fs::write(
        out.join("fixture.json"),
        serde_json::to_vec_pretty(&json!({
            "task":task,"arm":arm,"inference_kind":inference_kind,"initial_context":ctx,
            "message":prompt,"fixture_path":fixture_file,"decision_profile":decision_profile,
            "policy":"isolated DB; normal discovery; set_context only active_state to declared states; read-only fixture; no shell, network tools or file writes; rubric never in model context"
        }))?,
    )?;
    let trace = Arc::new(RoutingTrace::new(&user));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = ObservedProvider {
        inner: provider_factory(fixture_file.clone()),
        fixture_file,
        artifacts: out.into(),
        calls: calls.clone(),
        state_user: Some((db.clone(), user.clone())),
        routing_trace: Some(trace.clone()),
    };
    let mut config = crate::config::Config::from_env();
    config.root_dir = runtime.to_string_lossy().into();
    let state = GatewayState {
        db: db.clone(),
        config,
        secrets: Default::default(),
        llm: crate::gateway::LlmHandle::new(LLMRouter::with_providers(
            vec![Box::new(provider)],
            model.provider.into(),
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
    let start = Instant::now();
    let result = fixture_http_chat(state, &user, &prompt).await;
    trace.drain(calls.load(Ordering::SeqCst))?;
    trace.save(out)?;
    crate::dashboard::stream::remove(&user);
    std::fs::write(
        out.join("result.json"),
        serde_json::to_vec_pretty(&json!({
            "http_result":result.as_ref().ok(),"transport_error":result.as_ref().err().map(|e|e.to_string()),
            "elapsed_ms":start.elapsed().as_millis(),"history":db.get_messages(&user,200)?,
            "final_context":db.load_context(&user)?,
        }))?,
    )?;
    let result = result?;
    anyhow::ensure!(
        result["success"] == true,
        "Gateway task failed: {}",
        result["error"]
    );
    // Completion is not quality or correct state selection. The runner scores
    // those separately, including successful answers with no model transition.
    Ok(())
}

#[tokio::test]
#[ignore = "Paid live inference: one isolated task/arm of the 20-task experiment"]
async fn local_model_live_state_task() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var_os("GPU_ROUTER_URL").is_none(),
        "Unset GPU_ROUTER_URL; no wake/rental in tests"
    );
    let url = std::env::var("PRAXIS_LIVE_URL")?;
    let model = std::env::var("PRAXIS_LIVE_MODEL")?;
    let provider = std::env::var("PRAXIS_LIVE_PROVIDER").unwrap_or_else(|_| "llamacpp".into());
    anyhow::ensure!(
        ["llamacpp", "ollama"].contains(&provider.as_str()),
        "Unsupported live provider"
    );
    let thinking = std::env::var("PRAXIS_LIVE_THINKING")
        .unwrap_or_else(|_| if provider == "ollama" { "auto" } else { "low" }.into());
    let out = PathBuf::from(std::env::var("PRAXIS_LIVE_ARTIFACTS")?);
    let id = std::env::var("PRAXIS_STATE_CASE")?;
    let arm = std::env::var("PRAXIS_STATE_ARM")?;
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("tests/fixtures/20-tasks.json")?)?;
    let task = manifest
        .as_array()
        .and_then(|tasks| tasks.iter().find(|t| t["id"] == id))
        .ok_or_else(|| anyhow::anyhow!("Unknown state experiment case"))?;
    let profile = if arm == "decision" {
        let path = std::env::var("PRAXIS_DECISION_PROFILE")
            .unwrap_or_else(|_| "decisions/task-router.json".into());
        let profile: crate::gateway::decision_profiles::DecisionProfile =
            serde_json::from_str(&std::fs::read_to_string(path)?)?;
        profile.validate()?;
        Some(profile)
    } else {
        None
    };
    let solver_model = model.clone();
    let use_ollama = provider == "ollama";
    run_state_task(
        task,
        &arm,
        &out,
        StateTaskModel {
            name: &model,
            provider: &provider,
            thinking: &thinking,
        },
        profile,
        "live",
        move |_| {
            let key = std::env::var("PRAXIS_LIVE_API_KEY").ok();
            if use_ollama {
                Box::new(crate::gateway::llm::ollama::OllamaProvider::new(
                    url.trim_end_matches('/').into(),
                    solver_model,
                    key,
                ))
            } else {
                Box::new(LlamaCppProvider::new(key, solver_model, url))
            }
        },
    )
    .await
}

#[tokio::test]
#[ignore = "Requires real POML_CLI; scripted solver and local Decision HTTP fixture"]
async fn state_experiment_offline_router_uses_confirmed_file_evidence() -> anyhow::Result<()> {
    use wiremock::{matchers::method, Mock, MockServer, Request, Respond, ResponseTemplate};
    struct Classifier(bool);
    impl Respond for Classifier {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let context = body["contexts"][0].as_str().unwrap();
            assert!(!context.contains("acceptable_states") && !context.contains("checks"));
            let label = if context.contains("EVIDENCE_ERROR_MARKER") {
                "E"
            } else {
                "A"
            };
            ResponseTemplate::new(if self.0 {503} else {200}).set_body_json(json!({"results":[{
                "decision":{"category":label},"fields":{"category":{"value":label,"probability":0.99}}
            }]}))
        }
    }
    struct Solver {
        file: PathBuf,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl LLMProvider for Solver {
        fn name(&self) -> &str {
            "llamacpp"
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn chat(&self, _: ChatRequest) -> anyhow::Result<ChatResponse> {
            let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
            Ok(ChatResponse {
                reasoning_content: None,
                content: if first {
                    None
                } else {
                    Some("cause: use math.isclose; regression test".into())
                },
                tool_calls: if first {
                    Some(vec![crate::gateway::llm::provider::ToolCall {
                        id: "read-evidence".into(),
                        function: crate::gateway::llm::provider::FunctionCall {
                            name: "read_file".into(),
                            arguments: json!({"path":self.file}).to_string(),
                        },
                    }])
                } else {
                    None
                },
                finish_reason: Some(if first { "tool_calls" } else { "stop" }.into()),
                usage: None,
            })
        }
    }
    for failed in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(Classifier(failed))
            .expect(2)
            .mount(&server)
            .await;
        let mut profile = crate::gateway::decision_profiles::load(
            std::path::Path::new("decisions"),
            "task-router",
        )?;
        profile.endpoint = format!("{}/v1/decision", server.uri());
        let original = std::fs::read("decisions/task-router.json")?;
        let outer = tempfile::tempdir()?;
        let out = outer.path().join("run");
        let task = json!({"id":"synthetic","initial_state":"standard","acceptable_states":["debugger"],
            "prompt":"Lies {fixture} und erkläre die Ursache.","fixture":"EVIDENCE_ERROR_MARKER: a concrete failure.","checks":["cause","math.isclose"]});
        run_state_task(
            &task,
            "decision",
            &out,
            StateTaskModel {
                name: "scripted-solver",
                provider: "llamacpp",
                thinking: "low",
            },
            Some(profile),
            "scripted",
            |file| {
                Box::new(Solver {
                    file,
                    calls: AtomicUsize::new(0),
                })
            },
        )
        .await?;
        let read = |name: &str| -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::from_slice(&std::fs::read(out.join(name))?)?)
        };
        let first = read("call-00.request.json")?;
        let second = read("call-01.request.json")?;
        assert_eq!(first["observed_state"]["active_state"], "standard");
        assert_eq!(
            second["observed_state"]["active_state"],
            if failed { "standard" } else { "debugger" }
        );
        let trace = read("decision-routes.json")?;
        assert_eq!(trace["complete"], true);
        assert_eq!(trace["events"].as_array().unwrap().len(), 2);
        assert_eq!(trace["events"][1]["before_call"], 1);
        assert_eq!(
            trace["events"][1]["route"]["status"],
            if failed { "failed" } else { "applied" }
        );
        let result = read("result.json")?;
        assert!(result["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool"
                && m["tool_name"] == "read_file"
                && m["content"]
                    .as_str()
                    .is_some_and(|s| s.contains("EVIDENCE_ERROR_MARKER"))));
        assert_eq!(
            std::fs::read("decisions/task-router.json")?,
            original,
            "Experiment rewrote the source profile"
        );
        // Exercise the actual Python observer against runtime-produced artifacts,
        // including the real tool-output archive envelope rather than fake text.
        let scored=std::process::Command::new("python3")
            .args(["-c","import json,sys; sys.path.insert(0,'scripts'); from bench_state_machine import analyse_run; print(json.dumps(analyse_run(sys.argv[1])))"])
            .arg(&out).output()?;
        anyhow::ensure!(
            scored.status.success(),
            "Experiment scorer failed on real gateway artifacts"
        );
        let scored: serde_json::Value = serde_json::from_slice(&scored.stdout)?;
        assert_eq!(scored["inference_kind"], "scripted");
        assert_eq!(scored["evidence_read"], true);
        assert_eq!(scored["task_rubric_ok"], true);
        assert_eq!(scored["policy_followed"], true);
        assert_eq!(
            scored["decision_transitions"].as_array().unwrap().len(),
            if failed { 0 } else { 1 }
        );
        assert_eq!(scored["deterministic_transitions"], json!([]));
    }
    Ok(())
}

#[tokio::test]
#[ignore = "Requires real POML_CLI; Ollama chat and System One are local HTTP fixtures"]
async fn state_experiment_offline_ollama_systemone_preserves_probability_threshold(
) -> anyhow::Result<()> {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, Request, Respond, ResponseTemplate,
    };
    struct Classifier {
        probability: f64,
    }
    impl Respond for Classifier {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "nimble:latest");
            let state = body["state"].as_str().unwrap();
            assert!(!state.contains("acceptable_states") && !state.contains("checks"));
            let evidence = state.contains("EVIDENCE_ERROR_MARKER");
            let label = if evidence { "E" } else { "A" };
            let probability = if evidence { self.probability } else { 0.95 };
            let criteria = body["questions"]["category"]["criteria"]
                .as_object()
                .unwrap();
            let mut probabilities = serde_json::Map::new();
            for key in criteria.keys() {
                probabilities.insert(key.clone(), json!(0.0));
            }
            probabilities.insert(label.into(), json!(probability));
            probabilities.insert(
                if evidence { "A" } else { "E" }.into(),
                json!(1.0 - probability),
            );
            // Opposing confidence values prove that the probability threshold
            // uses the selected choice probability, never the confidence metric.
            ResponseTemplate::new(200).set_body_json(json!({"model":"nimble:latest",
                "answers":{"category":{"type":"choice","choice":label,"probabilities":probabilities,
                    "confidence":if probability<0.8 {0.99} else {0.01}}},
                "usage":{"input_tokens":25,"output_tokens":1}}))
        }
    }
    struct Solver;
    impl Respond for Solver {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "solver:latest");
            assert_eq!(body["stream"], true);
            assert!(
                body.get("think").is_none(),
                "Use model defaults for ordinary Ollama models"
            );
            let messages = body["messages"].as_array().unwrap();
            let evidence = messages.iter().any(|m| {
                m["role"] == "tool"
                    && m["content"]
                        .as_str()
                        .is_some_and(|s| s.contains("EVIDENCE_ERROR_MARKER"))
            });
            let message = if evidence {
                json!({"role":"assistant","content":"cause: use math.isclose; regression test"})
            } else {
                let prompt = messages.iter().rev().find(|m| m["role"] == "user").unwrap()
                    ["content"]
                    .as_str()
                    .unwrap();
                let file = prompt.strip_prefix("Read file: ").unwrap();
                json!({"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":file}}}]})
            };
            ResponseTemplate::new(200).insert_header("content-type","application/x-ndjson")
                .set_body_string(json!({"model":"solver:latest","message":message,"done":true,"done_reason":"stop",
                    "prompt_eval_count":50,"eval_count":10}).to_string()+"\n")
        }
    }
    for probability in [0.79, 0.8, 0.95] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(Classifier { probability })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(Solver)
            .expect(2)
            .mount(&server)
            .await;
        let mut profile = crate::gateway::decision_profiles::load(
            std::path::Path::new("decisions"),
            "task-router",
        )?;
        profile.backend = crate::gateway::decision_profiles::DecisionBackend::Ollama;
        profile.endpoint = format!("{}/v1/systemone", server.uri());
        profile.model = "nimble:latest".into();
        let outer = tempfile::tempdir()?;
        let out = outer.path().join("run");
        let task = json!({"id":"synthetic","initial_state":"standard","acceptable_states":["debugger"],
            "prompt":"Read file: {fixture}","fixture":"EVIDENCE_ERROR_MARKER: a concrete failure.","checks":["cause","math.isclose"]});
        let url = server.uri();
        run_state_task(
            &task,
            "decision",
            &out,
            StateTaskModel {
                name: "solver:latest",
                provider: "ollama",
                thinking: "auto",
            },
            Some(profile),
            "scripted",
            move |_| {
                Box::new(crate::gateway::llm::ollama::OllamaProvider::new(
                    url,
                    "solver:latest".into(),
                    None,
                ))
            },
        )
        .await?;
        let expected = if probability >= 0.8 {
            "debugger"
        } else {
            "standard"
        };
        let observed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("call-01.request.json"))?)?;
        assert_eq!(observed["observed_state"]["active_state"], expected);
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("decision-routes.json"))?)?;
        assert_eq!(trace["events"][1]["route"]["probability"], probability);
        assert_eq!(
            trace["events"][1]["route"]["status"],
            if probability >= 0.8 {
                "applied"
            } else {
                "low_probability"
            }
        );
        assert_eq!(trace["events"][1]["route"]["usage"]["input_tokens"], 25);
        let scored=std::process::Command::new("python3")
            .args(["-c","import json,sys; sys.path.insert(0,'scripts'); from bench_state_machine import analyse_run; print(json.dumps(analyse_run(sys.argv[1])))"])
            .arg(&out).output()?;
        anyhow::ensure!(scored.status.success(), "Ollama experiment scorer failed");
        let scored: serde_json::Value = serde_json::from_slice(&scored.stdout)?;
        assert_eq!(scored["evidence_read"], true);
        assert_eq!(scored["task_rubric_ok"], true);
        assert_eq!(scored["policy_followed"], true);
        assert_eq!(
            scored["decision_transitions"].as_array().unwrap().len(),
            usize::from(probability >= 0.8)
        );
        assert_eq!(scored["prompt_tokens"], 100);
        assert_eq!(scored["inference_kind"], "scripted");
    }
    Ok(())
}
