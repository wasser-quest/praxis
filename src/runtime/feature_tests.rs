use super::features::{InvocationContext, NativeService, INVOCATION_API_VERSION};
use crate::{db::Database, gateway::task_control, plugins::PluginRegistry};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Fixture {
    calls: AtomicUsize,
    stopped: AtomicUsize,
    contexts: Mutex<Vec<InvocationContext>>,
    entered: Notify,
    release: Notify,
    block: bool,
}

#[async_trait::async_trait]
impl NativeService for Fixture {
    async fn invoke(
        &self,
        context: InvocationContext,
        operation: &str,
        args: Value,
    ) -> anyhow::Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.contexts.lock().unwrap().push(context.clone());
        self.entered.notify_one();
        if self.block {
            self.release.notified().await;
        }
        if operation == "store" {
            context.storage_put("state", &args)?;
        }
        if operation == "error" {
            anyhow::bail!("native implementation error includes PRIVATE_SECRET");
        }
        Ok(
            json!({"args":args,"user":context.user(),"task":context.task_id(),"call":context.call_id()}),
        )
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn native_feature_unready_service_is_hidden_until_host_initialization_and_forced_stop_runs() {
    use std::sync::atomic::AtomicBool;
    struct Lifecycle {
        ready: AtomicBool,
        initialized: AtomicUsize,
        forced: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl NativeService for Lifecycle {
        fn available(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }
        async fn initialize(&self) -> anyhow::Result<()> {
            self.initialized.fetch_add(1, Ordering::SeqCst);
            self.ready.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn force_stop(&self) {
            self.forced.fetch_add(1, Ordering::SeqCst);
            self.ready.store(false, Ordering::SeqCst);
        }
        async fn invoke(&self, _: InvocationContext, _: &str, _: Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
        async fn shutdown(&self) -> anyhow::Result<()> {
            std::future::pending().await
        }
    }
    let mut registry = registry(false);
    let service = Arc::new(Lifecycle {
        ready: AtomicBool::new(false),
        initialized: AtomicUsize::new(0),
        forced: AtomicUsize::new(0),
    });
    registry
        .register_service("fixture", "backend", 1, &["echo"], service.clone())
        .unwrap();
    let handle = registry.service_handle("fixture", "backend").unwrap();
    assert!(!handle.enabled());
    assert!(registry.tool_definitions().is_empty());
    assert_eq!(service.initialized.load(Ordering::SeqCst), 0);
    handle.initialize().await.unwrap();
    assert!(handle.enabled());
    assert_eq!(registry.tool_definitions().len(), 1);
    assert!(!handle.disable(Duration::from_millis(10)).await.unwrap());
    assert_eq!(service.forced.load(Ordering::SeqCst), 1);
    assert!(!handle.enabled());
    assert!(handle.initialize().await.is_err());
}

fn registry(contract: bool) -> PluginRegistry {
    let mut value = json!({"name":"fixture","version":"1","description":"native fixture","secrets":["declared"],"tools":[{
        "name":"feature_echo","description":"fixture operation",
        "parameters":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},
        "handler":{"type":"service","service":"backend","operation":"echo","api_version":1,"timeout_secs":30}
    }]});
    if contract {
        value["tools"][0]["contract"] = json!({"effect":"read_only","idempotency":"idempotent","timeout_secs":30,"postconditions":[{"program":"/bin/true"}]});
    }
    let mut registry = PluginRegistry::new();
    registry
        .try_register(serde_json::from_value(value).unwrap())
        .unwrap();
    registry
}

fn task(
    db: &Database,
    registry: &PluginRegistry,
    root: &std::path::Path,
    user: &str,
) -> task_control::TaskGuard {
    db.load_context(user).unwrap();
    let task = task_control::begin(user).unwrap();
    task_control::pin_registry(user, registry).unwrap();
    task_control::pin_workspace(user, root).unwrap();
    task
}

fn bind(registry: &mut PluginRegistry, fixture: Arc<Fixture>) {
    registry
        .register_service(
            "fixture",
            "backend",
            INVOCATION_API_VERSION,
            &["echo", "store", "error"],
            fixture,
        )
        .unwrap();
}

fn short_deadline(registry: &mut PluginRegistry) {
    let mut plugin = registry.get("fixture").unwrap().clone();
    if let crate::plugins::PluginHandler::Service(adapter) = &mut plugin.tools[0].handler {
        adapter.timeout_secs = 1;
    }
    registry.register(plugin);
}

async fn invoke(
    registry: &PluginRegistry,
    db: &Database,
    user: &str,
    call: &str,
) -> anyhow::Result<String> {
    registry
        .execute_tool_with_host(
            db,
            user,
            call,
            "feature_echo",
            &json!({"message":"hello"}),
            None,
            None,
        )
        .await
}

#[tokio::test]
async fn native_feature_registration_checks_dependencies_without_starting_the_service() {
    let mut registry = registry(false);
    let before = registry.revision().unwrap();
    let fixture = Arc::new(Fixture::default());
    assert!(registry
        .register_service("missing", "backend", 1, &["echo"], fixture.clone())
        .is_err());
    assert!(registry
        .register_service("fixture", "backend", 2, &["echo"], fixture.clone())
        .is_err());
    assert!(registry
        .register_service("fixture", "backend", 1, &["other"], fixture.clone())
        .is_err());
    assert_eq!(registry.revision().unwrap(), before);
    assert!(registry.tool_definitions().is_empty());
    bind(&mut registry, fixture.clone());
    assert_ne!(registry.revision().unwrap(), before);
    assert_eq!(
        registry.clone().revision().unwrap(),
        registry.revision().unwrap()
    );
    assert_eq!(registry.tool_definitions().len(), 1);
    assert!(registry
        .register_service("fixture", "backend", 1, &["echo"], fixture.clone())
        .is_err());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.stopped.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_feature_scope_comes_from_host_and_expires_after_call() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    let user = "native-feature-scope";
    let mut ctx = db.load_context(user).unwrap();
    ctx.session_id = "private-session".into();
    ctx.settings.path = "/model/chosen/root".into();
    db.save_context(&ctx).unwrap();
    let _task = task(&db, &registry, root.path(), user);
    let secrets = HashMap::from([
        ("declared".into(), "permitted".into()),
        ("other".into(), "PRIVATE_SECRET".into()),
    ]);
    let result = registry
        .execute_tool_with_host(
            &db,
            user,
            "real-call",
            "feature_echo",
            &json!({"message":"hello"}),
            None,
            Some(&secrets),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["result"]["call"],
        "real-call"
    );
    let context = fixture.contexts.lock().unwrap()[0].clone();
    assert_eq!(context.user(), user);
    assert_eq!(context.session(), "private-session");
    assert_eq!(context.owner(), "fixture");
    assert_eq!(context.workspace(), root.path().canonicalize().unwrap());
    assert_eq!(context.task_id(), task_control::task_id(user).unwrap());
    assert_eq!(context.registry_revision(), registry.revision().unwrap());
    assert_eq!(context.secret("declared"), Some("permitted"));
    assert!(context.secret("other").is_none());
    assert!(context.cancellation().is_cancelled());
    assert!(context.storage_get("state").is_err());
    assert!(!task_control::cancellation(user).unwrap().is_cancelled());
    assert!(registry
        .execute_tool_for_task(
            user,
            "no-host",
            "feature_echo",
            &json!({"message":"x"}),
            None,
            None
        )
        .await
        .is_err());
}

#[tokio::test]
async fn native_feature_missing_disabled_and_stale_bindings_cannot_execute() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let user = "native-feature-stale";
    let guard = task(&db, &registry, root.path(), user);
    assert!(invoke(&registry, &db, user, "missing").await.is_err());
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    assert!(invoke(&registry, &db, user, "stale").await.is_err());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    drop(guard);
    let _guard = task(&db, &registry, root.path(), user);
    registry
        .service_handle("fixture", "backend")
        .unwrap()
        .disable(Duration::from_millis(50))
        .await
        .unwrap();
    assert!(registry.tool_definitions().is_empty());
    assert!(invoke(&registry, &db, user, "disabled").await.is_err());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_feature_contract_uses_host_receipts_and_preserves_task_identity() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(true);
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    let user = "native-feature-contract";
    let _task = task(&db, &registry, root.path(), user);
    let task_id = task_control::task_id(user).unwrap();
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());
    let result: Value =
        serde_json::from_str(&invoke(&registry, &db, user, "verified").await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["outcome"], "committed");
    assert_eq!(result["receipt"]["verified"], true);
    assert_eq!(result["receipt"]["task_id"], task_id);
    assert_eq!(
        result["receipt"]["registry_revision"],
        registry.revision().unwrap()
    );
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_ok());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_feature_dispatch_runs_in_both_ingresses_and_lowered_ir() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("contexts")).unwrap();
    let text = "[state working]\nsettings.activated_tools = [\"execute_decision\",\"feature_echo\"]\n[decision_ir]\nE = fixture/feature_echo\n[action_guards]\n_complete = [fixture/feature_echo]";
    std::fs::write(root.path().join("contexts/fixture.sm"), text).unwrap();
    let sm = crate::sm::parse(text).unwrap();
    let db = Database::new(root.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut registry = registry(true);
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    for (mode, label) in [(DispatchMode::Chat, "chat"), (DispatchMode::Agent, "agent")] {
        for ir in [false, true] {
            let user = format!("native-feature-ingress-{label}-{ir}");
            let mut ctx = db.load_context(&user).unwrap();
            ctx.sm_file = Some("fixture".into());
            ctx.active_state = Some("working".into());
            ctx.settings.active_state = ctx.active_state.clone();
            ctx.settings.activated_tools = vec!["execute_decision".into(), "feature_echo".into()];
            db.save_context(&ctx).unwrap();
            let _task = task(&db, &registry, root.path(), &user);
            crate::gateway::action_contracts::bind(&user, "fixture", &sm, root.path()).unwrap();
            let call = ToolCall {
                id: format!("call-{label}-{ir}"),
                function: FunctionCall {
                    name: if ir {
                        "execute_decision"
                    } else {
                        "feature_echo"
                    }
                    .into(),
                    arguments: if ir {
                        json!({"ir":"1 E {\"message\":\"hello\"}"})
                    } else {
                        json!({"message":"hello"})
                    }
                    .to_string(),
                },
            };
            let result = DispatchContext::new(root.path(), &db, &user, &registry, mode)
                .execute(&call)
                .await;
            let result: Value =
                serde_json::from_str(&result).unwrap_or_else(|_| panic!("{result}"));
            assert_eq!(result["receipt"]["verified"], true);
            assert_eq!(result["result"]["user"], user);
            assert_eq!(result["receipt"]["call_id"], call.id);
        }
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn native_feature_deadline_and_failed_postcondition_never_grant_completion() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut timed_registry = registry(true);
    short_deadline(&mut timed_registry);
    let fixture = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    bind(&mut timed_registry, fixture.clone());
    let user = "native-feature-deadline";
    let _task = task(&db, &timed_registry, root.path(), user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    match invoke(&timed_registry, &db, user, "timeout").await {
        Ok(result) => {
            let result: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(result["receipt"]["failure"], "timed_out");
            assert_eq!(result["receipt"]["verified"], false);
            let contexts = fixture.contexts.lock().unwrap();
            if let Some(context) = contexts.first() {
                assert!(context.cancellation().is_cancelled());
            } else {
                // The contract can expire during checks after acquiring the
                // shared lock, producing a failed receipt before the callback.
                assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
            }
        }
        // Other contract tests can hold the shared lock. Expiry before the
        // callback creates no receipt and no authority, which is correct too.
        Err(error) => {
            assert!(error.to_string().contains("timed out"), "{error}");
            assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
        }
    }
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());

    let mut failed = registry(true);
    let mut package = failed.get("fixture").unwrap().clone();
    package.tools[0].contract.as_mut().unwrap().postconditions[0].program = "/bin/false".into();
    failed.register(package);
    bind(&mut failed, Arc::new(Fixture::default()));
    let user = "native-feature-postcondition";
    let _task = task(&db, &failed, root.path(), user);
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    let result: Value =
        serde_json::from_str(&invoke(&failed, &db, user, "failed-post").await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["attempted"], true);
    assert_eq!(result["receipt"]["failure"], "condition_failed");
    assert_eq!(result["receipt"]["verified"], false);
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn native_feature_deadline_cancels_an_already_running_callback() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    short_deadline(&mut registry);
    let fixture = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    bind(&mut registry, fixture.clone());
    let user = "native-feature-handler-deadline";
    let _task = task(&db, &registry, root.path(), user);
    let error = invoke(&registry, &db, user, "deadline")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert!(fixture.contexts.lock().unwrap()[0]
        .cancellation()
        .is_cancelled());
}

#[tokio::test]
async fn native_feature_mutating_contract_runs_declared_compensation_without_granting_completion() {
    struct Write;
    #[async_trait::async_trait]
    impl NativeService for Write {
        async fn invoke(&self, ctx: InvocationContext, _: &str, _: Value) -> anyhow::Result<Value> {
            std::fs::write(ctx.workspace().join("source"), "changed")?;
            Ok(json!({"written":true}))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    std::fs::write(root.path().join("source"), "old").unwrap();
    let cleanup = root.path().join("restore.sh");
    std::fs::write(&cleanup, "printf old > source\n").unwrap();
    let mut registry = registry(true);
    let mut plugin = registry.get("fixture").unwrap().clone();
    plugin.tools[0].contract = Some(serde_json::from_value(json!({
        "effect":"workspace_write","idempotency":"non_idempotent","timeout_secs":30,
        "postconditions":[{"program":"/bin/false","resources":["source"]}],
        "compensation":{
            "handler":{"type":"script","path":cleanup,"interpreter":"/bin/sh"},
            "postconditions":[{"program":"/bin/sh","args":["-c","test \"$(cat source)\" = old"],"resources":["source"]}],
            "timeout_secs":5
        }
    })).unwrap());
    registry.register(plugin);
    registry
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Write))
        .unwrap();
    let user = "native-feature-compensation";
    let _task = task(&db, &registry, root.path(), user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    let result: Value =
        serde_json::from_str(&invoke(&registry, &db, user, "compensate").await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["attempted"], true);
    assert_eq!(result["receipt"]["verified"], false);
    assert_eq!(result["receipt"]["outcome"], "compensated");
    assert_eq!(result["receipt"]["compensation_verified"], true);
    assert_eq!(
        std::fs::read_to_string(root.path().join("source")).unwrap(),
        "old"
    );
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn native_feature_rechecks_live_enable_before_contract_handler() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(true);
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    let user = "native-feature-queued-disable";
    let _task = task(&db, &registry, root.path(), user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    let lock = crate::tools::apply_patch::FILE_OPERATIONS.lock().await;
    let future = invoke(&registry, &db, user, "queued");
    tokio::pin!(future);
    tokio::select! { result = &mut future => panic!("must queue: {result:?}"), _ = tokio::task::yield_now() => {} }
    crate::db::tools::set_plugin_tool_enabled(&db, "feature_echo", false).unwrap();
    drop(lock);
    let result = future.await;
    assert!(
        result.is_err()
            || !serde_json::from_str::<Value>(&result.unwrap()).unwrap()["receipt"]["verified"]
                .as_bool()
                .unwrap()
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn native_feature_deadline_bounds_waiting_for_the_contract_lock() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(true);
    short_deadline(&mut registry);
    let fixture = Arc::new(Fixture::default());
    bind(&mut registry, fixture.clone());
    let user = "native-feature-lock-deadline";
    let _task = task(&db, &registry, root.path(), user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    let _lock = crate::tools::apply_patch::FILE_OPERATIONS.lock().await;
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        invoke(&registry, &db, user, "waiting"),
    )
    .await
    .unwrap()
    .unwrap_err()
    .to_string();
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_feature_stop_cancels_call_and_cannot_cancel_parent_through_context() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let fixture = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    bind(&mut registry, fixture.clone());
    let user = "native-feature-cancel";
    let _task = task(&db, &registry, root.path(), user);
    let future = invoke(&registry, &db, user, "cancel");
    tokio::pin!(future);
    tokio::select! { result = &mut future => panic!("must block: {result:?}"), _ = fixture.entered.notified() => {} }
    let context = fixture.contexts.lock().unwrap()[0].clone();
    context.cancellation().cancel();
    assert!(!task_control::cancellation(user).unwrap().is_cancelled());
    assert!(future.await.unwrap_err().to_string().contains("cancelled"));
    // A second call uses a fresh child token; stopping the task cancels it too.
    let future = invoke(&registry, &db, user, "stop");
    tokio::pin!(future);
    tokio::select! { result = &mut future => panic!("must block: {result:?}"), _ = fixture.entered.notified() => {} }
    task_control::cancel(user);
    assert!(future.await.unwrap_err().to_string().contains("cancelled"));
}

#[tokio::test]
async fn native_feature_disable_drains_or_forces_and_shutdown_is_once() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    for forced in [false, true] {
        let mut registry = registry(false);
        let fixture = Arc::new(Fixture {
            block: true,
            ..Default::default()
        });
        bind(&mut registry, fixture.clone());
        let user = format!("native-feature-drain-{forced}");
        let _task = task(&db, &registry, root.path(), &user);
        let handle = registry.service_handle("fixture", "backend").unwrap();
        let call = invoke(&registry, &db, &user, "in-flight");
        tokio::pin!(call);
        tokio::select! { result = &mut call => panic!("must block: {result:?}"), _ = fixture.entered.notified() => {} }
        let stop = handle.disable(if forced {
            Duration::ZERO
        } else {
            Duration::from_secs(1)
        });
        tokio::pin!(stop);
        tokio::select! { _ = &mut call => panic!("not released"), result = &mut stop => { assert!(forced); assert!(!result.unwrap()); }, _ = tokio::task::yield_now(), if !forced => { fixture.release.notify_one(); } }
        if forced {
            assert!(call.await.is_err());
        } else {
            assert!(call.await.is_ok());
            assert!(stop.await.unwrap());
        }
        assert!(!handle.enabled());
        let _ = handle.disable(Duration::from_secs(1)).await.unwrap();
        assert_eq!(fixture.stopped.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn native_feature_storage_is_scoped_durable_and_read_only_contracts_cannot_write() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    struct Storage;
    #[async_trait::async_trait]
    impl NativeService for Storage {
        async fn invoke(
            &self,
            ctx: InvocationContext,
            _: &str,
            args: Value,
        ) -> anyhow::Result<Value> {
            let before = ctx.storage_get("state")?;
            let written = ctx.storage_put("state", &args).is_ok();
            assert!(!ctx
                .storage_compare_exchange("state", None, &json!({}))
                .unwrap_or(false));
            assert!(!ctx
                .storage_compare_exchange("state", Some(&json!({"wrong":"value"})), &json!({}))
                .unwrap_or(false));
            let exchanged = ctx
                .storage_compare_exchange("state", Some(&args), &json!({"saved":ctx.user()}))
                .unwrap_or(false);
            assert!(ctx.storage_put("../outside", &args).is_err());
            assert!(ctx
                .storage_put("oversize", &json!("x".repeat(65537)))
                .is_err());
            Ok(json!({"before":before,"written":written,"exchanged":exchanged}))
        }
    }
    let mut writable = registry(false);
    writable
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Storage))
        .unwrap();
    for (user, expected) in [
        ("storage-a", Value::Null),
        ("storage-b", Value::Null),
        ("storage-a", json!({"saved":"storage-a"})),
    ] {
        let _task = task(&db, &writable, root.path(), user);
        let output: Value =
            serde_json::from_str(&invoke(&writable, &db, user, "storage").await.unwrap()).unwrap();
        assert_eq!(output["result"]["before"], expected);
        assert_eq!(output["result"]["written"], true);
        assert_eq!(output["result"]["exchanged"], true);
    }
    let db = Database::new(root.path()).unwrap();
    {
        let _task = task(&db, &writable, root.path(), "storage-a");
        let result: Value = serde_json::from_str(
            &invoke(&writable, &db, "storage-a", "reopened")
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["result"]["before"], json!({"saved":"storage-a"}));
    }
    let mut readonly = registry(true);
    readonly
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Storage))
        .unwrap();
    let _task = task(&db, &readonly, root.path(), "storage-readonly");
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind("storage-readonly", "fixture", &sm, root.path())
        .unwrap();
    let output: Value = serde_json::from_str(
        &invoke(&readonly, &db, "storage-readonly", "readonly")
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["result"]["written"], false);
    assert_eq!(output["result"]["exchanged"], false);
}

#[test]
fn native_feature_preflight_checks_selected_services_without_ir_and_leaves_core_workflows_usable() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let registry = registry(false);
    let ctx = db.load_context("service-preflight").unwrap();
    let selected =
        crate::sm::parse("[state working]\nsettings.activated_tools = [\"feature_echo\"]").unwrap();
    let error =
        crate::gateway::workflow_preflight::validate(&db, &registry, "fixture", &selected, &ctx)
            .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::gateway::workflow_preflight::SetupError>()
            .unwrap()
            .code,
        "service_unavailable"
    );
    let core =
        crate::sm::parse("[state working]\nsettings.activated_tools = [\"inspect_file\"]").unwrap();
    assert!(
        crate::gateway::workflow_preflight::validate(&db, &registry, "fixture", &core, &ctx)
            .is_ok()
    );
    let implicit = crate::sm::parse("[state working]").unwrap();
    let mut implicit_ctx = ctx.clone();
    implicit_ctx.settings.activated_tools.clear();
    implicit_ctx.settings.full_tool_schemas.clear();
    implicit_ctx.settings.full_tool_categories.clear();
    assert!(crate::gateway::workflow_preflight::validate(
        &db,
        &registry,
        "fixture",
        &implicit,
        &implicit_ctx
    )
    .is_ok());
    let mut disabled = registry.clone();
    let mut package = disabled.get("fixture").unwrap().clone();
    package.enabled = false;
    disabled.register(package);
    let error =
        crate::gateway::workflow_preflight::validate(&db, &disabled, "fixture", &selected, &ctx)
            .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::gateway::workflow_preflight::SetupError>()
            .unwrap()
            .code,
        "plugin_disabled"
    );
}

#[tokio::test]
async fn native_feature_scope_rejects_changed_session_workspace_and_restarted_task() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let fixture = Arc::new(Fixture {
        block: true,
        ..Default::default()
    });
    bind(&mut registry, fixture.clone());
    let user = "native-feature-scope-change";
    let task_guard = task(&db, &registry, root.path(), user);
    assert!(task_control::pin_workspace(user, elsewhere.path()).is_err());
    let call = invoke(&registry, &db, user, "old-scope");
    tokio::pin!(call);
    tokio::select! { result = &mut call => panic!("must block: {result:?}"), _ = fixture.entered.notified() => {} }
    let context = fixture.contexts.lock().unwrap()[0].clone();
    context.storage_put("state", &json!("before")).unwrap();
    let mut ctx = db.load_context(user).unwrap();
    ctx.session_id = "another-session".into();
    db.save_context(&ctx).unwrap();
    assert!(context.storage_put("state", &json!("after")).is_err());
    let old_id = context.task_id().to_string();
    drop(task_guard);
    let _replacement = task(&db, &registry, root.path(), user);
    assert_ne!(task_control::task_id(user).unwrap(), old_id);
    assert!(context.storage_put("state", &json!("leaked")).is_err());
    assert!(call.await.is_err());
}

#[tokio::test]
async fn native_feature_outputs_are_bounded_and_errors_do_not_expose_native_details() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    struct Results;
    #[async_trait::async_trait]
    impl NativeService for Results {
        async fn invoke(
            &self,
            _: InvocationContext,
            _: &str,
            args: Value,
        ) -> anyhow::Result<Value> {
            if args["message"] == "error" {
                anyhow::bail!("PRIVATE_SECRET /private/path native traceback");
            }
            if args["message"] == "large" {
                return Ok(json!("x".repeat(1024 * 1024)));
            }
            Ok(json!({"receipt":{"verified":true,"outcome":"committed"}}))
        }
    }
    let mut registry = registry(false);
    registry
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Results))
        .unwrap();
    let user = "native-feature-output";
    let _task = task(&db, &registry, root.path(), user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/feature_echo]")
            .unwrap();
    crate::gateway::action_contracts::bind(user, "fixture", &sm, root.path()).unwrap();
    for (message, expected) in [("error", "failed"), ("large", "too large")] {
        let error = registry
            .execute_tool_with_host(
                &db,
                user,
                "result",
                "feature_echo",
                &json!({"message":message}),
                None,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("PRIVATE_SECRET"));
        assert!(!error.contains("/private/path"));
    }
    assert!(registry
        .execute_tool_with_host(
            &db,
            user,
            "bad-input",
            "feature_echo",
            &json!({}),
            None,
            None
        )
        .await
        .is_err());
    let output: Value =
        serde_json::from_str(&invoke(&registry, &db, user, "forged").await.unwrap()).unwrap();
    assert!(output.get("receipt").is_none());
    assert!(output["result"]["receipt"]["verified"].as_bool().unwrap());
    assert!(crate::gateway::action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn native_feature_failures_are_displayed_as_errors_in_both_ingresses() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let mut package = registry.get("fixture").unwrap().clone();
    if let crate::plugins::PluginHandler::Service(adapter) = &mut package.tools[0].handler {
        adapter.operation = "error".into();
    }
    registry.register(package);
    bind(&mut registry, Arc::new(Fixture::default()));
    for (mode, label) in [(DispatchMode::Chat, "chat"), (DispatchMode::Agent, "agent")] {
        let user = format!("native-feature-error-ingress-{label}");
        let _task = task(&db, &registry, root.path(), &user);
        let call = ToolCall {
            id: "failed".into(),
            function: FunctionCall {
                name: "feature_echo".into(),
                arguments: json!({"message":"x"}).to_string(),
            },
        };
        let result = DispatchContext::new(root.path(), &db, &user, &registry, mode)
            .execute(&call)
            .await;
        assert!(result.starts_with("Error:"), "{result}");
        assert!(!crate::gateway::tool_results::display_success(&result));
        assert!(!result.contains("PRIVATE_SECRET"));
    }
}

#[tokio::test]
async fn native_feature_storage_cannot_cross_package_owners_and_binding_revisions_change() {
    struct Store;
    #[async_trait::async_trait]
    impl NativeService for Store {
        async fn invoke(&self, ctx: InvocationContext, _: &str, _: Value) -> anyhow::Result<Value> {
            let before = ctx.storage_get("shared-key")?;
            ctx.storage_put("shared-key", &json!(ctx.owner()))?;
            Ok(json!({"before":before}))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let db = Database::new(root.path()).unwrap();
    let mut registry = registry(false);
    let mut second = registry.get("fixture").unwrap().clone();
    second.name = "second".into();
    second.tools[0].name = "feature_other".into();
    registry.try_register(second).unwrap();
    registry
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Store))
        .unwrap();
    registry
        .register_service("second", "backend", 1, &["echo"], Arc::new(Store))
        .unwrap();
    let user = "native-feature-owner-storage";
    let _task = task(&db, &registry, root.path(), user);
    for (name, expected) in [
        ("feature_echo", Value::Null),
        ("feature_other", Value::Null),
        ("feature_echo", json!("fixture")),
        ("feature_other", json!("second")),
    ] {
        let result: Value = serde_json::from_str(
            &registry
                .execute_tool_with_host(
                    &db,
                    user,
                    "store",
                    name,
                    &json!({"message":"x"}),
                    None,
                    None,
                )
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["result"]["before"], expected);
    }
    let mut replacement = PluginRegistry::new();
    for package in registry.list() {
        replacement.try_register(package.clone()).unwrap();
    }
    replacement
        .register_service("fixture", "backend", 1, &["echo"], Arc::new(Store))
        .unwrap();
    replacement
        .register_service("second", "backend", 1, &["echo"], Arc::new(Store))
        .unwrap();
    assert_ne!(
        registry.revision().unwrap(),
        replacement.revision().unwrap()
    );
    assert!(task_control::check_registry(user, &replacement).is_err());
}

#[test]
fn native_feature_manifest_versions_and_compensation_are_checked_on_activation() {
    let plain = registry(false);
    let mut package = plain.get("fixture").unwrap().clone();
    if let crate::plugins::PluginHandler::Service(adapter) = &mut package.tools[0].handler {
        adapter.api_version = 2;
    }
    assert!(PluginRegistry::new().try_register(package).is_err());
    assert!(serde_json::from_value::<crate::plugins::PluginHandler>(json!({"type":"service","service":"backend","operation":"echo","api_version":1,"timeout_secs":1,"unknown":"field"})).is_err());
    let registry = registry(true);
    let mut package = registry.get("fixture").unwrap().clone();
    let handler = package.tools[0].handler.clone();
    let contract = package.tools[0].contract.as_mut().unwrap();
    contract.effect = crate::plugins::contracts::EffectClass::ExternalWrite;
    contract.compensation = Some(crate::plugins::contracts::Compensation {
        handler,
        postconditions: contract.postconditions.clone(),
        timeout_secs: 1,
    });
    assert!(PluginRegistry::new().try_register(package).is_err());
}
