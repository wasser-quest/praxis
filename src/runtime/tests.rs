use super::{
    events, retention,
    services::{ServiceHost, SERVICE_API_VERSION},
    templates,
};
use crate::{db::Database, gateway, plugins::PluginRegistry};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{oneshot, Notify};

#[test]
fn runtime_events_are_user_scoped_and_share_the_dashboard_adapter() {
    let a = "runtime-events-a";
    let b = "runtime-events-b";
    events::remove(a);
    events::remove(b);
    assert!(events::subscribe(a).is_none());
    let mut core = events::get_or_create(a).subscribe();
    let mut dashboard = crate::runtime::events::subscribe(a).unwrap();
    let mut other = events::get_or_create(b).subscribe();
    events::assistant_saved(a, 42, "a private reply");
    assert_eq!(core.try_recv().unwrap().event, "assistant_saved");
    let adapted = dashboard.try_recv().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&adapted.data).unwrap(),
        json!({"id":42,"role":"assistant","content":"a private reply"})
    );
    assert!(other.try_recv().is_err());
    crate::runtime::events::send(a, "char", "x");
    assert_eq!(core.try_recv().unwrap().data, "x");
    events::remove(a);
    events::remove(b);
}

#[test]
fn runtime_context_events_publish_only_the_public_speech_flag() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    let user = "runtime-context-events";
    let mut events = events::get_or_create(user).subscribe();
    let mut ctx = db.load_context(user).unwrap();
    ctx.settings.web_chat_tts = true;
    ctx.custom_data = json!({"private_setting":"do not publish"});
    db.save_context(&ctx).unwrap();
    let event = events.try_recv().unwrap();
    assert_eq!(event.event, "chat_tts_settings");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&event.data).unwrap(),
        json!({"enabled":true})
    );
    super::events::remove(user);
    let disconnected = "runtime-context-disconnected";
    super::events::remove(disconnected);
    super::events::chat_tts_settings(disconnected, true);
    assert!(super::events::subscribe(disconnected).is_none());
}

#[test]
fn runtime_template_sync_reads_the_explicit_root_and_preserves_catalog_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("templates");
    std::fs::create_dir_all(root.join("states/local")).unwrap();
    std::fs::write(root.join("standard.poml"), "operator-edited disk content").unwrap();
    std::fs::write(root.join("states/local/prompt.poml"), "nested prompt").unwrap();
    std::fs::write(root.join("ignore.json"), "{}").unwrap();
    let db = Database::new(&dir.path().join("db")).unwrap();
    db.save_template("standard", "old", Some("Operator description"), false)
        .unwrap();
    db.save_template("catalog_only", "keep", Some("Local entry"), false)
        .unwrap();
    assert_eq!(templates::sync_from_disk(&db, &root).unwrap(), 2);
    let standard = db.get_template("standard").unwrap().unwrap();
    assert_eq!(standard.content, "operator-edited disk content");
    assert_eq!(
        standard.description.as_deref(),
        Some("Operator description")
    );
    assert!(!standard.is_system);
    assert!(
        db.get_template("states/local/prompt")
            .unwrap()
            .unwrap()
            .is_system
    );
    assert_eq!(
        db.get_template("catalog_only").unwrap().unwrap().content,
        "keep"
    );
    assert_eq!(templates::sync_from_disk(&db, &root).unwrap(), 0);
    assert_eq!(db.list_templates().unwrap().len(), 3);
    assert_eq!(
        templates::sync_from_disk(&db, &dir.path().join("absent")).unwrap(),
        0
    );
    assert!(templates::sync_from_disk(&db, &root.join("ignore.json")).is_err());
}

#[cfg(unix)]
#[test]
fn runtime_template_sync_cannot_import_outside_symlinks_or_recurse_forever() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("templates");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("standard.poml"), "inside").unwrap();
    std::fs::write(dir.path().join("outside.poml"), "outside").unwrap();
    symlink(dir.path().join("outside.poml"), root.join("escape.poml")).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    let db = Database::new(&dir.path().join("db")).unwrap();
    assert_eq!(templates::sync_from_disk(&db, &root).unwrap(), 1);
    assert!(db.get_template("escape").unwrap().is_none());
    assert_eq!(db.list_templates().unwrap().len(), 1);
}

#[tokio::test]
async fn runtime_service_rejects_bad_versions_duplicates_and_zero_intervals_before_start() {
    let mut host = ServiceHost::new();
    let starts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let count = starts.clone();
    let signal = started.clone();
    assert!(host
        .register_periodic(
            "fixture",
            SERVICE_API_VERSION + 1,
            Duration::from_secs(1),
            || async { Ok(()) }
        )
        .is_err());
    assert!(host
        .register_periodic("fixture", SERVICE_API_VERSION, Duration::ZERO, || async {
            Ok(())
        })
        .is_err());
    host.register_periodic(
        "fixture",
        SERVICE_API_VERSION,
        Duration::from_secs(3600),
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            signal.notify_one();
            async { Ok(()) }
        },
    )
    .unwrap();
    assert!(host
        .register_periodic(
            "fixture",
            SERVICE_API_VERSION,
            Duration::from_secs(1),
            || async { panic!("duplicate must never start") }
        )
        .is_err());
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(host
        .disable("fixture", Duration::from_secs(1))
        .await
        .unwrap());
    assert!(!host.contains("fixture"));
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_service_disable_drains_the_inflight_tick_and_blocks_new_ticks() {
    let mut host = ServiceHost::new();
    let started = Arc::new(Notify::new());
    let finished = Arc::new(AtomicUsize::new(0));
    let (release, wait) = oneshot::channel();
    let mut wait = Some(wait);
    let signal = started.clone();
    let count = finished.clone();
    host.register_periodic(
        "draining",
        SERVICE_API_VERSION,
        Duration::from_millis(1),
        move || {
            let wait = wait.take().expect("no second tick during draining");
            signal.notify_one();
            let count = count.clone();
            async move {
                wait.await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let disabling = host.disable("draining", Duration::from_secs(2));
    tokio::pin!(disabling);
    tokio::select! {
        result = &mut disabling => panic!("inflight tick must drain: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    release.send(()).unwrap();
    assert!(disabling.await.unwrap());
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_service_timeout_aborts_and_drop_cancels_owned_workers() {
    struct DropSignal(Arc<Notify>);
    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    for explicit_disable in [true, false] {
        let mut host = ServiceHost::new();
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(Notify::new());
        let signal = started.clone();
        let on_drop = dropped.clone();
        host.register_periodic(
            "blocked",
            SERVICE_API_VERSION,
            Duration::from_secs(1),
            move || {
                let signal = signal.clone();
                let on_drop = on_drop.clone();
                async move {
                    let _guard = DropSignal(on_drop);
                    signal.notify_one();
                    std::future::pending::<()>().await;
                    Ok(())
                }
            },
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        if explicit_disable {
            assert!(!host
                .disable("blocked", Duration::from_millis(10))
                .await
                .unwrap());
        }
        drop(host);
        tokio::time::timeout(Duration::from_secs(2), dropped.notified())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn runtime_retention_runs_without_plugins_even_when_another_service_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    let old = db
        .save_tool_output("retention-user", "fixture", "call", "old output")
        .unwrap();
    db.conn()
        .execute(
            "UPDATE tool_outputs SET created_at=0 WHERE id=?1",
            [&old.id],
        )
        .unwrap();
    let keep = db
        .save_tool_output("retention-user", "fixture", "call-new", "current")
        .unwrap();
    // Saving an output also prunes; insert the expired row after that save.
    db.conn().execute("INSERT INTO tool_outputs(id,owner_id,session_id,tool_name,call_id,content,retained_bytes,source_bytes,source_chars,created_at) VALUES('expired','retention-user','','fixture','old','x',1,1,1,0)", []).unwrap();
    let mut host = ServiceHost::new();
    host.register_periodic(
        "blocked-feature",
        SERVICE_API_VERSION,
        Duration::from_secs(60),
        || async {
            std::future::pending::<()>().await;
            Ok(())
        },
    )
    .unwrap();
    retention::register(&mut host, db.clone()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT count(*) FROM tool_outputs WHERE id='expired'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if count == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        db.get_tool_output("retention-user", &keep.id)
            .unwrap()
            .unwrap()
            .content,
        "current"
    );
    host.shutdown(Duration::from_millis(10)).await;
    assert!(!host.contains("blocked-feature"));
}

async fn plugin_free_workflow(
    user: &str,
) -> (
    tempfile::TempDir,
    Database,
    crate::db::contexts::Context,
    gateway::task_control::TaskGuard,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("contexts")).unwrap();
    std::fs::create_dir(dir.path().join("templates")).unwrap();
    std::fs::write(dir.path().join("contexts/local.sm"), "@routing graph\n@start first\n[state first]\nsettings.system_template = before\nsm_data.phase = reading\n[state second]\nsettings.system_template = after\nsm_data.phase = explaining\n[transitions]\nfirst -> second\n[decision_ir]\nN = agent_next\nK = agent_back\n").unwrap();
    std::fs::write(dir.path().join("templates/before.poml"), "<poml><p>Before: {{custom_data.nickname}} {{sm_data.phase}} {{custom_data.user_prompt}}</p></poml>").unwrap();
    std::fs::write(dir.path().join("templates/after.poml"), "<poml><p>After: {{custom_data.nickname}} {{sm_data.phase}} {{custom_data.user_prompt}}</p></poml>").unwrap();
    let db = Database::new(&dir.path().join("data")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut ctx = db.load_context(user).unwrap();
    ctx.settings.sm_file = Some("local".into());
    ctx.settings.use_decision_router = false;
    ctx.settings.decision_profile = Some("off".into());
    ctx.active_state = Some("first".into());
    ctx.settings.active_state = ctx.active_state.clone();
    ctx.custom_data = json!({"nickname":"Ada"});
    let task = gateway::task_control::begin(user).unwrap();
    gateway::prompt::route_context(
        dir.path(),
        &mut ctx,
        "Hello from context",
        &PluginRegistry::new(),
        None,
    )
    .await
    .unwrap();
    db.save_context(&ctx).unwrap();
    (dir, db, ctx, task)
}

#[tokio::test]
async fn runtime_context_and_state_graph_work_without_feature_plugins_or_provider() {
    let user = "runtime-plugin-free-graph";
    let (dir, db, ctx, _task) = plugin_free_workflow(user).await;
    let plugins = PluginRegistry::new();
    let value =
        gateway::prompt::build_context(&db, &ctx, "Hello from context", &plugins, 0, dir.path())
            .await
            .unwrap();
    assert_eq!(value["custom_data"]["nickname"], "Ada");
    assert_eq!(value["sm_data"]["phase"], "reading");
    assert_eq!(value["settings"]["system_template"], "before");
    let mut rx = events::get_or_create(user).subscribe();
    let dispatcher = gateway::tool_dispatch::DispatchContext::new(
        dir.path(),
        &db,
        user,
        &plugins,
        gateway::tool_dispatch::DispatchMode::Chat,
    );
    let call = gateway::llm::provider::ToolCall {
        id: "plugin-free-next".into(),
        function: gateway::llm::provider::FunctionCall {
            name: "execute_decision".into(),
            arguments: json!({"ir":"1 N {\"edge\":0}"}).to_string(),
        },
    };
    let result = dispatcher.execute(&call).await;
    let receipt: serde_json::Value =
        serde_json::from_str(&result).unwrap_or_else(|e| panic!("{e}: {result}"));
    assert_eq!(receipt["verified"], true);
    assert_eq!(
        db.load_context(user).unwrap().sm_data["phase"],
        "explaining"
    );
    // Context synchronization may precede the transition event.
    let transition = std::iter::from_fn(|| rx.try_recv().ok())
        .find(|e| e.event == "state_transition")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&transition.data).unwrap()["to_state"],
        "second"
    );
    gateway::workflow_graph::navigate(&db, dir.path(), user, true, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        db.load_context(user).unwrap().active_state.as_deref(),
        Some("first")
    );
    events::remove(user);
}

#[test]
fn runtime_plugin_free_workflow_rejects_required_missing_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let ctx = db.load_context("plugin-free-required-capability").unwrap();
    let sm = crate::sm::parse("[state working]\nsettings.activated_tools = [\"execute_decision\"]\n[decision_ir]\nV = absent/probe").unwrap();
    let error = gateway::workflow_preflight::validate(
        &db,
        &PluginRegistry::new(),
        "requires-plugin",
        &sm,
        &ctx,
    )
    .unwrap_err();
    let setup = error
        .downcast_ref::<gateway::workflow_preflight::SetupError>()
        .unwrap_or_else(|| panic!("Unexpected setup error: {error:#}"));
    assert_eq!(setup.code, "plugin_missing");
    assert_eq!(setup.target, "absent/probe");
    assert!(!setup.retryable);
    assert!(db.execution_events(&ctx.user_id, 10).unwrap().is_empty());
}

#[tokio::test]
async fn runtime_poml_renders_context_changes_without_feature_plugins() {
    if std::env::var_os("POML_CLI").is_none() {
        assert_ne!(
            std::env::var("PRAXIS_REQUIRE_POML").ok().as_deref(),
            Some("1"),
            "POML_CLI is required for this run"
        );
        eprintln!("skipping real renderer: set POML_CLI and PRAXIS_REQUIRE_POML=1");
        return;
    }
    let user = "runtime-plugin-free-poml";
    let (dir, db, before, _task) = plugin_free_workflow(user).await;
    let plugins = PluginRegistry::new();
    async fn render(
        root: &Path,
        db: &Database,
        ctx: &crate::db::contexts::Context,
        plugins: &PluginRegistry,
    ) -> String {
        let context =
            gateway::prompt::build_context(db, ctx, "Hello from context", plugins, 0, root)
                .await
                .unwrap();
        let path = templates::resolve_template(
            &root.join("templates"),
            ctx.settings.system_template.as_deref().unwrap(),
        )
        .unwrap();
        gateway::poml::render_strict(&path.to_string_lossy(), &context)
            .await
            .unwrap()
    }
    assert_eq!(
        render(dir.path(), &db, &before, &plugins).await,
        "Before: Ada reading Hello from context"
    );
    gateway::workflow_graph::navigate(&db, dir.path(), user, false, &json!({"edge":0}))
        .await
        .unwrap();
    let after = db.load_context(user).unwrap();
    assert_eq!(
        render(dir.path(), &db, &after, &plugins).await,
        "After: Ada explaining Hello from context"
    );
}
