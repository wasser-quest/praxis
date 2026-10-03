//! Real gateway and renderer; models are scripted and no paid services run.
use super::*;

#[tokio::test]
async fn workflow_preflight_stops_missing_plugin_before_provider_history_or_source_changes() {
    for flow in ["message", "agent"] {
        let dir = tempfile::tempdir().unwrap();
        let (install, _configured, _configured_user, workspace) =
            crate::gateway::workspace_tests::fixture();
        let (mut state, user, requests) = fixture(dir.path(), vec![]);
        state.config.root_dir = install.path().to_string_lossy().into_owned();
        state.config.workspace_dir = Some(workspace.to_string_lossy().into_owned());
        state.db.merge_context(&user, serde_json::json!({"settings.sm_file":"verified-implementation","active_state":"working","settings.active_state":"working"})).unwrap();
        let before = serde_json::json!(state.db.load_context(&user).unwrap());
        let error = if flow == "message" {
            handle_message(&state, &user, "Implement the project", None)
                .await
                .unwrap_err()
        } else {
            crate::gateway::agent_loop::run_agent_loop(
                &state,
                &user,
                "Implement the project",
                Default::default(),
                None,
            )
            .await
            .unwrap_err()
        };
        let setup = error
            .downcast_ref::<crate::gateway::workflow_preflight::SetupError>()
            .unwrap();
        assert_eq!(setup.code, "plugin_missing");
        assert!(!setup.retryable);
        assert!(setup.operator_action.contains("verified_rust"));
        assert!(
            requests.lock().unwrap().is_empty(),
            "{flow}: provider must not be called"
        );
        assert!(state.db.get_messages(&user, 10).unwrap().is_empty());
        assert_eq!(
            serde_json::json!(state.db.load_context(&user).unwrap()),
            before
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }
}

#[tokio::test]
#[ignore = "Requires real POML; provider is scripted and no learner data/services are used"]
async fn learning_flow_chat_waits_for_reply_then_finishes_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let ir = |id: &str, instruction: &str| {
        call(
            id,
            "execute_decision",
            serde_json::json!({"ir":instruction}),
        )
    };
    let (state, user, requests) = fixture(
        dir.path(),
        vec![
            Step::Reply(reply(
                None,
                vec![ir("lesson", "1 N {\"edge\":0,\"from_state\":\"setup\"}")],
            )),
            Step::Reply(reply(
                None,
                vec![ir("practice", "1 N {\"edge\":0,\"from_state\":\"lesson\"}")],
            )),
            Step::Reply(reply(
                None,
                vec![ir(
                    "too-early",
                    "1 N {\"edge\":0,\"from_state\":\"practice\"}",
                )],
            )),
            Step::Reply(reply(Some("How do you say hello in French?"), vec![])),
            Step::Reply(reply(
                None,
                vec![ir(
                    "feedback",
                    "1 N {\"edge\":0,\"from_state\":\"practice\"}",
                )],
            )),
            Step::Reply(reply(
                None,
                vec![ir(
                    "another-question",
                    "1 N {\"edge\":0,\"from_state\":\"feedback\"}",
                )],
            )),
            Step::Reply(reply(
                Some("Bonjour is correct. How do you say thank you?"),
                vec![],
            )),
            Step::Reply(reply(
                None,
                vec![ir("review", "1 N {\"edge\":1,\"from_state\":\"practice\"}")],
            )),
            Step::Reply(reply(
                None,
                vec![ir("done", "1 N {\"edge\":1,\"from_state\":\"review\"}")],
            )),
            Step::Reply(reply(
                Some("Lesson completed; no progress saved."),
                vec![ir("complete", "1 C")],
            )),
            Step::Reply(reply(Some("Lesson completed; no progress saved."), vec![])),
            Step::Reply(reply(Some("Ready for a new lesson."), vec![])),
        ],
    );
    state.db.merge_context(&user, serde_json::json!({"settings.sm_file":"language-learning","active_state":"setup","settings.active_state":"setup","settings.compaction_enabled":false,"custom_data.language_learning":{"target_language":"French","explanation_language":"English","level":"A1"}})).unwrap();
    let first = handle_message(
        &state,
        &user,
        "Teach me French without saving progress",
        None,
    )
    .await
    .unwrap();
    assert!(first.contains("How do you say hello"));
    let ctx = state.db.load_context(&user).unwrap();
    assert_eq!(ctx.active_state.as_deref(), Some("practice"));
    assert!(!ctx.settings.done);
    let history = state.db.get_messages(&user, 100).unwrap();
    assert!(history
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("too-early"))
        .unwrap()
        .content
        .contains("new user message"));
    let second = handle_message(&state, &user, "bonjour", None)
        .await
        .unwrap();
    assert!(second.contains("Bonjour is correct"));
    assert_eq!(
        state
            .db
            .load_context(&user)
            .unwrap()
            .active_state
            .as_deref(),
        Some("practice")
    );
    let third = handle_message(&state, &user, "Finish the lesson", None)
        .await
        .unwrap();
    assert!(third.contains("Lesson completed"));
    assert!(state.db.load_context(&user).unwrap().settings.done);
    handle_message(&state, &user, "A new lesson", None)
        .await
        .unwrap();
    assert_eq!(
        state
            .db
            .load_context(&user)
            .unwrap()
            .active_state
            .as_deref(),
        Some("setup")
    );
    assert!(crate::db::memory_profiles::list_profiles(&state.db, &user)
        .unwrap()
        .iter()
        .all(|p| p == "standard"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 12);
    assert!(requests[3].messages[0]
        .content
        .as_deref()
        .unwrap()
        .contains("State changed since this input: yes"));
    assert!(requests[4].messages[0]
        .content
        .as_deref()
        .unwrap()
        .contains("State changed since this input: no"));
    assert!(requests[4]
        .messages
        .iter()
        .any(|m| m.role == "user" && m.content.as_deref().unwrap_or("").contains("bonjour")));
    assert!(requests[5].messages[0]
        .content
        .as_deref()
        .unwrap()
        .contains("Feedback on the actual reply"));
}

#[tokio::test]
#[ignore = "Requires real POML, Cargo and rustfmt; model and standalone Rust project are offline fixtures"]
async fn coding_profile_real_workspace_completes_through_chat_and_agent_role_changes() {
    for flow in ["message", "agent"] {
        let install = tempfile::tempdir().unwrap();
        crate::assets::install(install.path(), false, false).unwrap();
        let project = install.path().join("ir-snake");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"normal-coding-fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[workspace]\n").unwrap();
        std::fs::write(
            project.join("Cargo.lock"),
            "version = 4\n[[package]]\nname = \"normal-coding-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
        if flow == "message" {
            let path = install.path().join("contexts/standard-verified.sm");
            let source = std::fs::read_to_string(&path)
                .unwrap()
                .replace("settings.max_llm_turns = 40", "settings.max_llm_turns = 1");
            std::fs::write(path, source).unwrap();
        }
        let content = "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn regression() {\n        assert_eq!(2 + 2, 4);\n    }\n}\n";
        let mut steps = vec![
            Step::Reply(reply(
                None,
                vec![call(
                    "read",
                    "execute_decision",
                    serde_json::json!({"ir":"1 R {\"path\":\"src/main.rs\"}"}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "edit",
                    "execute_decision",
                    serde_json::json!({"ir":format!("1 M {}",serde_json::json!({"path":"src/main.rs","expected_sha256":crate::tools::apply_patch::hash(b"fn main() {}\n"),"content":content}))}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "review-role",
                    "set_context",
                    serde_json::json!({"key":"sm_data.role","value":"review"}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "build",
                    "execute_decision",
                    serde_json::json!({"ir":"1 B {\"scope\":\"workspace\"}"}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "tests",
                    "execute_decision",
                    serde_json::json!({"ir":"1 T {\"scope\":\"workspace\"}"}),
                )],
            )),
            Step::Reply(reply(
                Some("Verified normal coding completed."),
                vec![call(
                    "complete",
                    "execute_decision",
                    serde_json::json!({"ir":"1 C"}),
                )],
            )),
        ];
        if flow == "message" {
            steps.push(Step::Reply(reply(
                Some("Verified normal coding completed."),
                vec![],
            )));
        }
        let (mut state, user, requests) = fixture(&install.path().join("data"), steps);
        state.config.root_dir = install.path().to_string_lossy().into_owned();
        state.config.workspace_dir = Some("ir-snake".into());
        state.plugins = Arc::new(crate::plugins::load_all_plugins(std::path::Path::new(
            "examples/plugins",
        )));
        state.db.merge_context(&user, serde_json::json!({"settings.sm_file":"standard-verified","settings.system_template":null,"sm_data.role":"code","settings.use_decision_router":false,"settings.compaction_enabled":false})).unwrap();
        let response = if flow == "message" {
            handle_message(
                &state,
                &user,
                "Implement and verify the prepared project",
                None,
            )
            .await
            .unwrap()
        } else {
            crate::gateway::agent_loop::run_agent_loop(
                &state,
                &user,
                "Implement and verify the prepared project",
                Default::default(),
                None,
            )
            .await
            .unwrap()
            .response
        };
        assert!(
            response.contains("Verified normal coding completed"),
            "{flow}: {response}"
        );
        assert!(state.db.load_context(&user).unwrap().settings.done);
        assert_eq!(
            state
                .db
                .load_context(&user)
                .unwrap()
                .active_state
                .as_deref(),
            Some("review")
        );
        assert_eq!(
            std::fs::read_to_string(project.join("src/main.rs")).unwrap(),
            content
        );
        let requests = requests.lock().unwrap();
        assert!(requests[0].messages[0]
            .content
            .as_deref()
            .unwrap()
            .contains("M=verified_rust/modify_source"));
        assert!(!requests[3].messages[0]
            .content
            .as_deref()
            .unwrap()
            .contains("M=verified_rust/modify_source"));
        for (turn, id) in [(2, "edit"), (4, "build"), (5, "tests")] {
            let result = requests[turn]
                .messages
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some(id))
                .unwrap();
            let receipt: serde_json::Value =
                serde_json::from_str(result.content.as_deref().unwrap()).unwrap();
            assert_eq!(receipt["receipt"]["verified"], true, "{flow}: {id}");
            assert_eq!(receipt["receipt"]["outcome"], "committed");
        }
    }
}
