//! VM tool flag compatibility is host policy, independent of the web package.
use super::*;

#[tokio::test]
async fn vm_dashboard_flag_updates_target_the_plugin_override_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut config = crate::config::Config::from_env();
    config.data_dir = dir.path().to_str().unwrap().into();
    config.vm_enabled = true;
    let mut plugins = crate::plugins::PluginRegistry::new();
    plugins
        .try_register(serde_json::from_str(include_str!("../../plugins/vm/plugin.json")).unwrap())
        .unwrap();
    crate::runtime::vm::configure(&db, &config, &mut plugins).unwrap();
    crate::db::tools::enable_vm_compatibility(&db).unwrap();
    set_dashboard_tool_enabled(&db, &plugins, "vm_shell", false).unwrap();
    assert!(!crate::db::tools::get_plugin_tool_enabled(&db, "vm_shell"));
    let state = Arc::new(DashboardState {
        db,
        plugins: Arc::new(plugins),
        gateway_api_key: "fixture-key".into(),
        admin_password: "fixture-password".into(),
    });
    let tools = list_all_tools(State(state)).await.unwrap().0;
    let entries: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "vm_shell")
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["source"], "plugin");
    assert_eq!(entries[0]["is_enabled"], false);
}
