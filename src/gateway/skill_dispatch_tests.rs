// Shared tests are compiled inside both gateway dispatchers.
use super::execute_tool_call;
use crate::gateway::llm::provider::{FunctionCall, ToolCall};

async fn invoke(db: &crate::db::Database, arguments: serde_json::Value) -> String {
    let call = ToolCall {
        id: "skill-test".into(),
        function: FunctionCall {
            name: "use_skill".into(),
            arguments: arguments.to_string(),
        },
    };
    execute_tool_call(
        db,
        "skill-test-user",
        &call,
        &crate::plugins::PluginRegistry::new(),
    )
    .await
}

#[tokio::test]
async fn use_skill_validates_arguments_and_names() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    for args in [
        serde_json::json!({}),
        serde_json::json!({"name": "debug", "parameters": []}),
        serde_json::json!({"name": "debug", "parameters": {}}),
        serde_json::json!({"name": "debug", "parameters": {"error": 42}}),
        serde_json::json!({"name": "../debug", "parameters": {"error": "test"}}),
    ] {
        let result = invoke(&db, args).await;
        assert!(result.starts_with("Error:"), "{result}");
        assert!(
            !result.contains("Unknown tool"),
            "use_skill was not dispatched: {result}"
        );
    }
}

#[tokio::test]
async fn use_skill_respects_disabled_tool() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    crate::db::tools::set_plugin_tool_enabled(&db, "use_skill", false).unwrap();
    let result = invoke(
        &db,
        serde_json::json!({"name": "debug", "parameters": {"error": "test"}}),
    )
    .await;
    assert!(result.contains("disabled"), "{result}");
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI pointing to Microsoft's JavaScript CLI"]
async fn use_skill_real_poml_dispatch() {
    assert!(
        std::env::var("POML_CLI").is_ok(),
        "Set POML_CLI for this integration test"
    );
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    for (name, key, expected) in [
        ("code_review", "code", "suggested fix"),
        ("debug", "error", "Root Cause"),
        ("tmux", "user_request", "tmux"),
        ("poml_templates", "user_request", "update_template"),
    ] {
        let result = invoke(
            &db,
            serde_json::json!({
                "name": name, "parameters": {key: "Test 日本語 {{literal_not_evaluated}}"}
            }),
        )
        .await;
        assert!(!result.starts_with("Error:"), "{name}: {result}");
        assert!(result.contains(expected), "{name}: {result}");
        assert!(
            result.contains("{{literal_not_evaluated}}"),
            "{name}: {result}"
        );
        assert!(
            result.contains("\n\n# Role\n"),
            "Expected decoded instructions, not raw POML/JSON: {result}"
        );
    }
}
