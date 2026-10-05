use super::{action_contracts, prompt, task_control};
use crate::{db::contexts::Context, sm};
use serde_json::json;

#[test]
fn coding_profile_trigger_is_task_owned_sticky_and_pinned() {
    let root = tempfile::tempdir().unwrap();
    let workflow = sm::parse("[state code]\n[state review]\n[action_guards]\n_complete = [native/modify_source, native/build, native/tests]\n[action_guard_triggers]\n_complete = [native/modify_source]").unwrap();
    let user = "coding-trigger-policy";
    let task = task_control::begin(user).unwrap();
    action_contracts::bind(user, "coding", &workflow, root.path()).unwrap();
    action_contracts::require(user, "_complete").unwrap();
    action_contracts::start_action(user, "build-first", "native/build", "build-hash").unwrap();
    action_contracts::require(user, "_complete").unwrap();
    action_contracts::start_action(user, "edit", "native/modify_source", "edit-hash").unwrap();
    assert!(action_contracts::require(user, "_complete").is_err());
    task_control::with_verification(user, |ledger| {
        ledger.invalidate();
        Ok(())
    })
    .unwrap();
    assert!(
        action_contracts::require(user, "_complete").is_err(),
        "failed/rolled-back edit must not reset the trigger"
    );
    let mut relaxed = workflow.clone();
    relaxed.action_guard_triggers.clear();
    assert!(action_contracts::bind(user, "coding", &relaxed, root.path()).is_err());
    let before = Context {
        user_id: user.into(),
        active_state: Some("code".into()),
        ..Default::default()
    };
    let mut after = before.clone();
    after.active_state = Some("review".into());
    after.settings.active_state = after.active_state.clone();
    after.settings.sm_file = Some("coding".into());
    action_contracts::validate_context(&before, &after).unwrap();
    after.settings.done = true;
    assert!(action_contracts::validate_context(&before, &after).is_err());
    drop(task);
    let _new_task = task_control::begin(user).unwrap();
    action_contracts::bind(user, "coding", &workflow, root.path()).unwrap();
    action_contracts::require(user, "_complete").unwrap();
}

#[test]
fn coding_profile_trigger_does_not_skip_unconditional_named_checks() {
    let root = tempfile::tempdir().unwrap();
    let _workspace = crate::tools::apply_patch::journal::ready(root.path()).unwrap();
    let workflow = sm::parse("[state code]\n[checks]\ncheck = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [check]\n[action_guards]\n_complete = [native/edit]\n[action_guard_triggers]\n_complete = [native/edit]").unwrap();
    let mut ledger = action_contracts::VerificationState::default();
    ledger.bind("coding", &workflow, root.path()).unwrap();
    assert!(ledger
        .require("_complete")
        .unwrap_err()
        .to_string()
        .contains("verified checks"));
}

#[test]
fn coding_profile_trigger_parser_rejects_unbound_empty_duplicate_and_unknown_triggers() {
    let prefix =
        "[state code]\n[action_guards]\n_complete = [native/edit]\n[action_guard_triggers]\n";
    for line in [
        "_complete = []",
        "_complete = [native/other]",
        "missing = [native/edit]",
        "_complete = [native/edit, native/edit]",
        "_complete = [native/edit]\n_complete = [native/edit]",
        "_complete = native/edit",
    ] {
        assert!(sm::parse(&format!("{prefix}{line}")).is_err(), "{line}");
    }
}

#[tokio::test]
async fn coding_profile_normal_roles_offer_semantic_actions_and_keep_legacy_opt_in() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let plugins = crate::plugins::load_all_plugins(&root.join("examples/plugins"));
    for role in [
        "code",
        "expert_programmer",
        "debugger",
        "senior_dev",
        "code_architect",
        "review",
    ] {
        let mut ctx = db.load_context("coding-roles").unwrap();
        ctx.settings.sm_file = Some("standard-verified".into());
        ctx.settings.system_template = None;
        ctx.settings.use_decision_router = false;
        ctx.sm_data = json!({"role":role});
        prompt::route_context(root, &mut ctx, "Inspect the project", &plugins, None).await.unwrap();
        assert_eq!(ctx.active_state.as_deref(), Some(role));
        let workflow = sm::load_file_in(&root.join("contexts"), "standard-verified").unwrap();
        super::workflow_preflight::validate(&db, &plugins, "standard-verified", &workflow, &ctx)
            .unwrap();
        let tools = crate::tools::registry::build_tool_definitions(
            &ctx.settings,
            Some(&plugins.tool_definitions()),
            Some(&db),
        );
        let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
        for semantic in [
            "execute_decision",
            "inspect_file",
            "build_workspace",
            "run_workspace_tests",
        ] {
            assert!(names.contains(&semantic), "{role}: {semantic}");
        }
        for raw in ["execute_terminal", "write_file", "edit_file"] {
            assert!(!names.contains(&raw), "{role}: raw {raw} must be absent");
        }
        assert_eq!(
            names.contains(&"modify_source"),
            !matches!(role, "review" | "code_architect")
        );
    }
    let mut legacy = db.load_context("coding-legacy").unwrap();
    legacy.settings.use_decision_router = false;
    legacy.sm_data = json!({"role":"code"});
    prompt::route_context(root, &mut legacy, "Code", &plugins, None).await.unwrap();
    assert_eq!(
        crate::tools::registry::build_tool_definitions(&legacy.settings, None, Some(&db))
            .iter()
            .any(|t| t.function.name == "execute_terminal"),
        cfg!(feature = "shell")
    );
}
