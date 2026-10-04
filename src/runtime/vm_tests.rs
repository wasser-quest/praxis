//! No QEMU or inference required: exercise configuration, ownership and flags.
use crate::{config::Config, db::Database, plugins::PluginRegistry};

fn fixture() -> (tempfile::TempDir, Database, Config, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut config = Config::from_env();
    config.data_dir = dir.path().to_str().unwrap().into();
    config.root_dir = dir.path().to_str().unwrap().into();
    config.vm_enabled = false;
    config.vm_service_executable = None;
    let plugin = serde_json::from_str(include_str!("../../plugins/vm/plugin.json")).unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    (dir, db, config, registry)
}

#[test]
fn vm_disabled_contributes_no_tools_and_creates_no_storage() {
    let (dir, db, config, mut registry) = fixture();
    super::vm::configure(&db, &config, &mut registry).unwrap();
    assert!(super::vm::runtime(&registry).is_none());
    assert!(!registry
        .tool_definitions()
        .iter()
        .any(|t| super::vm::is_vm_tool(&t.function.name)));
    assert!(!dir.path().join("vm").exists());
    assert!(!dir.path().join("shared").exists());
}

#[test]
fn vm_compatibility_activation_is_explicit_and_preserves_other_tool_flags() {
    let (_dir, db, config, mut registry) = fixture();
    crate::db::tools::set_plugin_tool_enabled(&db, "custom_plugin_tool", false).unwrap();
    assert!(!crate::db::tools::get_plugin_tool_enabled(&db, "vm_shell"));
    super::vm::configure(&db, &config, &mut registry).unwrap();
    assert!(!crate::db::tools::get_plugin_tool_enabled(&db, "vm_shell"));
    crate::db::tools::enable_vm_compatibility(&db).unwrap();
    for name in super::vm::TOOL_NAMES {
        assert!(crate::db::tools::get_plugin_tool_enabled(&db, name));
    }
    assert!(!crate::db::tools::get_plugin_tool_enabled(
        &db,
        "custom_plugin_tool"
    ));
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", false).unwrap();
    super::vm::configure(&db, &config, &mut registry).unwrap();
    assert!(!crate::db::tools::get_plugin_tool_enabled(&db, "vm_shell"));
}

#[cfg(feature = "vm")]
#[test]
fn vm_owned_catalog_preserves_legacy_disable_and_explicit_plugin_override() {
    let (dir, db, mut config, mut registry) = fixture();
    config.vm_enabled = true;
    crate::db::tools::set_enabled(&db, "vm_shell", false).unwrap();
    super::vm::configure(&db, &config, &mut registry).unwrap();
    assert!(super::vm::runtime(&registry).is_some());
    assert!(
        matches!(crate::tools::catalog::owner(&registry, "vm_shell").unwrap(), crate::tools::catalog::ToolOwner::Plugin { plugin, .. } if plugin.name == "vm")
    );
    assert!(!crate::tools::catalog::definitions(&db, &registry)
        .unwrap()
        .iter()
        .any(|t| t.function.name == "vm_shell"));
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
    assert_eq!(
        crate::tools::catalog::definitions(&db, &registry)
            .unwrap()
            .iter()
            .filter(|t| t.function.name == "vm_shell")
            .count(),
        1
    );
    assert!(!dir.path().join("vm").exists());
    assert!(!dir.path().join("shared").exists());
}

#[cfg(feature = "vm")]
#[tokio::test]
async fn vm_disabled_binding_removes_hooks_and_guest_alias_fails_closed() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        task_control,
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let (dir, db, mut config, mut registry) = fixture();
    config.vm_enabled = true;
    config.vm_mode = "vm".into();
    super::vm::configure(&db, &config, &mut registry).unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
    let handle = registry.service_handle("vm", "vm").unwrap();
    assert!(handle
        .disable(std::time::Duration::from_secs(1))
        .await
        .unwrap());
    assert!(super::vm::runtime(&registry).is_none());
    let user = format!("vm-disabled-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    task_control::pin_workspace(&user, dir.path()).unwrap();
    task_control::pin_registry(&user, &registry).unwrap();
    let sentinel = dir.path().join("host-sentinel");
    let call = ToolCall {
        id: "no-host-fallback".into(),
        function: FunctionCall {
            name: "execute_terminal".into(),
            arguments: serde_json::json!({"command":format!("touch {}", sentinel.display())})
                .to_string(),
        },
    };
    let result = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent)
        .execute(&call)
        .await;
    assert!(result.starts_with("Error:"), "{result}");
    assert!(!sentinel.exists());
    assert!(!dir.path().join("vm").exists());
}

#[cfg(feature = "vm")]
#[test]
fn vm_manifest_cannot_grant_an_undeclared_credential() {
    let (dir, db, mut config, mut registry) = fixture();
    config.vm_enabled = true;
    let mut plugin = registry.get("vm").unwrap().clone();
    plugin.context.insert(
        "credential_grants".into(),
        serde_json::json!({"praxis-vm":{"VM_TOKEN":"undeclared"}}),
    );
    registry.register(plugin);
    assert!(super::vm::configure(&db, &config, &mut registry)
        .unwrap_err()
        .to_string()
        .contains("undeclared"));
    assert!(!dir.path().join("vm").exists());
}

#[cfg(feature = "vm")]
#[tokio::test]
async fn vm_native_dispatch_is_guest_scoped_and_cannot_satisfy_host_receipts() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        task_control,
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let (dir, db, mut config, mut registry) = fixture();
    config.vm_enabled = true;
    config.vm_mode = "vm".into();
    super::vm::configure(&db, &config, &mut registry).unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "vm_shell", true).unwrap();
    let user = format!("vm-test-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    task_control::pin_workspace(&user, dir.path()).unwrap();
    task_control::pin_registry(&user, &registry).unwrap();
    let call = ToolCall {
        id: "guest-call".into(),
        function: FunctionCall {
            name: "execute_terminal".into(),
            arguments: r#"{"command":"true"}"#.into(),
        },
    };
    let result = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent)
        .execute(&call)
        .await;
    assert!(result.contains("guest"), "{result}");
    assert!(result.contains("failed"), "{result}");
    assert!(result.contains("not found"), "{result}");
    assert!(!result.contains(r#""verified":true"#), "{result}");
    assert!(!dir.path().join("vm").exists());
}

#[cfg(not(feature = "vm"))]
#[test]
fn vm_requested_in_build_without_package_is_an_explicit_setup_error() {
    let (dir, db, mut config, mut registry) = fixture();
    config.vm_enabled = true;
    assert!(super::vm::configure(&db, &config, &mut registry)
        .unwrap_err()
        .to_string()
        .contains("vm"));
    assert!(!dir.path().join("vm").exists());
}
