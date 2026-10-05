//! Manifest-declared process services: binding, identity and fail-closed paths.
use crate::{config::Config, db::Database, gateway::task_control, plugins::PluginRegistry};
use serde_json::{json, Value};
use std::time::Duration;

fn fixture(mode: &str) -> (tempfile::TempDir, Database, Config, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let db = Database::new(&data).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let package = dir.path().join("plugins/probe");
    std::fs::create_dir_all(&package).unwrap();
    let worker = package.join("worker");
    std::fs::write(&worker, include_str!("fixtures/probe_service.py")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.path().join("worker.mode"), mode).unwrap();
    std::fs::write(
        package.join("plugin.json"),
        serde_json::to_string_pretty(&json!({
            "name": "probe",
            "description": "declared service fixture",
            "version": "1",
            "enabled": true,
            "tools": [{
                "name": "probe_echo",
                "description": "fixture",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": false},
                "handler": {
                    "type": "service",
                    "service": "probe",
                    "operation": "probe_echo",
                    "api_version": 1,
                    "timeout_secs": 30,
                    "executable": "worker",
                    "args": ["--stdio"]
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = Config::from_env();
    config.root_dir = dir.path().to_string_lossy().into();
    config.data_dir = data.to_string_lossy().into();
    let registry = crate::plugins::load_all_plugins(&dir.path().join("plugins"));
    assert_eq!(registry.list().len(), 1);
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
    db.save_context(&ctx).unwrap();
    let guard = task_control::begin(user).unwrap();
    task_control::pin_registry(user, registry).unwrap();
    task_control::pin_workspace(user, root).unwrap();
    guard
}

#[cfg(unix)]
#[tokio::test]
async fn declared_service_binds_from_manifest_and_forwards_identity() {
    let (dir, db, config, mut registry) = fixture("echo");
    super::process_service::configure(&config, &mut registry).unwrap();
    // Registration never spawns; the unready binding stays out of the catalog.
    assert!(!dir.path().join("worker.ready").exists());
    assert!(!registry
        .tool_definitions()
        .iter()
        .any(|t| t.function.name == "probe_echo"));
    super::process_service::initialize_service(&config, &registry)
        .await
        .unwrap();
    assert!(dir.path().join("worker.ready").exists());
    assert!(registry
        .tool_definitions()
        .iter()
        .any(|t| t.function.name == "probe_echo"));

    let user = format!("declared-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let output = registry
        .execute_tool_with_host(
            &db,
            &user,
            "call-1",
            "probe_echo",
            &json!({}),
            None,
            None,
        )
        .await
        .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();
    let caller = &output["result"]["caller"];
    assert_eq!(caller["user"], user);
    assert_eq!(caller["session"], "host-session");
    assert_eq!(caller["call_id"], "call-1");
    assert_eq!(caller["owner"], "probe");
    assert_eq!(
        output["result"]["initialization"]["root_dir"].as_str().unwrap(),
        dir.path().canonicalize().unwrap().to_string_lossy()
    );
    registry.shutdown_services(Duration::from_secs(2)).await;
}

#[cfg(unix)]
#[tokio::test]
async fn declared_service_bad_version_fails_before_effects() {
    // Bad handshake version fails initialization and leaves no binding ready.
    let (dir, _db, config, mut registry) = fixture("bad_version");
    super::process_service::configure(&config, &mut registry).unwrap();
    assert!(super::process_service::initialize_service(&config, &registry)
        .await
        .is_err());
    assert!(!dir.path().join("worker.calls").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn declared_service_conflicting_workers_are_rejected() {
    let (dir, _db, config, mut registry) = fixture("echo");
    // A second tool for the same service that points at a different worker.
    let manifest = dir.path().join("plugins/probe/plugin.json");
    let mut data: Value = serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    let mut second = data["tools"][0].clone();
    second["name"] = json!("probe_other");
    second["handler"]["operation"] = json!("probe_other");
    second["handler"]["executable"] = json!("other-worker");
    data["tools"].as_array_mut().unwrap().push(second);
    std::fs::write(&manifest, data.to_string()).unwrap();
    std::fs::write(
        dir.path().join("plugins/probe/other-worker"),
        include_str!("fixtures/probe_service.py"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("plugins/probe/other-worker"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let mut registry = crate::plugins::load_all_plugins(&dir.path().join("plugins"));
    assert!(super::process_service::configure(&config, &mut registry).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn declared_service_crash_fails_closed_without_fallback() {
    let (dir, db, config, mut registry) = fixture("crash");
    super::process_service::configure(&config, &mut registry).unwrap();
    super::process_service::initialize_service(&config, &registry)
        .await
        .unwrap();
    let user = format!("declared-crash-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    for call in ["first", "after-crash"] {
        let output = registry
            .execute_tool_with_host(&db, &user, call, "probe_echo", &json!({}), None, None)
            .await
            .unwrap_or_else(|error| format!("Error: {error}"));
        assert!(output.starts_with("Error:"), "{output}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("worker.calls")).unwrap(),
        "invoke\n"
    );
}
