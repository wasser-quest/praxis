use super::*;
use crate::gateway::{action_contracts, task_control};
use serde_json::{json, Value};

fn fixture(post: &str) -> (tempfile::TempDir, PluginRegistry) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source"), "old").unwrap();
    let plugin: Plugin = serde_json::from_value(json!({
        "name":"sample", "description":"source edit", "version":"1", "tools":[{
            "name":"modify_source", "description":"edit", "handler":{"type":"source_edit"},
            "parameters":{"type":"object","properties":{
                "path":{"type":"string"},"expected_sha256":{"type":"string"},"content":{"type":"string"}
            },"required":["path","expected_sha256","content"],"additionalProperties":false},
            "contract":{"effect":"workspace_write","idempotency":"non_idempotent","timeout_secs":5,
                "postconditions":[{"program":"/bin/sh","args":["-c",post]}]}
        }]
    })).unwrap();
    let mut registry = PluginRegistry::new();
    registry.register(plugin);
    (dir, registry)
}
fn bind(user: &str, root: &Path) -> task_control::TaskGuard {
    let task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[state done]\n[action_guards]\ndone = [sample/modify_source]\n_complete = [sample/modify_source]").unwrap();
    action_contracts::bind(user, "source-fixture", &sm, root).unwrap();
    task
}
fn input() -> Value {
    json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":"new"})
}
async fn invoke(registry: &PluginRegistry, user: &str, call: &str, args: Value) -> Value {
    serde_json::from_str(
        &registry
            .execute_tool_for_task(user, call, "modify_source", &args, None, None)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn source_capability_commits_and_tracks_the_edited_file_without_declared_resources() {
    let (dir, registry) = fixture("test \"$(cat source)\" = new");
    let user = "source-commit";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "edit", input()).await;
    assert_eq!(result["receipt"]["action"], "sample/modify_source");
    assert_eq!(result["receipt"]["outcome"], "committed");
    assert_eq!(result["receipt"]["verified"], true);
    assert_eq!(
        result["result"]["files"][0]["after_sha256"],
        crate::tools::apply_patch::hash(b"new")
    );
    action_contracts::require(user, "done").unwrap();
    assert!(registry
        .execute_tool_for_task(user, "edit", "modify_source", &input(), None, None)
        .await
        .is_err());
    std::fs::write(dir.path().join("source"), "external").unwrap();
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn source_capability_failed_postcondition_restores_bytes_and_mode() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, registry) = fixture("exit 1");
    std::fs::set_permissions(
        dir.path().join("source"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let user = "source-rollback";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "edit", input()).await;
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(result["receipt"]["verified"], false);
    assert_eq!(result["receipt"]["compensation_verified"], true);
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"old");
    assert_eq!(
        std::fs::metadata(dir.path().join("source"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert!(!crate::tools::apply_patch::journal::pending_exists(
        dir.path()
    ));
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn source_capability_preflight_and_preconditions_cannot_touch_the_file() {
    let (dir, mut registry) = fixture("true");
    let user = "source-preflight";
    let _task = bind(user, dir.path());
    for (i, args) in [
        json!({"path":"source","expected_sha256":"0".repeat(64),"content":"new"}),
        json!({"path":"../source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":"new"}),
        json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(b"old"),"content":"old"}),
    ].into_iter().enumerate() {
        let result = invoke(&registry, user, &format!("bad-{i}"), args).await;
        assert_eq!(result["receipt"]["attempted"], false);
        assert_eq!(result["receipt"]["verified"], false);
    }
    registry.plugins.get_mut("sample").unwrap().tools[0]
        .contract
        .as_mut()
        .unwrap()
        .preconditions = vec![action_contracts::CheckContract {
        program: "/bin/false".into(),
        args: vec![],
        cwd: ".".into(),
        timeout_secs: 1,
        resources: vec![],
    }];
    // Changing a definition needs a new task, even after an unsuccessful attempt.
    drop(_task);
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "pre", input()).await;
    assert_eq!(result["receipt"]["outcome"], "precondition_failed");
    assert_eq!(result["receipt"]["attempted"], false);
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"old");
}

#[tokio::test]
async fn source_capability_timeout_and_dropped_future_use_durable_rollback() {
    let (dir, mut registry) = fixture("touch started; sleep 20");
    registry.plugins.get_mut("sample").unwrap().tools[0]
        .contract
        .as_mut()
        .unwrap()
        .timeout_secs = 1;
    let user = "source-timeout";
    let _task = bind(user, dir.path());
    let result = invoke(&registry, user, "timeout", input()).await;
    assert_eq!(result["receipt"]["failure"], "timed_out");
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"old");
    std::fs::remove_file(dir.path().join("started")).unwrap();
    let mut execution = Box::pin(invoke(&registry, user, "drop", input()));
    let wait = async {
        for _ in 0..500 {
            if dir.path().join("started").exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("postcondition did not start");
    };
    tokio::select! { _ = &mut execution => panic!("unexpected completion"), _ = wait => {} }
    drop(execution);
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"old");
    assert!(!crate::tools::apply_patch::journal::pending_exists(
        dir.path()
    ));
    assert!(action_contracts::require(user, "done").is_err());
}

#[tokio::test]
async fn source_capability_cancel_preserves_external_conflicts() {
    let (dir, registry) = fixture("touch started; sleep 20");
    let user = "source-conflict";
    let _task = bind(user, dir.path());
    let execution = invoke(&registry, user, "edit", input());
    let cancel = async {
        for _ in 0..500 {
            if dir.path().join("started").exists() {
                std::fs::write(dir.path().join("source"), "external").unwrap();
                task_control::cancel(user);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("postcondition did not start");
    };
    let (result, _) = tokio::join!(execution, cancel);
    assert_eq!(result["receipt"]["outcome"], "rollback_conflict");
    assert_eq!(result["receipt"]["compensation_verified"], false);
    assert_eq!(
        std::fs::read(dir.path().join("source")).unwrap(),
        b"external"
    );
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[test]
fn source_capability_requires_native_contract_and_fixed_input_shape() {
    let (_, mut registry) = fixture("true");
    let tool = &mut registry.plugins.get_mut("sample").unwrap().tools[0];
    contracts::validate_tool("sample", tool).unwrap();
    tool.contract.as_mut().unwrap().effect = contracts::EffectClass::ReadOnly;
    assert!(contracts::validate_tool("sample", tool).is_err());
    tool.contract.as_mut().unwrap().effect = contracts::EffectClass::WorkspaceWrite;
    tool.parameters["properties"]["command"] = json!({"type":"string"});
    assert!(contracts::validate_tool("sample", tool).is_err());
    assert!(
        serde_json::from_value::<PluginHandler>(json!({"type":"source_edit","program":"sh"}))
            .is_err()
    );
    tool.contract = None;
    assert!(contracts::validate_tool("sample", tool).is_err());
}

#[tokio::test]
#[ignore = "Requires cargo and rustfmt on PATH; builds and tests a dependency-free temporary project"]
async fn source_capability_bundled_rust_rollback_edit_build_and_test_with_real_cargo() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"praxis-source-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
    std::fs::write(
        root.path().join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"praxis-source-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let code = |value: &str| {
        format!("pub fn answer() -> u32 {{\n    {value}\n}}\n\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn regression() {{\n        assert_eq!(super::answer(), 42);\n    }}\n}}\n")
    };
    let source = root.path().join("src/lib.rs");
    let original = code("41");
    std::fs::write(&source, &original).unwrap();
    let registry = load_all_plugins(Path::new("examples/plugins"));
    let sm = crate::sm::load_file("verified-implementation").unwrap();
    let user = "source-real-cargo";
    let _task = task_control::begin(user).unwrap();
    action_contracts::bind(user, "verified-implementation", &sm, root.path()).unwrap();
    let bad = invoke(&registry,user,"bad",json!({"path":"src/lib.rs","expected_sha256":crate::tools::apply_patch::hash(original.as_bytes()),"content":code("\"wrong\"")})).await;
    assert_eq!(bad["receipt"]["outcome"], "rolled_back");
    assert_eq!(bad["receipt"]["conditions"][0]["outcome"], "passed");
    assert_eq!(bad["receipt"]["conditions"][1]["exit_code"], 101);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
    let good = invoke(&registry,user,"good",json!({"path":"src/lib.rs","expected_sha256":crate::tools::apply_patch::hash(original.as_bytes()),"content":code("42")})).await;
    assert_eq!(good["receipt"]["verified"], true, "{good}");
    for name in ["build_workspace", "run_workspace_tests"] {
        assert!(action_contracts::require(user, "_complete").is_err());
        let output: Value = serde_json::from_str(
            &registry
                .execute_tool_for_task(user, name, name, &json!({"scope":"workspace"}), None, None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(output["receipt"]["verified"], true, "{output}");
    }
    action_contracts::require(user, "done").unwrap();
    action_contracts::require(user, "_complete").unwrap();
}

#[tokio::test]
async fn source_capability_rejects_symlinks_hardlinks_missing_files_and_injected_commands() {
    use std::os::unix::fs::symlink;
    let (dir, registry) = fixture("true");
    symlink("source", dir.path().join("link")).unwrap();
    std::fs::hard_link(dir.path().join("source"), dir.path().join("hard")).unwrap();
    let user = "source-links";
    let _task = bind(user, dir.path());
    for (i, path) in ["link", "hard", "missing"].into_iter().enumerate() {
        let mut args = input();
        args["path"] = json!(path);
        let result = invoke(&registry, user, &format!("bad-{i}"), args).await;
        assert_eq!(result["receipt"]["attempted"], false);
    }
    let mut args = input();
    args["program"] = json!("/bin/true");
    assert!(registry
        .execute_tool_for_task(user, "injected", "modify_source", &args, None, None)
        .await
        .is_err());
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"old");
}

#[tokio::test]
async fn source_capability_path_binding_is_a_single_argv_operand() {
    let (dir, mut registry) = fixture("true");
    let path = "source ; touch escaped ; 'quotes'.rs";
    std::fs::rename(dir.path().join("source"), dir.path().join(path)).unwrap();
    let post = &mut registry.plugins.get_mut("sample").unwrap().tools[0]
        .contract
        .as_mut()
        .unwrap()
        .postconditions[0];
    post.args = vec![
        "-c".into(),
        "test \"$(cat \"$1\")\" = new".into(),
        "check".into(),
        "{source_path}".into(),
    ];
    let user = "source-argv";
    let _task = bind(user, dir.path());
    let mut args = input();
    args["path"] = json!(path);
    let result = invoke(&registry, user, "edit", args).await;
    assert_eq!(result["receipt"]["verified"], true, "{result}");
    assert!(!dir.path().join("escaped").exists());
    action_contracts::require(user, "_complete").unwrap();
}

#[tokio::test]
async fn source_capability_rejects_non_utf8_originals_before_effects() {
    let (dir, registry) = fixture("true");
    std::fs::write(dir.path().join("source"), [0xff]).unwrap();
    let user = "source-binary";
    let _task = bind(user, dir.path());
    let result=invoke(&registry,user,"edit",json!({"path":"source","expected_sha256":crate::tools::apply_patch::hash(&[0xff]),"content":"new"})).await;
    assert_eq!(result["receipt"]["attempted"], false);
    assert_eq!(result["receipt"]["failure"], "source_not_utf8");
    assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), [0xff]);
}
