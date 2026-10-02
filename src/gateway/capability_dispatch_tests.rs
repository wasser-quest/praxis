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

    fn ir_fixture() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        crate::db::Database,
        crate::plugins::PluginRegistry,
        String,
        task_control::TaskGuard,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source"), "old").unwrap();
        let mut registry = crate::plugins::PluginRegistry::new();
        let mut tools:Vec<_> = ["build_project", "run_tests"].into_iter().map(|name| json!({
            "name":name,"description":name,"parameters":{"type":"object","properties":{},"additionalProperties":false},
            "handler":{"type":"verification"},
            "contract":{"effect":"verification","idempotency":"idempotent","timeout_secs":5,"postconditions":[{"program":"/bin/true","resources":["source"]}]}
        })).collect();
        tools.push(json!({"name":"modify_source","description":"edit","handler":{"type":"source_edit"},
            "parameters":{"type":"object","properties":{"path":{"type":"string"},"expected_sha256":{"type":"string"},"content":{"type":"string"}},"required":["path","expected_sha256","content"],"additionalProperties":false},
            "contract":{"effect":"workspace_write","idempotency":"non_idempotent","timeout_secs":5,"postconditions":[{"program":"/bin/sh","args":["-c","test \"$(cat source)\" = good"]}]}
        }));
        registry.register(
            serde_json::from_value(
                json!({"name":"native","description":"native","version":"1","tools":tools}),
            )
            .unwrap(),
        );
        let user = format!("ir-dispatch-{}", uuid::Uuid::new_v4());
        db.merge_context(&user,json!({"settings.sm_file":"ir-fixture","settings.activated_tools":["execute_decision","inspect_file","modify_source","build_project","run_tests","agent_complete"]})).unwrap();
        let task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state done]\n[decision_ir]\nR = inspect_file\nM = native/modify_source\nB = native/build_project\nT = native/run_tests\nC = agent_complete\n[action_guards]\n_complete = [native/modify_source, native/build_project, native/run_tests]").unwrap();
        action_contracts::bind(&user, "ir-fixture", &sm, root.path()).unwrap();
        (dir, root, db, registry, user, task)
    }
    fn ir_call(id: &str, ir: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            function: FunctionCall {
                name: "execute_decision".into(),
                arguments: json!({"ir":ir}).to_string(),
            },
        }
    }

    #[tokio::test]
    async fn decision_ir_and_normal_calls_share_rollback_receipts_and_completion_guards() {
        for ir in [false, true] {
            let (_dir, root, db, registry, user, _task) = ir_fixture();
            let invoke = |id: &str, op: &str, name: &str, args: serde_json::Value| {
                if ir {
                    ir_call(id, &format!("1 {op} {args}"))
                } else {
                    ToolCall {
                        id: id.into(),
                        function: FunctionCall {
                            name: name.into(),
                            arguments: args.to_string(),
                        },
                    }
                }
            };
            let early = invoke("early", "C", "agent_complete", json!({}));
            assert!(execute_tool_call(&db, &user, &early, &registry)
                .await
                .contains("requires current verified capabilities"));
            for (id, content, outcome) in
                [("bad", "bad", "rolled_back"), ("edit", "good", "committed")]
            {
                let call = invoke(
                    id,
                    "M",
                    "modify_source",
                    json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":content}),
                );
                let result: serde_json::Value =
                    serde_json::from_str(&execute_tool_call(&db, &user, &call, &registry).await)
                        .unwrap();
                assert_eq!(result["receipt"]["outcome"], outcome);
                assert_eq!(result["receipt"]["call_id"], id);
                assert_eq!(result["receipt"]["action"], "native/modify_source");
            }
            for (op, name) in [("B", "build_project"), ("T", "run_tests")] {
                assert!(action_contracts::require(&user, "_complete").is_err());
                let call = invoke(name, op, name, json!({}));
                let result: serde_json::Value =
                    serde_json::from_str(&execute_tool_call(&db, &user, &call, &registry).await)
                        .unwrap();
                assert_eq!(result["receipt"]["verified"], true);
            }
            action_contracts::require(&user, "_complete").unwrap();
            let replay = ToolCall{id:"edit".into(),function:FunctionCall{name:"modify_source".into(),arguments:json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"good"),"content":"good again"}).to_string()}};
            assert!(execute_tool_call(&db, &user, &replay, &registry)
                .await
                .contains("Duplicate"));
            // A rejected mutating rerun conservatively revokes evidence.
            assert!(action_contracts::require(&user, "_complete").is_err());
            assert_eq!(std::fs::read(root.path().join("source")).unwrap(), b"good");
        }
    }

    #[tokio::test]
    async fn decision_ir_cannot_bypass_state_plugin_or_builtin_disable() {
        let (_dir, root, db, registry, user, _task) = ir_fixture();
        let edit = ir_call(
            "edit",
            &format!(
                "1 M {}",
                json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":"good"})
            ),
        );
        crate::db::tools::set_plugin_tool_enabled(&db, "modify_source", false).unwrap();
        assert!(execute_tool_call(&db, &user, &edit, &registry)
            .await
            .contains("disabled or unavailable"));
        crate::db::tools::set_plugin_tool_enabled(&db, "modify_source", true).unwrap();
        db.merge_context(
            &user,
            json!({"settings.activated_tools":["execute_decision","agent_complete"]}),
        )
        .unwrap();
        assert!(execute_tool_call(&db, &user, &edit, &registry)
            .await
            .contains("disabled or unavailable"));
        assert!(
            execute_tool_call(&db, &user, &ir_call("batch", "1 M {}; 1 C"), &registry)
                .await
                .starts_with("Error:")
        );
        assert!(
            execute_tool_call(&db, &user, &ir_call("undeclared", "1 X"), &registry)
                .await
                .contains("not declared")
        );
        let mut tool = crate::gateway::decision_ir::definition();
        tool.is_enabled = false;
        crate::db::tools::save(&db, &tool).unwrap();
        assert!(
            execute_tool_call(&db, &user, &ir_call("disabled", "1 C"), &registry)
                .await
                .contains("disabled or unavailable")
        );
        assert_eq!(std::fs::read(root.path().join("source")).unwrap(), b"old");
        assert!(!db.load_context(&user).unwrap().settings.done);
    }

    #[tokio::test]
    async fn decision_ir_completion_requires_every_receipt_and_uses_the_same_guard() {
        let (_dir, _root, db, registry, user, _task) = ir_fixture();
        for (id, text) in [
            (
                "edit",
                format!(
                    "1 M {}",
                    json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":"good"})
                ),
            ),
            ("build", "1 B".into()),
            ("tests", "1 T".into()),
        ] {
            assert!(execute_tool_call(
                &db,
                &user,
                &ir_call(&format!("early-{id}"), "1 C"),
                &registry
            )
            .await
            .contains("requires current verified capabilities"));
            let result: serde_json::Value = serde_json::from_str(
                &execute_tool_call(&db, &user, &ir_call(id, &text), &registry).await,
            )
            .unwrap();
            assert_eq!(result["receipt"]["verified"], true);
        }
        assert!(
            execute_tool_call(&db, &user, &ir_call("done", "1 C"), &registry)
                .await
                .contains("Agent signal processed: Complete")
        );
        assert!(db.load_context(&user).unwrap().settings.done);
    }

    #[tokio::test]
    async fn decision_ir_rejects_wrong_owners_and_legacy_uncontracted_targets() {
        let (_dir, _root, db, mut registry, user, task) = ir_fixture();
        drop(task);
        let _task = task_control::begin(&user).unwrap();
        let root = tempfile::tempdir().unwrap();
        let sm = crate::sm::parse(
            "[state working]\n[decision_ir]\nT = wrong/run_tests\nL = legacy/legacy_action",
        )
        .unwrap();
        action_contracts::bind(&user, "ir-fixture", &sm, root.path()).unwrap();
        assert!(
            execute_tool_call(&db, &user, &ir_call("wrong", "1 T"), &registry)
                .await
                .contains("declared enabled contracted capability owner")
        );
        registry.register(serde_json::from_value(json!({"name":"legacy","description":"legacy","version":"1","tools":[{
            "name":"legacy_action","description":"legacy","handler":{"type":"script","path":"never","interpreter":"/bin/sh"},"parameters":{"type":"object","properties":{}}
        }]})).unwrap());
        assert!(
            execute_tool_call(&db, &user, &ir_call("legacy", "1 L"), &registry)
                .await
                .contains("declared enabled contracted capability owner")
        );
    }
}
