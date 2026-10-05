use super::*;
use crate::{db::tools, plugins::PluginRegistry};

const VERIFIED_TOOLS: [&str; 3] = ["modify_source", "build_workspace", "run_workspace_tests"];

fn verified_plugins(dir: &std::path::Path) -> PluginRegistry {
    let plugin_dir = dir.join("verified-rust");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        include_str!("../../examples/plugins/verified-rust/plugin.json"),
    )
    .unwrap();
    let plugins = crate::plugins::load_all_plugins(dir);
    assert_eq!(plugins.get("verified_rust").unwrap().version, "1.2.1");
    plugins
}

#[test]
fn backend_tool_toggle_verified_rust_persists_and_updates_discovery() {
    let data = tempfile::tempdir().unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(data.path()).unwrap();
    tools::init_default_tools(&db).unwrap();
    let plugins = verified_plugins(plugin_dir.path());
    tools::set_plugin_tool_enabled(&db, "unrelated_plugin_tool", false).unwrap();
    let mut settings = crate::db::contexts::ContextSettings::default();
    settings.activated_tools = VERIFIED_TOOLS.into_iter().map(str::to_owned).collect();

    for enabled in [false, true] {
        for name in VERIFIED_TOOLS {
            set_dashboard_tool_enabled(&db, &plugins, name, enabled).unwrap();
            assert_eq!(tools::get_plugin_tool_enabled(&db, name), enabled, "{name}");
            assert_eq!(
                crate::tools::discovery::enabled(&db, &plugins, name),
                enabled
            );
            let offered = crate::tools::registry::build_tool_definitions(
                &settings,
                Some(&plugins.tool_definitions()),
                Some(&db),
            );
            assert_eq!(
                offered.iter().any(|tool| tool.function.name == name),
                enabled
            );
        }
        let reopened = crate::db::Database::new(data.path()).unwrap();
        for name in VERIFIED_TOOLS {
            assert_eq!(tools::get_plugin_tool_enabled(&reopened, name), enabled);
        }
        assert!(!tools::get_plugin_tool_enabled(
            &reopened,
            "unrelated_plugin_tool"
        ));
    }

    settings.activated_tools = vec!["execute_decision".into()];
    let offered = crate::tools::registry::build_tool_definitions(
        &settings,
        Some(&plugins.tool_definitions()),
        Some(&db),
    );
    assert!(offered
        .iter()
        .any(|tool| tool.function.name == "execute_decision"));
    assert!(offered
        .iter()
        .all(|tool| !VERIFIED_TOOLS.contains(&tool.function.name.as_str())));
}

#[test]
fn backend_tool_toggle_builtin_ownership_is_preserved() {
    let data = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(data.path()).unwrap();
    tools::init_default_tools(&db).unwrap();
    let mut plugins = PluginRegistry::new();
    assert!(plugins.try_register(crate::plugins::Plugin {
        name: "shadow".into(),
        description: "Synthetic name collision".into(),
        version: "1.0.0".into(),
        tools: vec![crate::plugins::PluginTool {
            name: "execute_decision".into(),
            description: "Synthetic tool".into(),
            parameters: serde_json::json!({"type":"object"}),
            handler: crate::plugins::PluginHandler::Builtin {
                name: "read_file".into(),
            },
            contract: None,
        }],
        context: HashMap::new(),
        secrets: vec![],
        enabled: true,
        replaces: Vec::new(),
        hooks: Default::default(),
        requires: Default::default(),
        frontend: None,
    }).is_err());
    assert!(plugins.list().is_empty());
    for enabled in [false, true] {
        set_dashboard_tool_enabled(&db, &plugins, "execute_decision", enabled).unwrap();
        assert_eq!(
            tools::get(&db, "execute_decision").unwrap().is_enabled,
            enabled
        );
        assert!(!tools::list_plugin_tools(&db)
            .unwrap()
            .contains_key("execute_decision"));
        assert_eq!(
            crate::tools::discovery::enabled(&db, &plugins, "execute_decision"),
            enabled
        );
    }
}

#[test]
fn backend_tool_toggle_rejects_unknown_or_manifest_disabled_tools() {
    let data = tempfile::tempdir().unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(data.path()).unwrap();
    tools::init_default_tools(&db).unwrap();
    let _ = verified_plugins(plugin_dir.path());
    let manifest_path = plugin_dir.path().join("verified-rust/plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["enabled"] = serde_json::json!(false);
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let plugins = crate::plugins::load_all_plugins(plugin_dir.path());
    for name in ["unknown_tool", "modify_source"] {
        assert_eq!(
            set_dashboard_tool_enabled(&db, &plugins, name, true),
            Err(StatusCode::NOT_FOUND)
        );
    }
    assert!(tools::list_plugin_tools(&db).unwrap().is_empty());
}

#[test]
fn backend_tool_toggle_reports_plugin_flag_write_errors() {
    let data = tempfile::tempdir().unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(data.path()).unwrap();
    tools::init_default_tools(&db).unwrap();
    let plugins = verified_plugins(plugin_dir.path());
    let flags = data.path().join("plugin_tools.json");
    std::fs::write(&flags, "malformed").unwrap();
    assert_eq!(
        set_dashboard_tool_enabled(&db, &plugins, "modify_source", true),
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    );
    assert_eq!(std::fs::read_to_string(flags).unwrap(), "malformed");
}
