use super::*;
use crate::gateway::{action_contracts, task_control};
use serde_json::{json, Value};

fn check(program: &str, args: &[&str]) -> action_contracts::CheckContract {
    action_contracts::CheckContract {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: ".".into(),
        timeout_secs: 5,
        resources: vec!["source".into()],
    }
}
fn fixture(handler: &str) -> (tempfile::TempDir, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source"), "old").unwrap();
    std::fs::write(dir.path().join("run.sh"), handler).unwrap();
    let tool: PluginTool = serde_json::from_value(json!({
        "name":"act", "description":"act", "parameters":{"type":"object","properties":{},"additionalProperties":false},
        "handler":{"type":"script","interpreter":"/bin/sh","path":dir.path().join("run.sh")},
        "contract":{"effect":"workspace_write","idempotency":"non_idempotent","timeout_secs":5,
            "postconditions":[{"program":"/bin/true","resources":["source"]}]}
    })).unwrap();
    let mut registry = PluginRegistry::new();
    registry.register(Plugin {
        name: "sample".into(),
        description: "sample".into(),
        version: "1".into(),
        tools: vec![tool],
        context: HashMap::new(),
        secrets: vec!["allowed".into()],
        enabled: true,
    });
    (dir, registry)
}
fn bind(user: &str, root: &Path) -> task_control::TaskGuard {
    let guard = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[state done]\n[action_guards]\ndone = [sample/act]\n_complete = [sample/act]").unwrap();
    action_contracts::bind(user, "fixture", &sm, root).unwrap();
    guard
}
async fn invoke(registry: &PluginRegistry, user: &str, call: &str) -> Value {
    serde_json::from_str(
        &registry
            .execute_tool_for_task(user, call, "act", &json!({}), None, None)
            .await
            .unwrap(),
    )
    .unwrap()
}
fn contract(registry: &mut PluginRegistry) -> &mut contracts::ActionContract {
    registry.plugins.get_mut("sample").unwrap().tools[0]
        .contract
        .as_mut()
        .unwrap()
}
fn compensate(registry: &mut PluginRegistry, dir: &Path, check_program: &str) {
    std::fs::write(dir.join("undo.sh"), "printf old > source").unwrap();
    contract(registry).compensation = Some(contracts::Compensation {
        handler: PluginHandler::Script {
            path: dir.join("undo.sh").to_string_lossy().into_owned(),
            interpreter: "/bin/sh".into(),
        },
        postconditions: vec![check(check_program, &[])],
        timeout_secs: 5,
    });
}

#[tokio::test]
async fn capability_commits_independent_evidence_and_rejects_replay() {
    let (dir, registry) = fixture("printf new > source; printf '{\"verified\":false}'");
    let user = "capability-commit";
    let _task = bind(user, dir.path());
    assert!(action_contracts::require(user, "done").is_err());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "committed");
    assert_eq!(result["receipt"]["verified"], true);
    assert_eq!(result["result"]["verified"], false);
    action_contracts::require(user, "done").unwrap();
    assert!(registry
        .execute_tool_for_task(user, "first", "act", &json!({}), None, None)
        .await
        .is_err());
    std::fs::write(dir.path().join("source"), "external").unwrap();
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn capability_claim_cannot_replace_check_and_compensation_is_not_success() {
    let (dir, mut registry) = fixture("printf new > source; printf '{\"verified\":true}'");
    contract(&mut registry).postconditions = vec![check("/bin/false", &[])];
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-compensated";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["verified"], false);
    assert_eq!(result["receipt"]["outcome"], "compensated");
    assert_eq!(result["receipt"]["compensation_verified"], true);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source")).unwrap(),
        "old"
    );
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn capability_precondition_prevents_handler_and_compensation() {
    let (dir, mut registry) = fixture("touch attempted");
    contract(&mut registry).preconditions = vec![check("/bin/false", &[])];
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-pre";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "precondition_failed");
    assert_eq!(result["receipt"]["attempted"], false);
    assert_eq!(result["receipt"]["compensation_attempted"], false);
    assert!(!dir.path().join("attempted").exists());
}

#[tokio::test]
async fn capability_failed_compensation_remains_incomplete() {
    let (dir, mut registry) = fixture("printf new > source");
    contract(&mut registry).postconditions = vec![check("/bin/false", &[])];
    compensate(&mut registry, dir.path(), "/bin/false");
    let user = "capability-cleanup-failed";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "compensation_failed");
    assert_eq!(result["receipt"]["compensation_verified"], false);
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn capability_failure_without_compensation_is_explicit() {
    let (dir, registry) = fixture("printf private-diagnostic >&2; exit 1");
    let user = "capability-no-cleanup";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "failed");
    assert_eq!(result["receipt"]["attempted"], true);
    assert!(!result.to_string().contains("private-diagnostic"));
}

#[tokio::test]
async fn capability_timeout_kills_foreground_before_compensation() {
    let (dir, mut registry) = fixture("printf new > source; sleep 2; printf escaped > source");
    contract(&mut registry).timeout_secs = 1;
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-timeout";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["failure"], "timed_out");
    assert_eq!(result["receipt"]["outcome"], "compensated");
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source")).unwrap(),
        "old"
    );
}

#[tokio::test]
async fn capability_cancel_still_runs_compensation_with_fresh_token() {
    let (dir, mut registry) = fixture("printf new > source; touch started; sleep 20");
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-cancel";
    let _task = bind(user, dir.path());
    let execution = invoke(&registry, user, "first");
    let cancellation = async {
        for _ in 0..100 {
            if dir.path().join("started").exists() {
                task_control::cancel(user);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("handler did not start");
    };
    let (result, _) = tokio::join!(execution, cancellation);
    assert_eq!(result["receipt"]["failure"], "cancelled");
    assert_eq!(result["receipt"]["compensation_verified"], true);
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn capability_later_condition_cannot_leave_earlier_scope_stale() {
    let (dir, mut registry) = fixture("true");
    contract(&mut registry).postconditions = vec![
        check("/bin/true", &[]),
        check("/bin/sh", &["-c", "printf new > source"]),
    ];
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-stale";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["verified"], false);
    assert_eq!(result["receipt"]["outcome"], "compensated");
}

#[tokio::test]
async fn capability_task_identity_definition_and_canonical_root_are_pinned() {
    let (dir, mut registry) = fixture("true");
    let user = "capability-pinned";
    let task = bind(user, dir.path());
    invoke(&registry, user, "first").await;
    registry.plugins.get_mut("sample").unwrap().version = "2".into();
    assert!(registry
        .execute_tool_for_task(user, "second", "act", &json!({}), None, None)
        .await
        .is_err());
    let sm = crate::sm::parse(
        "[state done]\n[action_guards]\ndone = [sample/act]\n_complete = [sample/act]",
    )
    .unwrap();
    let other = tempfile::tempdir().unwrap();
    assert!(action_contracts::bind(user, "fixture", &sm, other.path()).is_err());
    drop(task);
    let _next = bind(user, dir.path());
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn capability_cannot_execute_without_task_or_through_legacy_api() {
    let (_dir, registry) = fixture("true");
    assert!(registry
        .execute_tool("act", &json!({}), None, None)
        .await
        .is_err());
    assert!(registry
        .execute_tool_for_task("no-task", "call", "act", &json!({}), None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn capability_checks_input_before_effect_and_limits_schema() {
    let (dir, mut registry) = fixture("touch attempted");
    registry.plugins.get_mut("sample").unwrap().tools[0].parameters = json!({"type":"object","properties":{"scope":{"type":"string","enum":["workspace"]},"count":{"type":"integer","minimum":1,"maximum":3}},"required":["scope"],"additionalProperties":false});
    let user = "capability-input";
    let _task = bind(user, dir.path());
    for input in [
        json!({}),
        json!({"scope":"all"}),
        json!({"scope":"workspace","count":4}),
        json!({"scope":"workspace","count":1.5}),
        json!({"scope":"workspace","contract":{}}),
    ] {
        assert!(registry
            .execute_tool_for_task(user, "first", "act", &input, None, None)
            .await
            .is_err());
    }
    assert!(!dir.path().join("attempted").exists());
    let mut definition = registry.plugins["sample"].tools[0].clone();
    definition.parameters["properties"]["scope"]["pattern"] = json!(".*");
    assert!(contracts::validate_tool("sample", &definition).is_err());
    definition.parameters = json!({"type":"object","properties":{"nested":{"type":"object"}},"additionalProperties":false});
    assert!(contracts::validate_tool("sample", &definition).is_err());
}

#[tokio::test]
async fn capability_verification_preserves_named_evidence_but_write_invalidates() {
    let (dir, mut registry) = fixture("true");
    contract(&mut registry).effect = contracts::EffectClass::Verification;
    let user = "capability-preserves";
    let _task = task_control::begin(user).unwrap();
    let mut sm = crate::sm::parse("[state done]\n[checks]\nbuild = {\"program\":\"/bin/true\",\"resources\":[\"source\"]}\n[guards]\ndone = [build]\n[action_guards]\ndone = [sample/act]").unwrap();
    action_contracts::bind(user, "fixture", &sm, dir.path()).unwrap();
    action_contracts::run(user, "build", &json!({"name":"build"}))
        .await
        .unwrap();
    invoke(&registry, user, "first").await;
    action_contracts::require(user, "done").unwrap();
    // A cooperating write invalidates another task even when bytes are unchanged.
    let writer = "capability-cooperating-writer";
    let _writer_task = bind(writer, dir.path());
    contract(&mut registry).effect = contracts::EffectClass::WorkspaceWrite;
    invoke(&registry, writer, "write").await;
    assert!(action_contracts::require(user, "done").is_err());
    action_contracts::before_tool(user, "execute_terminal").unwrap();
    assert!(action_contracts::require(user, "done").is_err());
    sm.action_guards.clear();
    assert!(action_contracts::bind(user, "fixture", &sm, dir.path()).is_err());
}

#[tokio::test]
async fn capability_failed_verifier_has_bounded_diagnostic_output() {
    let (dir, mut registry) = fixture("true");
    contract(&mut registry).postconditions = vec![check(
        "/bin/sh",
        &[
            "-c",
            "head -c 70000 /dev/zero | tr '\\000' x; printf test-failed >&2; exit 1",
        ],
    )];
    let user = "capability-output";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    let output = &result["receipt"]["conditions"][0]["output"];
    assert_eq!(output["stderr"], "test-failed");
    assert_eq!(output["stdout"].as_str().unwrap().len(), 65536);
    assert_eq!(output["stdout_truncated"], true);
}

#[test]
fn capability_manifest_guard_validation_and_builtin_shadowing() {
    for text in [
        "[action_guards]\n_complete = []",
        "[action_guards]\nunknown = [sample/act]",
        "[state done]\n[action_guards]\ndone = [sample/act, sample/act]",
        "[action_guards]\n_complete = [claim]",
        "[action_guards]\n_complete = [a/b/c]",
    ] {
        assert!(crate::sm::parse(text).is_err(), "{text}");
    }
    let (_dir, mut registry) = fixture("true");
    registry.plugins.get_mut("sample").unwrap().tools[0].name = "execute_terminal".into();
    assert!(!registry.manages_contract("execute_terminal"));
    let mut tool = registry.plugins["sample"].tools[0].clone();
    tool.contract.as_mut().unwrap().postconditions.clear();
    assert!(contracts::validate_tool("sample", &tool).is_err());
}

#[tokio::test]
async fn capability_scoped_secrets_and_context_are_replaced() {
    let (dir, registry) = fixture("printf '%s' \"$PLUGIN_SECRETS\"");
    let user = "capability-secrets";
    let _task = bind(user, dir.path());
    let secrets = HashMap::from([
        ("allowed".into(), "yes".into()),
        ("other".into(), "private".into()),
    ]);
    let result: Value = serde_json::from_str(
        &registry
            .execute_tool_for_task(user, "first", "act", &json!({}), None, Some(&secrets))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["result"], json!({"allowed":"yes"}));
}

#[test]
fn capability_relative_install_paths_are_absolute_for_workspace_cwd() {
    let dir = tempfile::tempdir_in(".").unwrap();
    let folder = dir.path().join("plugin");
    std::fs::create_dir(&folder).unwrap();
    let (_, registry) = fixture("true");
    let mut plugin = registry.plugins["sample"].clone();
    plugin.tools[0].handler = PluginHandler::Script {
        path: "run.sh".into(),
        interpreter: "/bin/sh".into(),
    };
    std::fs::write(
        folder.join("plugin.json"),
        serde_json::to_vec(&plugin).unwrap(),
    )
    .unwrap();
    let plugins = load_plugins_from_dir(dir.path()).unwrap();
    let PluginHandler::Script { path, .. } = &plugins[0].tools[0].handler else {
        panic!()
    };
    assert!(Path::new(path).is_absolute());
}

#[test]
fn capability_bundled_semantic_example_has_checks_and_receipt_guards() {
    let registry = load_all_plugins(Path::new("examples/plugins"));
    let plugin = registry.get("verified_rust").unwrap();
    assert_eq!(plugin.tools.len(), 3);
    for tool in &plugin.tools {
        contracts::validate_tool(&plugin.name, tool).unwrap();
        assert_eq!(
            serde_json::to_value(&tool.handler).unwrap(),
            if tool.name == "modify_source" { json!({"type":"source_edit"}) } else { json!({"type":"verification"}) }
        );
    }
    let sm =
        crate::sm::parse(&std::fs::read_to_string("contexts/verified-capabilities.sm").unwrap())
            .unwrap();
    assert!(sm.checks.contains_key("build")); // native transactional edits retain their verifier
    assert!(sm.guards.is_empty());
    assert_eq!(
        sm.action_guards["_complete"],
        vec![
            "verified_rust/build_workspace",
            "verified_rust/run_workspace_tests"
        ]
    );
    let tools = &sm.states["working"].variables["settings.activated_tools"];
    let tools: Vec<String> = serde_json::from_str(tools).unwrap();
    assert!(
        tools.contains(&"build_workspace".into()) && tools.contains(&"run_workspace_tests".into())
    );
    assert!(!tools.contains(&"run_check".into()) && !tools.contains(&"execute_terminal".into()));
}

fn native_fixture() -> (tempfile::TempDir, PluginRegistry) {
    let (dir, mut registry) = fixture("exit 99");
    registry.plugins.get_mut("sample").unwrap().tools[0].handler =
        serde_json::from_value(json!({"type":"verification"})).unwrap();
    contract(&mut registry).effect = contracts::EffectClass::Verification;
    (dir, registry)
}

#[tokio::test]
async fn capability_native_verification_executes_checks_once_without_script() {
    let (dir, mut registry) = native_fixture();
    // Leave the original failing script in place: the adapter cannot depend on it.
    contract(&mut registry).postconditions = vec![check(
        "/bin/sh",
        &["-c", "printf ran >> executions; printf build-passed"],
    )];
    let user = "capability-native-pass";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "committed");
    assert_eq!(result["receipt"]["verified"], true);
    assert_eq!(
        result["receipt"]["conditions"][0]["output"]["stdout"],
        "build-passed"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("executions")).unwrap(),
        "ran"
    );
    action_contracts::require(user, "done").unwrap();
    assert!(registry
        .execute_tool_for_task(user, "first", "act", &json!({}), None, None)
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("executions")).unwrap(),
        "ran"
    );
}

#[tokio::test]
async fn capability_native_failed_check_cannot_keep_previous_success() {
    let (dir, mut registry) = native_fixture();
    contract(&mut registry).postconditions = vec![check(
        "/bin/sh",
        &[
            "-c",
            "if test -e executions; then printf tests-failed >&2; exit 1; fi; touch executions",
        ],
    )];
    let user = "capability-native-rerun";
    let _task = bind(user, dir.path());
    assert_eq!(
        invoke(&registry, user, "first").await["receipt"]["verified"],
        true
    );
    action_contracts::require(user, "done").unwrap();
    let failed = invoke(&registry, user, "second").await;
    assert_eq!(failed["receipt"]["outcome"], "failed");
    assert_eq!(failed["receipt"]["verified"], false);
    assert_eq!(
        failed["receipt"]["conditions"][0]["output"]["stderr"],
        "tests-failed"
    );
    assert!(action_contracts::require(user, "done").is_err());
}

#[test]
fn capability_native_adapter_requires_verification_contract_and_strict_handler() {
    assert!(serde_json::from_value::<PluginHandler>(
        json!({"type":"verification","program":"model-command"})
    )
    .is_err());
    let (_dir, mut registry) = native_fixture();
    let mut tool = registry.plugins["sample"].tools[0].clone();
    tool.contract = None;
    assert!(contracts::validate_tool("sample", &tool).is_err());
    for effect in [
        contracts::EffectClass::ReadOnly,
        contracts::EffectClass::WorkspaceWrite,
        contracts::EffectClass::ExternalWrite,
    ] {
        contract(&mut registry).effect = effect;
        assert!(contracts::validate_tool("sample", &registry.plugins["sample"].tools[0]).is_err());
    }
    // A read/verification adapter cannot stand in for restoring a mutation.
    registry.plugins.get_mut("sample").unwrap().tools[0].handler = PluginHandler::Script {
        path: _dir.path().join("run.sh").to_string_lossy().into_owned(),
        interpreter: "/bin/sh".into(),
    };
    contract(&mut registry).compensation = Some(contracts::Compensation {
        handler: serde_json::from_value(json!({"type":"verification"})).unwrap(),
        postconditions: vec![check("/bin/true", &[])],
        timeout_secs: 5,
    });
    assert!(
        contracts::validate_tool("sample", &registry.plugins["sample"].tools[0])
            .unwrap_err()
            .to_string()
            .contains("cannot perform compensation")
    );
}

#[tokio::test]
async fn capability_native_model_cannot_override_verifier_or_forge_receipt() {
    let (dir, mut registry) = native_fixture();
    contract(&mut registry).postconditions = vec![check("/bin/sh", &["-c", "touch executed"])];
    let user = "capability-native-input";
    let _task = bind(user, dir.path());
    for args in [
        json!({"program":"/bin/true"}),
        json!({"receipt":{"verified":true}}),
        json!({"contract":{"postconditions":[]}}),
    ] {
        assert!(registry
            .execute_tool_for_task(user, "first", "act", &args, None, None)
            .await
            .is_err());
    }
    assert!(!dir.path().join("executed").exists());
    assert!(action_contracts::require(user, "done").is_err());
    assert!(registry
        .execute_tool("act", &json!({}), None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn capability_native_timeout_and_cancel_never_produce_proof() {
    for cancelled in [false, true] {
        let (dir, mut registry) = native_fixture();
        contract(&mut registry).timeout_secs = 1;
        contract(&mut registry).postconditions =
            vec![check("/bin/sh", &["-c", "touch started; sleep 20"])];
        let user = if cancelled {
            "capability-native-cancel"
        } else {
            "capability-native-timeout"
        };
        let _task = bind(user, dir.path());
        let token = task_control::cancellation(user).unwrap();
        let run = invoke(&registry, user, "first");
        let cancel = async {
            if cancelled {
                for _ in 0..100 {
                    if dir.path().join("started").exists() {
                        token.cancel();
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                panic!("verifier did not start");
            }
        };
        let (receipt, _) = tokio::join!(run, cancel);
        assert_eq!(receipt["receipt"]["verified"], false);
        assert_eq!(
            receipt["receipt"]["failure"],
            if cancelled { "cancelled" } else { "timed_out" }
        );
        assert!(action_contracts::require(user, "done").is_err());
    }
}

#[tokio::test]
#[ignore = "Requires a Rust toolchain on PATH; compiles and tests a temporary, dependency-free project"]
async fn capability_native_bundled_build_and_tests_with_real_cargo() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"praxis-capability-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
    std::fs::write(
        root.path().join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"praxis-capability-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let source = root.path().join("src/lib.rs");
    let good = "#[test] fn regression() { assert_eq!(2 + 2, 4); }\n";
    std::fs::write(&source, good).unwrap();
    let registry = load_all_plugins(Path::new("examples/plugins"));
    let sm = crate::sm::load_file("verified-capabilities").unwrap();
    let user = "capability-native-real-cargo";
    let _task = task_control::begin(user).unwrap();
    action_contracts::bind(user, "verified-capabilities", &sm, root.path()).unwrap();
    let run = |call: &'static str, tool: &'static str| async {
        serde_json::from_str::<Value>(
            &registry
                .execute_tool_for_task(user, call, tool, &json!({"scope":"workspace"}), None, None)
                .await
                .unwrap(),
        )
        .unwrap()
    };
    for (id, tool) in [
        ("initial-build", "build_workspace"),
        ("initial-tests", "run_workspace_tests"),
    ] {
        assert_eq!(run(id, tool).await["receipt"]["verified"], true);
    }
    action_contracts::require(user, "_complete").unwrap();
    std::fs::write(
        &source,
        "#[test] fn regression() { assert_eq!(2 + 2, 5); }\n",
    )
    .unwrap();
    assert!(action_contracts::require(user, "_complete").is_err());
    assert_eq!(
        run("failing-build", "build_workspace").await["receipt"]["verified"],
        true
    );
    let failed = run("failing-tests", "run_workspace_tests").await;
    assert_eq!(failed["receipt"]["verified"], false);
    assert_eq!(failed["receipt"]["conditions"][0]["exit_code"], 101);
    assert!(action_contracts::require(user, "_complete").is_err());
    std::fs::write(&source, good).unwrap();
    for (id, tool) in [
        ("fixed-build", "build_workspace"),
        ("fixed-tests", "run_workspace_tests"),
    ] {
        assert_eq!(run(id, tool).await["receipt"]["verified"], true);
    }
    action_contracts::require(user, "_complete").unwrap();
}

#[tokio::test]
async fn capability_external_timeout_does_not_claim_remote_rollback() {
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/act"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"verified":true}))
                .set_delay(std::time::Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let (dir, mut registry) = fixture("true");
    registry.plugins.get_mut("sample").unwrap().tools[0].handler = PluginHandler::Http {
        url: format!("{}/act", server.uri()),
        method: "POST".into(),
    };
    contract(&mut registry).effect = contracts::EffectClass::ExternalWrite;
    contract(&mut registry).timeout_secs = 1;
    compensate(&mut registry, dir.path(), "/bin/true");
    let user = "capability-http-uncertain";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "first").await;
    assert_eq!(result["receipt"]["outcome"], "compensation_failed");
    assert_eq!(result["receipt"]["compensation_verified"], false);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn capability_canonical_root_cannot_be_retargeted() {
    let (dir, _registry) = fixture("true");
    let other = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let link = parent.path().join("root");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    let user = "capability-root-link";
    let _task = bind(user, &link);
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(other.path(), &link).unwrap();
    let sm = crate::sm::parse(
        "[state done]\n[action_guards]\ndone = [sample/act]\n_complete = [sample/act]",
    )
    .unwrap();
    assert!(action_contracts::bind(user, "fixture", &sm, &link).is_err());
}
