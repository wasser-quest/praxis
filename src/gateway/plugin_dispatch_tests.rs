//! Run through both real ingress adapters; no provider or VM process is needed.
use crate::{
    db::Database,
    gateway::{
        action_contracts,
        llm::provider::{FunctionCall, ToolCall},
        task_control,
    },
    plugins::PluginRegistry,
};
use serde_json::json;

fn plugin(owner: &str, tool: &str) -> crate::plugins::Plugin {
    serde_json::from_value(json!({
        "name":owner,"description":"fixture","version":"1","tools":[{
            "name":tool,"description":"fixture","parameters":{"type":"object","properties":{},"additionalProperties":false},
            "handler":{"type":"verification"},
            "contract":{"effect":"verification","idempotency":"idempotent","timeout_secs":5,
                "postconditions":[{"program":"/bin/true","resources":["source"]}]}
        }]
    })).unwrap()
}

async fn invoke(
    db: &Database,
    user: &str,
    registry: &PluginRegistry,
    name: &str,
    args: serde_json::Value,
) -> String {
    super::execute_tool_call(
        db,
        user,
        &ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments: args.to_string(),
            },
        },
        registry,
    )
    .await
}

#[tokio::test]
async fn plugin_dispatch_allows_vm_prefix_without_stealing_a_builtin_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    std::fs::write(dir.path().join("source"), "untouched").unwrap();
    let mut registry = PluginRegistry::new();
    registry.register(plugin("fixture", "vm_probe"));
    let user = format!("plugin-dispatch-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    let sm = crate::sm::parse("[state working]\n[action_guards]\n_complete = [fixture/vm_probe]")
        .unwrap();
    action_contracts::bind(&user, "fixture", &sm, dir.path()).unwrap();
    let result = invoke(&db, &user, &registry, "vm_probe", json!({})).await;
    let result: serde_json::Value =
        serde_json::from_str(&result).unwrap_or_else(|error| panic!("{error}: {result}"));
    assert_eq!(result["receipt"]["verified"], true, "{result}");
    assert_eq!(result["receipt"]["action"], "fixture/vm_probe");
    action_contracts::require(&user, "_complete").unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_probe", false).unwrap();
    let disabled = invoke(&db, &user, &registry, "vm_probe", json!({})).await;
    assert!(disabled.contains("disabled"), "{disabled}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source")).unwrap(),
        "untouched"
    );
}

#[tokio::test]
async fn plugin_dispatch_cancelled_task_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let user = format!("cancelled-dispatch-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    task_control::cancel(&user);
    let path = dir.path().join("effect");
    let result = invoke(
        &db,
        &user,
        &PluginRegistry::new(),
        "write_file",
        json!({"path":path,"content":"changed"}),
    )
    .await;
    assert!(result.contains("cancelled"), "{result}");
    assert!(!path.exists());
}

#[tokio::test]
async fn plugin_dispatch_cron_and_background_resources_use_authenticated_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let registry = PluginRegistry::new();
    let alice = format!("alice-{}", uuid::Uuid::new_v4());
    let bob = format!("bob-{}", uuid::Uuid::new_v4());
    let created = invoke(
        &db,
        &alice,
        &registry,
        "cron_add",
        json!({
            "name":"private-job","schedule":"0 0 9 * * *","prompt":"private-prompt","user_id":bob
        }),
    )
    .await;
    assert!(created.starts_with("Cron job created"), "{created}");
    let jobs = db.list_cron_jobs(&alice).unwrap();
    assert_eq!(jobs.len(), 1);
    assert!(db.list_cron_jobs(&bob).unwrap().is_empty());
    let id = &jobs[0].id;
    for name in ["cron_run", "cron_toggle", "cron_delete"] {
        let result = invoke(
            &db,
            &bob,
            &registry,
            name,
            json!({"job_id":id,"enabled":false}),
        )
        .await;
        assert!(result.starts_with("Error"), "{result}");
        assert!(!result.contains("private-prompt"), "{result}");
        assert!(db.get_cron_job(id).unwrap().unwrap().enabled);
    }
    let background = crate::tools::execute_terminal::start_background(
        "printf private-output",
        None,
        Some(&alice),
    )
    .await
    .unwrap();
    let own = invoke(
        &db,
        &alice,
        &registry,
        "background_status",
        json!({"job_id":background}),
    )
    .await;
    assert!(own.contains(&background), "{own}");
    let denied = invoke(
        &db,
        &bob,
        &registry,
        "background_status",
        json!({"job_id":background}),
    )
    .await;
    assert!(denied.starts_with("Unknown job id"), "{denied}");
    let list = invoke(&db, &bob, &registry, "background_status", json!({})).await;
    assert!(!list.contains(&background), "{list}");
}

#[tokio::test]
async fn plugin_dispatch_rechecks_disabled_builtin_before_reading_or_writing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let path = dir.path().join("source");
    std::fs::write(&path, "private-data").unwrap();
    let registry = PluginRegistry::new();
    for name in ["read_file", "write_file"] {
        crate::db::tools::disable(&db, name).unwrap();
        let result = invoke(
            &db,
            "alice",
            &registry,
            name,
            json!({"path":path,"content":"changed"}),
        )
        .await;
        assert!(result.contains("disabled"), "{result}");
        assert!(!result.contains("private-data"));
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), "private-data");
}

#[tokio::test]
async fn plugin_dispatch_advertised_cron_tool_works_in_both_modes() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let registry = PluginRegistry::new();
    assert!(crate::tools::discovery::enabled(
        &db,
        &registry,
        "cron_list"
    ));
    assert_eq!(
        invoke(&db, "alice", &registry, "cron_list", json!({})).await,
        "No cron jobs found."
    );
}

#[test]
fn plugin_catalog_rejects_duplicate_owners_and_builtin_shadowing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    for name in ["probe", "write_file"] {
        let mut registry = PluginRegistry::new();
        registry.register(plugin("one", name));
        if name == "probe" {
            registry.register(plugin("two", name));
        }
        let error = crate::tools::discovery::catalog(&db, &registry).unwrap_err();
        assert!(error.to_string().contains("owner_conflict"), "{error}");
        let sm = crate::sm::parse("[state standard]").unwrap();
        let ctx = db.load_context("alice").unwrap();
        let error =
            crate::gateway::workflow_preflight::validate(&db, &registry, "fixture", &sm, &ctx)
                .unwrap_err();
        assert!(error.to_string().contains("owner_conflict"), "{error}");
    }
}

#[tokio::test]
async fn plugin_dispatch_rejects_shadow_before_builtin_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let path = dir.path().join("effect");
    let mut registry = PluginRegistry::new();
    registry.register(plugin("one", "write_file"));
    let result = invoke(
        &db,
        "alice",
        &registry,
        "write_file",
        json!({"path":path,"content":"changed"}),
    )
    .await;
    assert!(result.contains("owner_conflict"), "{result}");
    assert!(!path.exists());
}

#[tokio::test]
async fn plugin_dispatch_replacement_package_takes_over_builtin_names() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tool-packages");
    let mut registry = PluginRegistry::new();
    for plugin in crate::plugins::load_plugins_from_dir(&examples).unwrap() {
        registry.try_register(plugin).unwrap();
    }
    let catalog = crate::tools::catalog::definitions(&db, &registry).unwrap();
    let terminal: Vec<_> = catalog.iter().filter(|t| t.function.name == "execute_terminal").collect();
    assert_eq!(terminal.len(), 1);
    assert!(terminal[0].function.description.contains("allowlisted"));
    assert!(catalog.iter().any(|t| t.function.name == "run_background"), "unreplaced names stay native");
    let user = format!("takeover-{}", uuid::Uuid::new_v4());
    let marker = dir.path().join("marker");
    let blocked = invoke(&db, &user, &registry, "execute_terminal", json!({"command": format!("touch {}", marker.display())})).await;
    assert!(blocked.contains("not allowlisted"), "{blocked}");
    assert!(!marker.exists(), "the native shell must not run");
    let listed = invoke(&db, &user, &registry, "execute_terminal", json!({"command": format!("ls {}", dir.path().display())})).await;
    assert!(listed.contains("exit 0") && listed.contains("tools.json"), "{listed}");
    // Native package state does not affect the replacement; the plugin's own flags do.
    crate::tools::packages::set(&db.data_dir(), "shell", false).unwrap();
    assert!(crate::tools::catalog::definitions(&db, &registry).unwrap().iter().any(|t| t.function.name == "execute_terminal"));
    crate::db::tools::set_plugin_tool_enabled(&db, "execute_terminal", false).unwrap();
    let off = invoke(&db, &user, &registry, "execute_terminal", json!({"command": "ls"})).await;
    assert!(off.contains("disabled"), "{off}");
}

#[test]
fn plugin_dispatch_replacements_are_validated() {
    let manifest = |name: &str, replaces: &str, tool: &str| -> crate::plugins::Plugin {
        serde_json::from_value(json!({
            "name": name, "description": "fixture", "version": "1", "replaces": [replaces],
            "tools": [{"name": tool, "description": "x", "parameters": {"type": "object"},
                "handler": {"type": "script", "path": "x.py", "interpreter": "python3"}}]
        })).unwrap()
    };
    let mut registry = PluginRegistry::new();
    assert!(registry.try_register(manifest("a", "runtime_control", "agent_next")).is_err());
    assert!(registry.try_register(manifest("b", "file_ops", "write_file")).is_err());
    assert!(registry.try_register(manifest("c", "memory", "execute_terminal")).is_err(), "names outside the replaced package stay native");
    registry.try_register(manifest("d", "memory", "memory_get")).unwrap();
    assert!(registry.try_register(manifest("e", "memory", "memory_set")).is_err(), "one replacement per package");
    assert!(matches!(crate::tools::catalog::owner(&registry, "memory_get").unwrap(), crate::tools::catalog::ToolOwner::Plugin { .. }));
    assert!(matches!(crate::tools::catalog::owner(&registry, "memory_set").unwrap(), crate::tools::catalog::ToolOwner::Builtin));
}
