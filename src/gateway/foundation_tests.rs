//! Registry and management regressions, without paid inference or feature hosts.
use super::*;
use crate::plugins::{Plugin, PluginRegistry};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn plugin(name: &str) -> Plugin {
    serde_json::from_value(json!({
        "name": name, "version":"1", "description":"fixture", "context":{"b":2,"a":1},
        "tools":[{"name":format!("{name}_check"),"description":"verify", "parameters":{
            "type":"object","properties":{},"additionalProperties":false
        },"handler":{"type":"verification"}, "contract":{
            "effect":"verification","idempotency":"idempotent","timeout_secs":5,
            "postconditions":[{"program":"/bin/true"}]
        }}]
    }))
    .unwrap()
}

#[test]
fn foundation_registry_revision_is_stable_and_covers_effect_authority() {
    let mut a = PluginRegistry::new();
    a.try_register(plugin("alpha")).unwrap();
    a.try_register(plugin("beta")).unwrap();
    let mut b = PluginRegistry::new();
    b.try_register(plugin("beta")).unwrap();
    let mut alpha = plugin("alpha");
    alpha.context = [("a".into(), json!(1)), ("b".into(), json!(2))]
        .into_iter()
        .collect();
    b.try_register(alpha).unwrap();
    assert_eq!(a.revision().unwrap(), b.revision().unwrap());
    assert_eq!(a.revision().unwrap().len(), 64);
    assert_eq!(a.context_defaults(), b.context_defaults());
    for kind in [
        "version", "schema", "handler", "contract", "grants", "defaults", "enabled",
    ] {
        let mut changed = plugin("alpha");
        match kind {
            "version" => changed.version = "2".into(),
            "schema" => changed.tools[0].parameters["description"] = json!("new schema"),
            "handler" => {
                changed.tools[0].handler = crate::plugins::PluginHandler::Http {
                    url: "http://example.invalid/changed".into(),
                    method: "POST".into(),
                }
            }
            "contract" => changed.tools[0].contract.as_mut().unwrap().timeout_secs = 6,
            "grants" => changed.secrets.push("fixture_secret".into()),
            "defaults" => {
                changed.context.insert("a".into(), json!(99));
            }
            "enabled" => changed.enabled = false,
            _ => unreachable!(),
        }
        let mut registry = PluginRegistry::new();
        registry.register(changed);
        registry.register(plugin("beta"));
        assert_ne!(
            registry.revision().unwrap(),
            a.revision().unwrap(),
            "{kind}"
        );
    }
}

#[test]
fn foundation_task_keeps_an_owned_registry_snapshot_until_teardown() {
    let user = "foundation-registry-pin";
    let task = task_control::begin(user).unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin("alpha")).unwrap();
    task_control::pin_registry(user, &registry).unwrap();
    let original = registry.revision().unwrap();
    let mut changed = plugin("alpha");
    changed.version = "2".into();
    registry.register(changed);
    let error = task_control::pin_registry(user, &registry).unwrap_err();
    assert!(error.to_string().contains("registry revision"), "{error}");
    let snapshot = task_control::registry_snapshot(user).unwrap();
    assert_eq!(snapshot.revision().unwrap(), original);
    assert_eq!(snapshot.get("alpha").unwrap().version, "1");
    drop(task);
    assert!(task_control::registry_snapshot(user).is_none());
    let _next = task_control::begin(user).unwrap();
    task_control::pin_registry(user, &registry).unwrap();
    assert_eq!(
        task_control::registry_revision(user).unwrap(),
        registry.revision().unwrap()
    );
}

fn fixture() -> (tempfile::TempDir, GatewayState, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("contexts")).unwrap();
    std::fs::create_dir(root.path().join("templates")).unwrap();
    std::fs::write(
        root.path().join("contexts/standard.sm"),
        "[state standard]\nsettings.system_template = standard\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("templates/standard.poml"),
        "<poml><role>Fixture</role></poml>",
    )
    .unwrap();
    let db = crate::db::Database::new(&root.path().join("data")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let user = format!("foundation-{}", uuid::Uuid::new_v4());
    let mut config = crate::config::Config::from_env();
    config.root_dir = root.path().to_string_lossy().into_owned();
    config.workspace_dir = None;
    config.gateway_api_key = "foundation-api-key-12345".into();
    config.dashboard_admin_password = "foundation-password".into();
    config.use_provider = "openai".into();
    config.llm_fallback_providers.clear();
    let state = management_state(db, config, Default::default(), PluginRegistry::new()).unwrap();
    (root, state, user)
}

#[cfg(unix)]
#[tokio::test]
async fn foundation_pinned_dispatch_rechecks_live_disable_and_records_revision() {
    let (root, mut state, user) = fixture();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin("alpha")).unwrap();
    let revision = registry.revision().unwrap();
    state.plugins = Arc::new(registry);
    let _task = task_control::begin(&user).unwrap();
    task_control::pin_registry(&user, &state.plugins).unwrap();
    let sm = crate::sm::parse("[state standard]\nsettings.activated_tools = [\"execute_decision\",\"alpha_check\"]\n[decision_ir]\nV = alpha/alpha_check\n[action_guards]\n_complete = [alpha/alpha_check]").unwrap();
    action_contracts::bind(&user, "fixture", &sm, root.path()).unwrap();
    let mut ctx = state.db.load_context(&user).unwrap();
    ctx.settings.activated_tools = vec!["execute_decision".into(), "alpha_check".into()];
    state.db.save_context(&ctx).unwrap();
    let call = llm::provider::ToolCall {
        id: "first".into(),
        function: llm::provider::FunctionCall {
            name: "alpha_check".into(),
            arguments: "{}".into(),
        },
    };
    crate::db::tools::set_plugin_tool_enabled(&state.db, "alpha_check", false).unwrap();
    let dispatch = tool_dispatch::DispatchContext::new(
        root.path(),
        &state.db,
        &user,
        &state.plugins,
        tool_dispatch::DispatchMode::Chat,
    );
    let disabled = dispatch.execute(&call).await;
    assert!(disabled.contains("disabled"), "{disabled}");
    assert!(action_contracts::require(&user, "_complete").is_err());
    crate::db::tools::set_plugin_tool_enabled(&state.db, "alpha_check", true).unwrap();
    let ir = llm::provider::ToolCall {
        id: call.id.clone(),
        function: llm::provider::FunctionCall {
            name: "execute_decision".into(),
            arguments: json!({"ir":"1 V {}"}).to_string(),
        },
    };
    let result: Value = serde_json::from_str(&dispatch.execute(&ir).await).unwrap();
    assert_eq!(result["receipt"]["registry_revision"], revision);
    assert_eq!(result["receipt"]["verified"], true, "{result}");
    action_contracts::require(&user, "_complete").unwrap();
    // Changed owners/contracts cannot execute even when a caller supplies a new registry.
    let mut changed = PluginRegistry::new();
    let mut owner = plugin("alpha");
    owner.tools[0].handler = crate::plugins::PluginHandler::Http {
        url: "http://127.0.0.1:1/must-not-run".into(),
        method: "POST".into(),
    };
    changed.register(owner);
    let dispatch = tool_dispatch::DispatchContext::new(
        root.path(),
        &state.db,
        &user,
        &changed,
        tool_dispatch::DispatchMode::Agent,
    );
    let denied = dispatch
        .execute(&llm::provider::ToolCall {
            id: "changed".into(),
            ..call.clone()
        })
        .await;
    assert!(
        denied.contains("registry revision") && denied.contains("not executed"),
        "{denied}"
    );
    let denied = dispatch
        .execute(&llm::provider::ToolCall {
            id: "changed-ir".into(),
            ..ir
        })
        .await;
    assert!(
        denied.contains("registry revision") && denied.contains("not executed"),
        "{denied}"
    );
    let direct = changed
        .execute_tool_for_task(&user, "direct", "alpha_check", &json!({}), None, None)
        .await
        .unwrap_err();
    assert!(direct.to_string().contains("registry revision"), "{direct}");
    let owner = changed.get("alpha").unwrap();
    let direct =
        crate::plugins::contracts::start_receipt(owner, &owner.tools[0], &user, "low-level")
            .err()
            .unwrap();
    assert!(direct.to_string().contains("registry revision"), "{direct}");
    assert_eq!(task_control::registry_revision(&user).unwrap(), revision);
}

#[tokio::test]
async fn foundation_unconfigured_chat_and_agent_leave_context_and_history_unchanged() {
    let (_root, state, user) = fixture();
    let mut ctx = state.db.load_context(&user).unwrap();
    ctx.settings.done = true;
    state.db.save_context(&ctx).unwrap();
    let before = json!(ctx);
    let mut events = crate::runtime::events::get_or_create(&user).subscribe();
    let error = message_handler::handle_message(&state, &user, "hello", Some("web"))
        .await
        .unwrap_err();
    let setup = error.downcast_ref::<inference::SetupError>().unwrap();
    assert_eq!(setup.code, "provider_not_configured");
    assert!(!setup.retryable);
    assert_eq!(json!(state.db.load_context(&user).unwrap()), before);
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert!(task_control::cancellation(&user).is_none());
    assert!(action_contracts::action_root(&user).is_err());
    let error = agent_loop::run_agent_loop(&state, &user, "hello", Default::default(), None)
        .await
        .unwrap_err();
    assert!(
        error.downcast_ref::<inference::SetupError>().is_some(),
        "{error}"
    );
    assert_eq!(json!(state.db.load_context(&user).unwrap()), before);
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert!(agent_loop::get_user_input_sender(&user).await.is_none());
    assert!(events.try_recv().is_err());
    crate::runtime::events::remove(&user);
}

#[tokio::test]
async fn foundation_missing_provider_preserves_a_completed_graph_at_its_final_node() {
    let (root, state, user) = fixture();
    std::fs::write(root.path().join("contexts/standard.sm"), "@routing graph\n@start entry\n[state entry]\nsettings.provider = openai\nsettings.system_template = standard\n[state done]\nsettings.system_template = standard\n[transitions]\nentry -> done\n").unwrap();
    let mut ctx = state.db.load_context(&user).unwrap();
    ctx.active_state = Some("done".into());
    ctx.settings.active_state = ctx.active_state.clone();
    ctx.settings.done = true;
    state.db.save_context(&ctx).unwrap();
    let error = message_handler::handle_message(&state, &user, "next task", None)
        .await
        .unwrap_err();
    assert!(
        error.downcast_ref::<inference::SetupError>().is_some(),
        "{error}"
    );
    assert_eq!(json!(state.db.load_context(&user).unwrap()), json!(ctx));
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
}

#[tokio::test]
async fn foundation_missing_fallback_blocks_inference_before_history_or_receipt_binding() {
    let (_root, state, user) = fixture();
    // Exercise a configured local primary with an unconfigured explicit fallback.
    let mut config = state.config.clone();
    config.use_provider = "ollama".into();
    config.llm_fallback_providers = vec!["missing_fallback".into()];
    state
        .llm
        .swap(llm::LLMRouter::new(&config, &Default::default()));
    let _task = task_control::begin(&user).unwrap();
    let error = inference::check_task(&state, &user, "hello", None, None).await.unwrap_err();
    assert!(error.to_string().contains("missing_fallback"), "{error}");
    assert!(error.downcast_ref::<inference::SetupError>().is_some());
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert!(action_contracts::action_root(&user).is_err());
}

#[tokio::test]
async fn foundation_websocket_reports_setup_error_and_remains_usable() {
    use futures_util::{SinkExt, StreamExt};
    let (_root, state, user) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let app = routes(state.clone());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"message","user_id":user,"content":"hello"}).to_string(),
        ))
        .await
        .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            if value["type"] == "error" {
                break value;
            }
        }
    })
    .await
    .unwrap();
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("provider_not_configured"));
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"ping"}).to_string(),
        ))
        .await
        .unwrap();
    let pong = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(pong.to_text().unwrap()).unwrap()["type"],
        "pong"
    );
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert!(task_control::cancellation(&user).is_none());
}

struct ReadyProvider(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl llm::provider::LLMProvider for ReadyProvider {
    fn name(&self) -> &str {
        "fixture"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(
        &self,
        _: llm::provider::ChatRequest,
    ) -> Result<llm::provider::ChatResponse, llm::error::ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(llm::provider::ChatResponse {
            content: Some("hello".into()),
            reasoning_content: None,
            tool_calls: None,
            finish_reason: None,
            usage: None,
        })
    }
}

#[tokio::test]
async fn foundation_direct_agent_restarts_a_completed_default_graph_before_provider_selection() {
    if std::env::var_os("POML_CLI").is_none() {
        assert_ne!(
            std::env::var("PRAXIS_REQUIRE_POML").ok().as_deref(),
            Some("1"),
            "POML_CLI is required for this run"
        );
        eprintln!("skipping real renderer: set POML_CLI and PRAXIS_REQUIRE_POML=1");
        return;
    }
    let (root, state, user) = fixture();
    std::fs::write(root.path().join("contexts/restart.sm"), "@routing graph\n@start entry\n[state entry]\nsettings.provider = fixture\nsettings.system_template = standard\n[state done]\nsettings.provider = openai\nsettings.system_template = standard\n[transitions]\nentry -> done\n").unwrap();
    std::fs::write(
        root.path().join("templates/user.poml"),
        include_str!("../../templates/user.poml"),
    )
    .unwrap();
    let mut ctx = state.db.load_context(&user).unwrap();
    ctx.sm_file = None;
    ctx.settings.sm_file = None;
    ctx.active_state = Some("done".into());
    ctx.settings.active_state = ctx.active_state.clone();
    ctx.settings.done = true;
    state.db.save_context(&ctx).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    state.llm.swap(llm::LLMRouter::with_providers(
        vec![Box::new(ReadyProvider(calls.clone()))],
        "fixture".into(),
        vec![],
        Default::default(),
    ));
    let result = agent_loop::run_agent_loop(
        &state,
        &user,
        "hello",
        agent_loop::AgentLoopConfig {
            sm_file: Some("restart".into()),
            max_turns: 1,
            tags_enabled: false,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.response, "hello");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .db
            .load_context(&user)
            .unwrap()
            .active_state
            .as_deref(),
        Some("entry")
    );
}

#[tokio::test]
async fn foundation_inference_preflight_honors_workflow_provider_override_and_router_swap() {
    let (root, state, user) = fixture();
    let _task = task_control::begin(&user).unwrap();
    assert!(inference::check_task(&state, &user, "hello", None, None).await.is_err());
    let calls = Arc::new(AtomicUsize::new(0));
    state.llm.swap(llm::LLMRouter::with_providers(
        vec![Box::new(ReadyProvider(calls.clone()))],
        "openai".into(),
        vec![],
        Default::default(),
    ));
    std::fs::write(
        root.path().join("contexts/standard.sm"),
        "[state standard]\nsettings.system_template = standard\nsettings.provider = fixture\n",
    )
    .unwrap();
    // The default is still unconfigured; trusted state routing chooses a usable provider.
    inference::check_task(&state, &user, "hello", None, None).await.unwrap();
    assert!(state.llm.get().validate_configuration().is_err());
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "setup must not send inference"
    );
    assert!(state
        .db
        .load_context(&user)
        .unwrap()
        .settings
        .provider
        .is_none());
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn foundation_management_routes_stay_authenticated_with_no_inference_provider() {
    let (_root, state, user) = fixture();
    assert!(!state.llm.is_initialized());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = routes(state.clone());
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{url}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{url}/api/status"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let status: Value = client
        .get(format!("{url}/api/status"))
        .bearer_auth(&state.config.gateway_api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["status"], "ok");
    assert_eq!(status["inference"]["ready"], false);
    assert_eq!(
        status["inference"]["error"]["code"],
        "provider_not_configured"
    );
    for path in [
        format!("/v1/context/{user}"),
        "/v1/providers".into(),
        "/v1/sessions".into(),
    ] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .bearer_auth(&state.config.gateway_api_key)
                .send()
                .await
                .unwrap()
                .status(),
            200,
            "{path}"
        );
    }
    let chat = client
        .post(format!("{url}/v1/chat"))
        .bearer_auth(&state.config.gateway_api_key)
        .json(&json!({"user_id":user,"message":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(chat.status(), 503);
    let error: Value = chat.json().await.unwrap();
    assert_eq!(error["success"], false);
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("provider_not_configured"));
    assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
    assert!(
        !state.llm.is_initialized(),
        "management and rejected inference must not initialize clients"
    );
    let calls = Arc::new(AtomicUsize::new(0));
    state.llm.swap(llm::LLMRouter::with_providers(
        vec![Box::new(ReadyProvider(calls.clone()))],
        "fixture".into(),
        vec![],
        Default::default(),
    ));
    let status: Value = client
        .get(format!("{url}/api/status"))
        .bearer_auth(&state.config.gateway_api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["default_provider"], "fixture");
    assert_eq!(status["inference"]["ready"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn foundation_lazy_provider_inventory_matches_the_initialized_router() {
    let mut config = crate::config::Config::from_env();
    config.use_provider = "ollama".into();
    let mut secrets = crate::db::secrets::Secrets::default();
    secrets.openai_api_key = Some("fixture-openai".into());
    secrets.anthropic_api_key = Some("fixture-anthropic".into());
    secrets.openrouter_api_key = Some("fixture-openrouter".into());
    secrets.minimax_api_key = Some("fixture-minimax".into());
    secrets.mimo_api_key = Some(" ".into());
    let handle = LlmHandle::configured(config, secrets);
    let names = handle.provider_names();
    handle.validate_request_configuration(None).unwrap();
    assert!(!handle.is_initialized());
    assert_eq!(handle.get().provider_names(), names);
    assert!(handle.is_initialized());
    assert!(Arc::ptr_eq(&handle.get(), &handle.get()));
    assert!(!names.iter().any(|name| name == "mimo"));
}

#[test]
fn foundation_management_startup_still_rejects_invalid_host_configuration() {
    let (_root, state, _user) = fixture();
    let mut config = state.config.clone();
    config.gateway_api_key = "short".into();
    assert!(management_state(
        state.db.clone(),
        config,
        Default::default(),
        PluginRegistry::new()
    )
    .is_err());
    let mut config = state.config.clone();
    config.llm_resilience.max_attempts = 0;
    assert!(management_state(state.db, config, Default::default(), PluginRegistry::new()).is_err());
}
