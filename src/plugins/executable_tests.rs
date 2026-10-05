use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

fn fixture(script: &str, timeout_secs: u64) -> (tempfile::TempDir, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("worker");
    std::fs::write(&path, format!("#!/usr/bin/env python3\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plugin: Plugin = serde_json::from_value(json!({
        "name":"executable_fixture", "description":"fixture", "version":"1",
        "secrets":["allowed"], "tools":[{
            "name":"fixture_echo", "description":"fixture", "parameters":{"type":"object"},
            "handler":{"type":"executable", "path":path, "timeout_secs":timeout_secs}
        }]
    }))
    .unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    (dir, registry)
}

#[tokio::test]
async fn executable_preserves_empty_and_whitespace_results_and_large_stdin() {
    let (_dir, registry) = fixture("import json, sys\nr=json.load(sys.stdin)\nassert r['protocol_version']==1\nassert r['tool']=='fixture_echo'\nprint(json.dumps({'protocol_version':1,'result':r['arguments']['text']}))", 5);
    for text in [
        String::new(),
        "\n  hello 世界  \n".into(),
        "large\n".repeat(60_000),
    ] {
        let result = registry
            .execute_tool("fixture_echo", &json!({"text":text}), None, None)
            .await
            .unwrap();
        assert_eq!(result, text);
    }
}

#[tokio::test]
async fn executable_receives_only_declared_secrets_and_replaces_inherited_envelopes() {
    let (_dir, registry) = fixture("import json, sys\nr=json.load(sys.stdin)\nprint(json.dumps({'protocol_version':1,'result':json.dumps({'context':r['context'],'secrets':r['secrets']})}))", 5);
    let secrets = HashMap::from([
        ("allowed".into(), "granted".into()),
        ("other".into(), "private".into()),
    ]);
    let output = registry
        .execute_tool(
            "fixture_echo",
            &json!({}),
            Some(&json!({"public":true})),
            Some(&secrets),
        )
        .await
        .unwrap();
    let data: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(data["context"], json!({"public":true}));
    assert_eq!(data["secrets"], json!({"allowed":"granted"}));
    let output = registry
        .execute_tool("fixture_echo", &json!({}), None, None)
        .await
        .unwrap();
    let data: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(data, json!({"context":{},"secrets":{}}));
}

#[tokio::test]
async fn executable_rejects_bad_protocol_and_nonzero_exit() {
    for script in [
        "print('{\"protocol_version\":2,\"result\":\"false success\"}')",
        "print('not a response')",
        "import sys\nprint('false success')\nsys.exit(1)",
    ] {
        let (_dir, registry) = fixture(script, 5);
        assert!(registry
            .execute_tool("fixture_echo", &json!({}), None, None)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn executable_timeout_and_cancel_stop_late_effects() {
    let script = "import json, sys, time\nr=json.load(sys.stdin)\ntime.sleep(2)\nopen(r['arguments']['marker'],'w').write('late')\nprint(json.dumps({'protocol_version':1,'result':'late'}))";
    let (dir, registry) = fixture(script, 1);
    let marker = dir.path().join("effect");
    assert!(registry
        .execute_tool("fixture_echo", &json!({"marker":marker}), None, None)
        .await
        .is_err());
    // Dropping the invocation must also kill the owned helper.
    let (_other_dir, other_registry) = fixture(script, 10);
    let args = json!({"marker":marker});
    let pending = other_registry.execute_tool("fixture_echo", &args, None, None);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), pending)
            .await
            .is_err()
    );
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert!(
        !marker.exists(),
        "cancelled helper must not commit a late write"
    );
}

#[tokio::test]
async fn executable_oversized_result_is_an_error_not_silent_truncation() {
    let (_dir, registry) = fixture(
        "import json\nprint(json.dumps({'protocol_version':1,'result':'x'*(8*1024*1024+1)}))",
        5,
    );
    assert!(registry
        .execute_tool("fixture_echo", &json!({}), None, None)
        .await
        .is_err());
}

#[test]
fn executable_loader_rejects_missing_escaped_or_invalid_helpers_before_registration() {
    let (dir, registry) = fixture("print('unused')", 5);
    let plugin = registry.get("executable_fixture").unwrap();
    let manifest = dir.path().join("plugin.json");
    let mut data = serde_json::to_value(plugin).unwrap();
    data["tools"][0]["handler"]["path"] = json!("worker");
    std::fs::write(&manifest, data.to_string()).unwrap();
    assert_eq!(
        load_plugin_from_manifest(&manifest, dir.path())
            .unwrap()
            .name,
        "executable_fixture"
    );
    for path in ["missing", "../outside", "/bin/true"] {
        data["tools"][0]["handler"]["path"] = json!(path);
        std::fs::write(&manifest, data.to_string()).unwrap();
        assert!(
            load_plugin_from_manifest(&manifest, dir.path()).is_err(),
            "{path}"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/bin/true", dir.path().join("escape")).unwrap();
        data["tools"][0]["handler"]["path"] = json!("escape");
        std::fs::write(&manifest, data.to_string()).unwrap();
        assert!(load_plugin_from_manifest(&manifest, dir.path()).is_err());
    }
    data["tools"][0]["handler"]["path"] = json!("worker");
    for timeout in [0, 601] {
        data["tools"][0]["handler"]["timeout_secs"] = json!(timeout);
        std::fs::write(&manifest, data.to_string()).unwrap();
        assert!(load_plugin_from_manifest(&manifest, dir.path()).is_err());
    }
}

#[test]
fn executable_replacement_preserves_flags_and_exposes_the_actual_package_schema() {
    let (dir, mut registry) = fixture("print('unused')", 5);
    let mut plugin = registry.plugins.remove("executable_fixture").unwrap();
    plugin.name = "legacy_fixture".into();
    plugin.replaces = vec!["legacy_file_ops".into()];
    plugin.tools[0].name = "read_file".into();
    plugin.tools[0].parameters =
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]});
    registry.try_register(plugin).unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    crate::db::tools::set_enabled(&db, "read_file", false).unwrap();
    crate::tools::packages::set(dir.path(), "legacy_file_ops", false).unwrap();
    assert!(!crate::tools::catalog::definitions(&db, &registry)
        .unwrap()
        .iter()
        .any(|t| t.function.name == "read_file"));
    let owner = crate::tools::catalog::owner(&registry, "read_file").unwrap();
    assert!(crate::tools::catalog::require_enabled(&db, &owner, "read_file").is_err());
    let mut settings = crate::db::contexts::ContextSettings::default();
    settings.activated_tools = vec!["read_file".into()];
    let definitions = registry.tool_definitions();
    assert!(crate::tools::registry::build_tool_definitions(
        &settings,
        Some(&definitions),
        Some(&db)
    )
    .is_empty());
    crate::db::tools::set_plugin_tool_enabled(&db, "read_file", true).unwrap();
    crate::tools::catalog::require_enabled(&db, &owner, "read_file").unwrap();
    let offered =
        crate::tools::registry::build_tool_definitions(&settings, Some(&definitions), Some(&db));
    assert_eq!(offered.len(), 1);
    assert_eq!(
        offered[0].function.description, "fixture",
        "request must describe the installed owner"
    );
    registry.plugins.get_mut("legacy_fixture").unwrap().enabled = false;
    let catalog = crate::tools::catalog::definitions(&db, &registry).unwrap();
    assert!(
        !catalog.iter().any(|t| t.function.name == "read_file"),
        "disabled plugin must not revive a disabled native package"
    );
}

#[tokio::test]
#[ignore = "Requires PRAXIS_LEGACY_FILE_OPS_EXECUTABLE pointing to the separately built package"]
async fn executable_real_legacy_package_runs_through_chat_agent_and_live_guards() {
    use crate::gateway::{
        llm::provider::{FunctionCall, ToolCall},
        task_control,
        tool_dispatch::{DispatchContext, DispatchMode},
    };
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("plugins/legacy_file_ops");
    std::fs::create_dir_all(package_dir.join("bin")).unwrap();
    let executable = std::env::var("PRAXIS_LEGACY_FILE_OPS_EXECUTABLE")
        .expect("build the package separately first");
    std::fs::copy(executable, package_dir.join("bin/praxis-legacy-file-ops")).unwrap();
    std::fs::copy(
        "packages/legacy_file_ops/plugin.json",
        package_dir.join("plugin.json"),
    )
    .unwrap();
    let registry = load_all_plugins(&dir.path().join("plugins"));
    assert_eq!(registry.list().len(), 1);
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let path = dir.path().join("source");
    let text = format!("\nhello hello\n{}尾部\n", "line\n".repeat(60_000));
    std::fs::write(&path, &text).unwrap();
    let user = format!("external-files-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    task_control::pin_registry(&user, &registry).unwrap();
    let call = |name: &str, args: serde_json::Value| ToolCall {
        id: uuid::Uuid::new_v4().to_string(),
        function: FunctionCall {
            name: name.into(),
            arguments: args.to_string(),
        },
    };
    for mode in [DispatchMode::Chat, DispatchMode::Agent] {
        let result = DispatchContext::new(dir.path(), &db, &user, &registry, mode)
            .execute(&call("read_file", json!({"path":path})))
            .await;
        assert_eq!(result, text);
    }
    let edit = call(
        "edit_file",
        json!({"path":path,"old_string":"hello","new_string":"rust"}),
    );
    let dispatch = DispatchContext::new(dir.path(), &db, &user, &registry, DispatchMode::Agent);
    assert!(dispatch.execute(&edit).await.starts_with("File edited:"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text.replacen("hello", "rust", 1)
    );
    // Raw packaged edits invalidate earlier verification exactly like native
    // edits; their text response cannot satisfy completion guards.
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\",\"resources\":[\"source\"]}\n[guards]\n_complete = [tests]").unwrap();
    crate::gateway::action_contracts::bind(&user, "fixture", &sm, dir.path()).unwrap();
    crate::gateway::action_contracts::run(&user, "before-edit", &json!({"name":"tests"}))
        .await
        .unwrap();
    crate::gateway::action_contracts::require(&user, "_complete").unwrap();
    assert!(dispatch.execute(&edit).await.starts_with("File edited:"));
    assert!(crate::gateway::action_contracts::require(&user, "_complete").is_err());
    let after = std::fs::read_to_string(&path).unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "edit_file", false).unwrap();
    assert!(dispatch.execute(&edit).await.contains("disabled"));
    crate::db::tools::set_plugin_tool_enabled(&db, "edit_file", true).unwrap();
    task_control::cancel(&user);
    assert!(dispatch.execute(&edit).await.contains("cancelled"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
}
