use super::*;
use crate::db::contexts::Context;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn fixture() -> (tempfile::TempDir, GatewayState, String, PathBuf) {
    let install = tempfile::tempdir().unwrap();
    for dir in ["contexts", "templates", "data", "ir-snake/src"] {
        std::fs::create_dir_all(install.path().join(dir)).unwrap();
    }
    std::fs::write(
        install.path().join("contexts/verified-implementation.sm"),
        include_str!("../../contexts/verified-implementation.sm"),
    )
    .unwrap();
    std::fs::write(
        install
            .path()
            .join("templates/verified-implementation.poml"),
        include_str!("../../templates/verified-implementation.poml"),
    )
    .unwrap();
    let workspace = install.path().join("ir-snake");
    std::fs::write(workspace.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(workspace.join("Cargo.toml"), "[package]\nname = \"praxis-workspace-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
    std::fs::write(
        workspace.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"praxis-workspace-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let db = crate::db::Database::new(&install.path().join("data")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let user = format!("ir-workspace-{}", uuid::Uuid::new_v4());
    let mut context = Context {
        user_id: user.clone(),
        ..Default::default()
    };
    context.sm_file = Some("verified-implementation".into());
    context.settings.sm_file = context.sm_file.clone();
    context.active_state = Some("working".into());
    context.settings.active_state = context.active_state.clone();
    context.settings.system_template = Some("verified-implementation".into());
    context.settings.path = "/untrusted/context/path".into();
    db.save_context(&context).unwrap();
    let config = crate::config::Config {
        root_dir: install.path().to_string_lossy().into_owned(),
        workspace_dir: Some("ir-snake".into()),
        ..crate::config::Config::from_env()
    };
    let router =
        llm::LLMRouter::with_providers(vec![], "unused".into(), vec![], Default::default());
    let state = GatewayState {
        db,
        config,
        secrets: Default::default(),
        llm: LlmHandle::new(router),
        plugins: Arc::new(crate::plugins::load_all_plugins(Path::new(
            "examples/plugins",
        ))),
        event_tx: tokio::sync::broadcast::channel(16).0,
        start_time: std::time::Instant::now(),
    };
    (install, state, user, workspace)
}

#[tokio::test]
async fn ir_workspace_preflight_rejects_install_directory_before_binding_or_context_save() {
    let (install, mut state, user, _workspace) = fixture();
    state.config.workspace_dir = None;
    let before = serde_json::to_value(state.db.load_context(&user).unwrap()).unwrap();
    let _task = task_control::begin(&user).unwrap();
    let error = prompt::prepare_runtime(&state, &user, "Build Snake", None, None)
        .unwrap_err().to_string();
    for expected in ["WORKSPACE_DIR", "Cargo.toml", "Cargo.lock", "src", install.path().to_str().unwrap()] {
        assert!(error.contains(expected), "{error}");
    }
    assert!(action_contracts::action_root(&user).is_err());
    assert_eq!(serde_json::to_value(state.db.load_context(&user).unwrap()).unwrap(), before);
    assert!(!install.path().join("Cargo.toml").exists());
}

#[tokio::test]
async fn ir_workspace_preflight_stops_real_message_entry_before_provider_or_tool_calls() {
    for turns in [1, 4] {
        let (install, mut state, user, _workspace) = fixture();
        state.config.workspace_dir = None;
        let mut context = state.db.load_context(&user).unwrap();
        context.settings.max_llm_turns = Some(turns);
        state.db.save_context(&context).unwrap();
        // The router has no providers: reaching chat would produce a different error.
        let error = message_handler::handle_message(&state, &user, "Implement Snake", None)
            .await.unwrap_err().to_string();
        assert!(error.contains("workspace preflight") && error.contains("WORKSPACE_DIR"), "{error}");
        assert!(state.db.get_messages(&user, 100).unwrap().is_empty());
        assert!(!install.path().join("Cargo.toml").exists());
    }
}

#[tokio::test]
async fn ir_workspace_preflight_accepts_the_explicit_prepared_project() {
    let (_install, state, user, workspace) = fixture();
    let _task = task_control::begin(&user).unwrap();
    prompt::prepare_runtime(&state, &user, "Build Snake", None, None).unwrap();
    assert_eq!(action_contracts::action_root(&user).unwrap(), workspace.canonicalize().unwrap());
}

#[tokio::test]
async fn ir_workspace_preflight_requirements_cannot_be_removed_during_a_task() {
    let (install, state, user, workspace) = fixture();
    let _task = task_control::begin(&user).unwrap();
    prompt::prepare_runtime(&state, &user, "Build Snake", None, None).unwrap();
    let mut changed = crate::sm::load_file_in(&install.path().join("contexts"), "verified-implementation").unwrap();
    changed.workspace = Default::default();
    let error = action_contracts::bind(&user, "verified-implementation", &changed, &workspace).unwrap_err().to_string();
    assert!(error.contains("pinned"), "{error}");
    assert_eq!(action_contracts::action_root(&user).unwrap(), workspace.canonicalize().unwrap());
}

#[test]
fn ir_workspace_preflight_parser_rejects_ambiguous_or_escaping_requirements() {
    for assignment in [
        "required_files = [\"../Cargo.toml\"]", "required_files = [\"/Cargo.toml\"]",
        "required_files = [\"src/../Cargo.toml\"]", "required_files = [\"./Cargo.toml\"]",
        "required_files = [\"src//main.rs\"]", "required_files = [\"src\\\\main.rs\"]",
        "required_files = [\"\"]", "required_files = [\"Cargo.toml\", \"Cargo.toml\"]",
        "required_files = Cargo.toml", "required_file = [\"Cargo.toml\"]",
        "required_files = [\"Cargo.toml\"]\nrequired_files = [\"Cargo.lock\"]",
    ] {
        assert!(crate::sm::parse(&format!("[state working]\n[workspace]\n{assignment}")).is_err(), "{assignment}");
    }
    assert!(crate::sm::parse("[state working]\n[workspace]\nrequired_files = [\"Cargo.toml\"]\nrequired_directories = [\"src\"]").is_ok());
}

#[tokio::test]
async fn ir_workspace_runtime_uses_project_root_and_keeps_install_assets_separate() {
    let (_install, state, user, workspace) = fixture();
    let _task = task_control::begin(&user).unwrap();
    let context =
        prompt::prepare_runtime(&state, &user, "Implement the prepared project.", None, None)
            .unwrap();
    assert_eq!(
        action_contracts::action_root(&user).unwrap(),
        workspace.canonicalize().unwrap()
    );
    assert_eq!(context.settings.path, "/untrusted/context/path");
    let read: Value = serde_json::from_str(
        &crate::tools::apply_patch::inspect(&user, &json!({"path":"src/main.rs"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["content"], "fn main() {}\n");
    assert_eq!(
        read["workspace_root"],
        workspace.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(action_contracts::instructions(&user).contains(workspace.to_str().unwrap()));
}

#[tokio::test]
async fn ir_workspace_root_stays_pinned_until_a_new_task() {
    let (install, mut state, user, workspace) = fixture();
    let next = install.path().join("other-project");
    std::fs::create_dir(&next).unwrap();
    std::fs::create_dir(next.join("src")).unwrap();
    for file in ["Cargo.toml", "Cargo.lock", "src/main.rs"] {
        std::fs::copy(workspace.join(file), next.join(file)).unwrap();
    }
    let task = task_control::begin(&user).unwrap();
    prompt::prepare_runtime(&state, &user, "First task", None, None).unwrap();
    state.config.workspace_dir = Some("other-project".into());
    let before = state.db.load_context(&user).unwrap();
    assert!(prompt::prepare_runtime(&state, &user, "Try changing roots", None, None).is_err());
    assert_eq!(
        action_contracts::action_root(&user).unwrap(),
        workspace.canonicalize().unwrap()
    );
    assert_eq!(
        serde_json::to_value(state.db.load_context(&user).unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    drop(task);
    let _task = task_control::begin(&user).unwrap();
    prompt::prepare_runtime(&state, &user, "New task", None, None).unwrap();
    assert_eq!(
        action_contracts::action_root(&user).unwrap(),
        next.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn ir_workspace_inspection_reports_missing_parents_without_weakening_path_checks() {
    let (_install, state, user, workspace) = fixture();
    let _task = task_control::begin(&user).unwrap();
    let sm = crate::sm::parse("[state working]\n[decision_ir]\nR = inspect_file").unwrap();
    action_contracts::bind(&user, "missing-path", &sm, &workspace).unwrap();
    let read: Value = serde_json::from_str(
        &crate::tools::apply_patch::inspect(&user, &json!({"path":"missing/src/main.rs"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["exists"], false);
    assert_eq!(read["content"], Value::Null);
    assert_eq!(read["sha256"], Value::Null);
    assert_eq!(
        read["workspace_root"],
        workspace.canonicalize().unwrap().to_str().unwrap()
    );
    for path in [
        "../Cargo.toml",
        "missing/../Cargo.toml",
        "./src/main.rs",
        "/absolute.rs",
    ] {
        assert!(
            crate::tools::apply_patch::inspect(&user, &json!({"path":path}))
                .await
                .is_err()
        );
    }
    assert!(!workspace.join("missing").exists());
    assert!(
        crate::tools::apply_patch::inspect(&user, &json!({"path":"src/main.rs/nested"}))
            .await
            .is_err()
    );
    assert_eq!(
        state.db.load_context(&user).unwrap().settings.path,
        "/untrusted/context/path"
    );
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires real Cargo; proves a child without its own manifest cannot verify its parent"]
async fn ir_workspace_rust_checks_never_search_parent_manifest() {
    let (install, state, user, workspace) = fixture();
    std::fs::remove_file(workspace.join("Cargo.toml")).unwrap();
    std::fs::remove_file(workspace.join("Cargo.lock")).unwrap();
    std::fs::create_dir(install.path().join("src")).unwrap();
    std::fs::write(install.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(install.path().join("Cargo.toml"), "[package]\nname = \"praxis-parent-must-not-verify\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
    std::fs::write(
        install.path().join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"praxis-parent-must-not-verify\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let _task = task_control::begin(&user).unwrap();
    let mut sm = crate::sm::load_file_in(&install.path().join("contexts"), "verified-implementation")
        .unwrap();
    // Isolate the verifier's manifest boundary independently of the new setup check.
    sm.workspace = Default::default();
    action_contracts::bind(&user, "verified-implementation", &sm, &workspace).unwrap();
    for tool in ["build_workspace", "run_workspace_tests"] {
        let result: Value = serde_json::from_str(
            &state
                .plugins
                .execute_tool_for_task(&user, tool, tool, &json!({"scope":"workspace"}), None, None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["receipt"]["verified"], false, "{result}");
        assert_ne!(result["receipt"]["outcome"], "committed");
        assert!(result["receipt"]["conditions"][0]["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("Cargo.toml"));
    }
    assert!(action_contracts::require(&user, "_complete").is_err());

    // The older named-check workflows must enforce the same project boundary.
    for (name, source, checks) in [
        (
            "verified-coding",
            include_str!("../../contexts/verified-coding.sm"),
            vec!["build", "tests"],
        ),
        (
            "verified-capabilities",
            include_str!("../../contexts/verified-capabilities.sm"),
            vec!["build"],
        ),
    ] {
        let user = format!("missing-manifest-{name}-{}", uuid::Uuid::new_v4());
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse(source).unwrap();
        action_contracts::bind(&user, name, &sm, &workspace).unwrap();
        for check in checks {
            let result: Value = serde_json::from_str(
                &action_contracts::run(&user, check, &json!({"name":check}))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                result["receipt"]["verified"], false,
                "{name}/{check}: {result}"
            );
            assert!(result["output"]["stderr"]
                .as_str()
                .unwrap()
                .contains("Cargo.toml"));
        }
    }
}
