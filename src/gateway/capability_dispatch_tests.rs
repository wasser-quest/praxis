// Included by both real gateway dispatchers to exercise their entry points.
#[cfg(all(test, unix))]
mod capability_dispatch_tests {
    use super::execute_tool_call;
    use crate::gateway::{
        action_contracts,
        llm::provider::{FunctionCall, ToolCall},
        task_control,
    };
    use serde_json::json;

    #[tokio::test]
    async fn capability_dispatch_preserves_build_and_requires_verified_action() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source"), "old").unwrap();
        let script = root.path().join("request.sh");
        std::fs::write(&script, "printf '{\"verified\":false}'").unwrap();
        let plugin: crate::plugins::Plugin = serde_json::from_value(json!({
            "name":"fixture","version":"1","description":"fixture","tools":[{
                "name":"verify_source","description":"verify","parameters":{"type":"object","properties":{},"additionalProperties":false},
                "handler":{"type":"script","path":script,"interpreter":"/bin/sh"},
                "contract":{"effect":"verification","idempotency":"idempotent","timeout_secs":5,"postconditions":[{"program":"/bin/true","resources":["source"]}]}
            }]
        })).unwrap();
        let mut plugins = crate::plugins::PluginRegistry::new();
        plugins.register(plugin);
        let user = format!("capability-dispatch-{}", uuid::Uuid::new_v4());
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state done]\n[checks]\nbuild = {\"program\":\"/bin/true\",\"resources\":[\"source\"]}\n[guards]\ndone = [build]\n[action_guards]\ndone = [fixture/verify_source]").unwrap();
        action_contracts::bind(&user, "fixture", &sm, root.path()).unwrap();
        action_contracts::run(&user, "build", &json!({"name":"build"}))
            .await
            .unwrap();
        let call = ToolCall {
            id: "verify".into(),
            function: FunctionCall {
                name: "verify_source".into(),
                arguments: "{}".into(),
            },
        };
        let result = execute_tool_call(&db, &user, &call, &plugins).await;
        let data: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(data["receipt"]["verified"], true);
        action_contracts::require(&user, "done").unwrap();
        assert!(execute_tool_call(&db, &user, &call, &plugins)
            .await
            .contains("Duplicate"));
    }

    #[tokio::test]
    async fn capability_native_dispatch_requires_both_build_and_test_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source"), "old").unwrap();
        let mut registry = crate::plugins::PluginRegistry::new();
        let tools: Vec<_> = ["build_project","run_tests"].into_iter().map(|name| json!({
            "name":name,"description":name,"parameters":{"type":"object","properties":{"scope":{"type":"string","enum":["workspace"]}},"required":["scope"],"additionalProperties":false},
            "handler":{"type":"verification"},
            "contract":{"effect":"verification","idempotency":"idempotent","timeout_secs":5,"postconditions":[{"program":"/bin/true","resources":["source"]}]}
        })).collect();
        registry.register(
            serde_json::from_value(
                json!({"name":"native","description":"native","version":"1","tools":tools}),
            )
            .unwrap(),
        );
        let user = format!("capability-native-dispatch-{}", uuid::Uuid::new_v4());
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state done]\n[action_guards]\ndone = [native/build_project, native/run_tests]\n_complete = [native/build_project, native/run_tests]").unwrap();
        action_contracts::bind(&user, "native", &sm, root.path()).unwrap();
        for name in ["build_project", "run_tests"] {
            assert!(action_contracts::require(&user, "_complete").is_err());
            let call = ToolCall {
                id: name.into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: json!({"scope":"workspace"}).to_string(),
                },
            };
            let result: serde_json::Value =
                serde_json::from_str(&execute_tool_call(&db, &user, &call, &registry).await)
                    .unwrap();
            assert_eq!(result["receipt"]["verified"], true);
        }
        action_contracts::require(&user, "_complete").unwrap();
        std::fs::write(root.path().join("source"), "external").unwrap();
        assert!(action_contracts::require(&user, "done").is_err());
    }
}
