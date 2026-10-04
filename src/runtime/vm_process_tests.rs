//! Core/IPC integration, also compiled without the optional QEMU crate.
use crate::{config::Config, db::Database, gateway::task_control, plugins::PluginRegistry};
use serde_json::{json, Value};
use std::time::Duration;

fn fixture(mode: &str) -> (tempfile::TempDir, Database, Config, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let db = Database::new(&data).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let package = dir.path().join("plugins/vm");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("plugin.json"),
        include_str!("../../plugins/vm/plugin.json"),
    )
    .unwrap();
    let worker = package.join("worker");
    std::fs::write(&worker, include_str!("fixtures/vm_service.py")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.path().join("worker.mode"), mode).unwrap();
    let mut config = Config::from_env();
    config.root_dir = dir.path().to_string_lossy().into();
    config.data_dir = data.to_string_lossy().into();
    config.vm_enabled = true;
    config.vm_mode = "vm".into();
    config.vm_service_executable = Some("plugins/vm/worker".into());
    let mut registry = PluginRegistry::new();
    registry
        .try_register(serde_json::from_str(include_str!("../../plugins/vm/plugin.json")).unwrap())
        .unwrap();
    (dir, db, config, registry)
}

fn task(
    db: &Database,
    registry: &PluginRegistry,
    root: &std::path::Path,
    user: &str,
) -> task_control::TaskGuard {
    let mut ctx = db.load_context(user).unwrap();
    ctx.session_id = "host-session".into();
    ctx.active_state = Some("working".into());
    ctx.settings.path = "/model-chosen-path".into();
    ctx.settings.vm_keyboard_layout = "de".into();
    ctx.settings.vm_screenshot_enabled = false;
    db.save_context(&ctx).unwrap();
    let guard = task_control::begin(user).unwrap();
    task_control::pin_registry(user, registry).unwrap();
    task_control::pin_workspace(user, root).unwrap();
    guard
}

#[cfg(unix)]
#[tokio::test]
async fn vm_process_disabled_package_and_missing_executable_never_spawn_a_worker() {
    let (dir, db, mut config, mut registry) = fixture("echo");
    config.vm_enabled = false;
    super::vm::configure(&db, &config, &mut registry).unwrap();
    super::vm::initialize_service(&config, &registry)
        .await
        .unwrap();
    assert!(registry.service_handle("vm", "vm").is_none());
    assert!(!dir.path().join("worker.ready").exists());

    let (dir, db, mut config, mut registry) = fixture("echo");
    config.vm_service_executable = Some("missing-worker".into());
    assert!(super::vm::configure(&db, &config, &mut registry).is_err());
    assert!(registry.service_handle("vm", "vm").is_none());
    assert!(!dir.path().join("data/vm").exists());

    config.vm_service_executable = Some("plugins/vm/worker".into());
    let mut plugin = registry.get("vm").unwrap().clone();
    plugin.enabled = false;
    registry.register(plugin);
    assert!(super::vm::configure(&db, &config, &mut registry).is_err());
    assert!(!dir.path().join("worker.ready").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn vm_process_binding_is_read_only_until_host_initialization_and_keeps_scoped_identity() {
    let (dir, db, config, mut registry) = fixture("echo");
    super::vm::configure(&db, &config, &mut registry).unwrap();
    assert!(registry.tool_definitions().is_empty());
    assert!(!dir.path().join("worker.ready").exists());
    super::vm::initialize_service(&config, &registry)
        .await
        .unwrap();
    assert_eq!(registry.tool_definitions().len(), 25);
    assert!(super::vm::guest_backend(&registry));
    assert!(super::vm::runtime(&registry).is_none()); // Native screenshot/CLI bridge is not used in process mode.
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
    let user = format!("vm-process-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let sm =
        crate::sm::parse("[state working]\n[action_guards]\n_complete = [rust/build_workspace]")
            .unwrap();
    crate::gateway::action_contracts::bind(&user, "fixture", &sm, dir.path()).unwrap();
    let output = registry
        .execute_tool_with_host(
            &db,
            &user,
            "real-call",
            "vm_shell",
            &json!({"command":"true"}),
            None,
            None,
        )
        .await
        .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();
    let result = &output["result"];
    assert_eq!(result["caller"]["user"], user);
    assert_eq!(result["caller"]["session"], "host-session");
    assert_eq!(result["caller"]["call_id"], "real-call");
    assert_eq!(
        result["caller"]["workspace"],
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        result["caller"]["registry_revision"],
        registry.revision().unwrap()
    );
    assert_eq!(result["preferences"]["keyboard_layout"], "de");
    assert_eq!(result["secret_count"], 0);
    assert_eq!(result["scope"]["kind"], "guest");
    assert_eq!(result["verified"], false);
    assert!(output.get("receipt").is_none());
    assert!(crate::gateway::action_contracts::require(&user, "_complete").is_err());
    assert!(!dir.path().join("data/vm").exists());
    assert!(registry
        .service_handle("vm", "vm")
        .unwrap()
        .disable(Duration::from_secs(2))
        .await
        .unwrap());
    assert!(registry.tool_definitions().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn vm_process_credential_grants_select_only_the_handler_guest() {
    let (dir, db, config, mut registry) = fixture("echo");
    let mut plugin = registry.get("vm").unwrap().clone();
    plugin.secrets = vec!["permitted".into(), "other".into()];
    plugin.context.insert(
        "credential_grants".into(),
        json!({"installer":{"VM_TOKEN":"permitted"}, "wrong":{"OTHER_TOKEN":"other"}}),
    );
    registry.register(plugin);
    super::vm::configure(&db, &config, &mut registry).unwrap();
    super::vm::initialize_service(&config, &registry)
        .await
        .unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_install", true).unwrap();
    let user = format!("vm-process-grants-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let secrets = std::collections::HashMap::from([
        ("permitted".into(), "permitted-fixture-value".into()),
        ("other".into(), "host-only".into()),
    ]);
    let output = registry
        .execute_tool_with_host(
            &db,
            &user,
            "install",
            "vm_install",
            &json!({"vm_name":"installer", "iso_name":"test.iso"}),
            None,
            Some(&secrets),
        )
        .await
        .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output["result"]["grant_names"], json!(["VM_TOKEN"]));
    assert_eq!(output["result"]["credentials_match"], true);
    assert!(!output.to_string().contains("host-only"));
    registry.shutdown_services(Duration::from_secs(2)).await;
}

#[cfg(unix)]
#[tokio::test]
async fn vm_process_bad_setup_and_crash_fail_closed_without_native_or_host_fallback() {
    for mode in ["bad_version", "crash"] {
        let (dir, db, config, mut registry) = fixture(mode);
        super::vm::configure(&db, &config, &mut registry).unwrap();
        let initialized = super::vm::initialize_service(&config, &registry).await;
        if mode == "bad_version" {
            assert!(initialized.is_err());
            assert!(super::vm::initialize_service(&config, &registry)
                .await
                .is_err());
            assert!(!dir.path().join("worker.calls").exists());
            continue;
        }
        initialized.unwrap();
        crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
        let user = format!("vm-process-crash-{}", uuid::Uuid::new_v4());
        let _task = task(&db, &registry, dir.path(), &user);
        for call in ["first", "after-crash"] {
            let output = crate::gateway::tool_dispatch::DispatchContext::new(
                dir.path(),
                &db,
                &user,
                &registry,
                crate::gateway::tool_dispatch::DispatchMode::Agent,
            )
            .execute(&crate::gateway::llm::provider::ToolCall {
                id: call.into(),
                function: crate::gateway::llm::provider::FunctionCall {
                    name: "execute_terminal".into(),
                    arguments: json!({"command":"touch host-sentinel"}).to_string(),
                },
            })
            .await;
            assert!(output.starts_with("Error:"), "{output}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("worker.calls")).unwrap(),
            "invoke\n"
        );
        assert!(registry.tool_definitions().is_empty());
        assert!(!dir.path().join("host-sentinel").exists());
        assert!(super::vm::initialize_service(&config, &registry)
            .await
            .is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn vm_process_forced_disable_cancels_worker_and_does_not_reactivate_old_binding() {
    let (dir, db, config, mut registry) = fixture("cancel");
    super::vm::configure(&db, &config, &mut registry).unwrap();
    super::vm::initialize_service(&config, &registry)
        .await
        .unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
    let user = format!("vm-process-disable-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let input = json!({"command":"true"});
    let call =
        registry.execute_tool_with_host(&db, &user, "cancel", "vm_shell", &input, None, None);
    tokio::pin!(call);
    tokio::select! { result = &mut call => panic!("must block: {result:?}"), _ = async { while !dir.path().join("worker.calls").exists() { tokio::task::yield_now().await; } } => {} }
    let handle = registry.service_handle("vm", "vm").unwrap();
    assert!(!handle.disable(Duration::from_millis(10)).await.unwrap());
    assert!(call.await.is_err());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dir.path().join("worker.cancelled").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!handle.enabled());
    assert!(super::vm::initialize_service(&config, &registry)
        .await
        .is_err());
}
