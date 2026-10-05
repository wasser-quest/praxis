//! Side-effect regression tests. All commands write only inside a TempDir.
use super::{
    error::{ErrorKind, ProviderError},
    provider::*,
    resilience::ResilienceConfig,
    LLMRouter,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct ToolProvider {
    calls: Arc<AtomicUsize>,
    command: String,
    fail_forever: bool,
}
#[async_trait::async_trait]
impl LLMProvider for ToolProvider {
    fn name(&self) -> &str {
        "synthetic"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        assert_eq!(request.max_tokens, Some(12_345), "configured output allowance lost");
        let turn = self.calls.fetch_add(1, Ordering::SeqCst);
        if turn > 0 {
            assert_eq!(
                request.messages.iter().filter(|m| m.role == "tool").count(),
                1,
                "tool result must survive every retry"
            );
            if turn == 1 || self.fail_forever {
                return Err(ProviderError::new(ErrorKind::Unavailable).into());
            }
        }
        Ok(ChatResponse {
    reasoning_content: None,
            content: Some(
                if turn == 0 {
                    "working"
                } else {
                    "done [[AGENT:COMPLETE]]"
                }
                .into(),
            ),
            tool_calls: (turn == 0).then(|| {
                vec![ToolCall {
                    id: "call-once".into(),
                    function: FunctionCall {
                        name: "execute_terminal".into(),
                        arguments: serde_json::json!({"command":self.command}).to_string(),
                    },
                }]
            }),
            finish_reason: None,
            usage: None,
        })
    }
}
fn fixture(
    fail_forever: bool,
) -> (
    tempfile::TempDir,
    crate::db::Database,
    LLMRouter,
    Arc<AtomicUsize>,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let p = ToolProvider {
        calls: calls.clone(),
        command: format!("printf x >> '{}'", dir.path().join("effects").display()),
        fail_forever,
    };
    let policy = ResilienceConfig {
        max_attempts: 2,
        max_output_tokens: 12_345,
        initial_backoff_ms: 1,
        max_backoff_ms: 1,
        ..Default::default()
    };
    let router = LLMRouter::with_providers(vec![Box::new(p)], "synthetic".into(), vec![], policy);
    (dir, db, router, calls)
}

#[tokio::test]
#[cfg(all(unix, feature = "shell"))]
async fn resilience_public_tool_loop_retries_only_llm_not_executed_command() {
    let (dir, db, router, calls) = fixture(false);
    let tools = crate::db::tools::to_tool_definitions(&db).unwrap();
    let result = router
        .chat_with_tools(
            &db,
            "resilience-public-tools",
            vec![],
            tools,
            Some(3),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effects")).unwrap(),
        "x"
    );
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "Requires Node and POML_CLI (only the renderer is real; LLM/tools use synthetic data)"]
async fn resilience_gateway_histories_survive_retries_and_exhaustion_on_both_paths() {
    assert!(std::env::var("POML_CLI").is_ok());
    for max_turns in [1, 4] {
        for exhausted in [false, true] {
            let (dir, db, router, calls) = fixture(exhausted);
            // Test retry/history semantics with an isolated trusted workflow.
            // Production role permissions must not implicitly authorize this
            // synthetic provider's raw terminal action.
            std::fs::create_dir(dir.path().join("contexts")).unwrap();
            std::fs::create_dir(dir.path().join("templates")).unwrap();
            std::fs::write(dir.path().join("contexts/retries.sm"), "[state standard]\nsettings.provider = synthetic\nsettings.system_template = standard\nsettings.activated_tools = [\"execute_terminal\",\"agent_complete\"]\n").unwrap();
            std::fs::write(dir.path().join("templates/standard.poml"), "<poml><role>Offline retry fixture</role></poml>").unwrap();
            std::fs::write(dir.path().join("templates/user.poml"), include_str!("../../../templates/user.poml")).unwrap();
            let user = format!("resilience-gateway-{max_turns}-{exhausted}");
            let mut ctx = db.load_context(&user).unwrap();
            ctx.sm_file = Some("retries".into());
            ctx.settings.sm_file = ctx.sm_file.clone();
            ctx.settings.system_template = Some("standard".into());
            ctx.settings.provider = Some("synthetic".into());
            ctx.settings.max_llm_turns = Some(max_turns);
            ctx.settings.history_with_toolcalls = true;
            ctx.settings.use_tts = false;
            db.save_context(&ctx).unwrap();
            let state = crate::gateway::GatewayState {
                db: db.clone(),
                config: crate::config::Config {
                    root_dir: dir.path().to_string_lossy().into_owned(),
                    workspace_dir: None,
                    ..crate::config::Config::from_env()
                },
                secrets: Default::default(),
                llm: crate::gateway::LlmHandle::new(router),
                plugins: Arc::new(crate::plugins::PluginRegistry::new()),
                event_tx: tokio::sync::broadcast::channel(16).0,
                start_time: std::time::Instant::now(),
            };
            let result = crate::gateway::message_handler::handle_message(
                &state,
                &user,
                "synthetic offline test",
                Some("web"),
            )
            .await;
            assert_eq!(result.is_err(), exhausted, "{result:?}");
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("effects")).unwrap(),
                "x"
            );
            let history = db.get_messages(&user, 100).unwrap();
            assert_eq!(history.iter().filter(|m| m.role == "tool").count(), 1);
            assert_eq!(history.iter().filter(|m| m.tool_calls.is_some()).count(), 1);
            assert!(crate::gateway::task_control::cancellation(&user).is_none());
            assert!(crate::gateway::agent_loop::get_user_input_sender(&user)
                .await
                .is_none());
        }
    }
}
