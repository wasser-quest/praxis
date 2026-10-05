//! Core/IPC integration for the installed shell worker, also compiled without
//! the optional native `praxis-shell` crate. Uses a local Python fixture.
use crate::{config::Config, db::Database, gateway::task_control, plugins::PluginRegistry};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

fn fixture(mode: &str) -> (tempfile::TempDir, Database, Config, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let db = Database::new(&data).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let package = dir.path().join("plugins/shell");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("plugin.json"),
        include_str!("../../packages/shell/plugin.json"),
    )
    .unwrap();
    let worker = package.join("worker");
    std::fs::write(&worker, include_str!("fixtures/shell_service.py")).unwrap();
    // The installed manifest declares a foreground executable too; registration
    // validates declarations without executing it.
    let foreground = package.join("bin/praxis-shell");
    std::fs::write(&foreground, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&worker, &foreground] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    std::fs::write(dir.path().join("worker.mode"), mode).unwrap();
    let mut config = Config::from_env();
    config.root_dir = dir.path().to_string_lossy().into();
    config.data_dir = data.to_string_lossy().into();
    config.shell_service_executable = Some("plugins/shell/worker".into());
    let mut registry = PluginRegistry::new();
    registry
        .try_register(
            serde_json::from_str(include_str!("../../packages/shell/plugin.json")).unwrap(),
        )
        .unwrap();
    (dir, db, config, registry)
}

fn task(
    db: &Database,
    registry: &PluginRegistry,
    root: &Path,
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
async fn shell_process_binding_hands_authenticated_identity_to_the_worker() {
    let (dir, db, config, mut registry) = fixture("echo");
    super::shell::configure(&config, &mut registry).unwrap();
    // Registration alone never spawns the worker.
    assert!(!dir.path().join("worker.ready").exists());
    super::shell::initialize_service(&config, &registry)
        .await
        .unwrap();
    assert!(dir.path().join("worker.ready").exists());
    assert!(registry
        .tool_definitions()
        .iter()
        .any(|t| t.function.name == "run_background"));
    let user = format!("shell-process-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let output = registry
        .execute_tool_with_host(
            &db,
            &user,
            "call-1",
            "run_background",
            &json!({"command":"printf hi"}),
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
    assert_eq!(caller["owner"], "shell");
    assert_eq!(output["result"]["operation"], "run_background");
    registry.shutdown_services(Duration::from_secs(2)).await;
}

#[cfg(unix)]
#[tokio::test]
async fn shell_process_missing_or_disabled_never_spawns_a_worker() {
    let (dir, _db, mut config, mut registry) = fixture("echo");
    config.shell_service_executable = Some("missing-worker".into());
    assert!(super::shell::configure(&config, &mut registry).is_err());
    assert!(!dir.path().join("worker.ready").exists());

    let (dir, _db, config, mut registry) = fixture("echo");
    let mut plugin = registry.get("shell").unwrap().clone();
    plugin.enabled = false;
    registry.register(plugin);
    assert!(super::shell::configure(&config, &mut registry).is_err());
    assert!(!dir.path().join("worker.ready").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn shell_process_bad_version_and_crash_fail_closed_without_fallback() {
    let (dir, _db, config, mut registry) = fixture("bad_version");
    super::shell::configure(&config, &mut registry).unwrap();
    assert!(super::shell::initialize_service(&config, &registry)
        .await
        .is_err());
    assert!(!dir.path().join("worker.calls").exists());

    let (dir, db, config, mut registry) = fixture("crash");
    super::shell::configure(&config, &mut registry).unwrap();
    super::shell::initialize_service(&config, &registry)
        .await
        .unwrap();
    let user = format!("shell-crash-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    for call in ["first", "after-crash"] {
        let output = registry
            .execute_tool_with_host(
                &db,
                &user,
                call,
                "run_background",
                &json!({"command":"touch host-sentinel"}),
                None,
                None,
            )
            .await
            .unwrap_or_else(|error| format!("Error: {error}"));
        assert!(output.starts_with("Error:"), "{output}");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("worker.calls")).unwrap(),
        "invoke\n"
    );
    assert!(!dir.path().join("host-sentinel").exists());
}

/// Exercises the actual separately built package through both ingress modes.
/// Build it first: `cargo build -p praxis-shell` and set
/// `PRAXIS_SHELL_EXECUTABLE` to that binary.
#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires PRAXIS_SHELL_EXECUTABLE pointing to the separately built package"]
async fn installed_shell_package_runs_through_chat_agent_and_worker() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("plugins/shell");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    let source = std::env::var("PRAXIS_SHELL_EXECUTABLE").expect("build praxis-shell first");
    std::fs::copy(source, package.join("bin/praxis-shell")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            package.join("bin/praxis-shell"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    std::fs::copy("packages/shell/plugin.json", package.join("plugin.json")).unwrap();
    let mut registry = crate::plugins::load_all_plugins(&dir.path().join("plugins"));
    assert_eq!(registry.list().len(), 1);
    let data = dir.path().join("data");
    let db = Database::new(&data).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut config = Config::from_env();
    config.root_dir = dir.path().to_string_lossy().into();
    config.data_dir = data.to_string_lossy().into();
    config.shell_service_executable = Some("plugins/shell/bin/praxis-shell".into());
    super::shell::configure(&config, &mut registry).unwrap();
    super::shell::initialize_service(&config, &registry)
        .await
        .unwrap();

    let user = format!("shell-installed-{}", uuid::Uuid::new_v4());
    let _task = task(&db, &registry, dir.path(), &user);
    let call = |name: &str, args: Value| ToolCall {
        id: uuid::Uuid::new_v4().to_string(),
        function: FunctionCall {
            name: name.into(),
            arguments: args.to_string(),
        },
    };
    for mode in [DispatchMode::Chat, DispatchMode::Agent] {
        let output = DispatchContext::new(dir.path(), &db, &user, &registry, mode)
            .execute(&call("execute_terminal", json!({"command":"printf installed-hello"})))
            .await;
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["stdout"], "installed-hello", "{output}");
        assert_eq!(value["exit_code"], 0);
    }
    let started = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent)
        .execute(&call(
            "run_background",
            json!({"command":"printf worker-hello"}),
        ))
        .await;
    assert!(started.contains("Background job started"), "{started}");
    let started_value: Value = serde_json::from_str(&started).unwrap();
    let started_text = started_value["result"].as_str().unwrap_or(&started);
    let job_id = started_text
        .split_whitespace()
        .nth(3)
        .unwrap()
        .trim_end_matches('.')
        .to_string();
    let mut done = false;
    for _ in 0..100 {
        let output = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent)
            .execute(&call("background_status", json!({"job_id": job_id})))
            .await;
        if output.contains("worker-hello") {
            assert!(output.contains("done"), "{output}");
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(done, "installed worker job did not complete");
    let stranger = format!("shell-stranger-{}", uuid::Uuid::new_v4());
    let _stranger_task = task(&db, &registry, dir.path(), &stranger);
    let denied = DispatchContext::new(dir.path(), &db, &stranger, &registry, DispatchMode::Agent)
        .execute(&call("background_status", json!({"job_id": job_id})))
        .await;
    assert!(denied.contains("Unknown job id"), "{denied}");
    task_control::cancel(&user);
    let cancelled = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent)
        .execute(&call("run_background", json!({"command":"printf late"})))
        .await;
    assert!(cancelled.contains("cancelled"), "{cancelled}");
    registry.shutdown_services(Duration::from_secs(2)).await;
}
