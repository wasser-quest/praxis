use super::action_contracts::*;
use crate::sm;
use serde_json::json;

fn policy() -> sm::StateMachine {
    sm::parse(
        r#"
@steps [working, done]
[state working]
[state done]
[checks]
tests = {"program":"cargo","args":["test","--locked"],"cwd":".","timeout_secs":60}
[guards]
done = [tests]
_complete = [tests]
"#,
    )
    .unwrap()
}

#[test]
fn contract_parser_rejects_unknown_checks_and_malformed_contracts() {
    assert!(sm::parse("[state done]\n[guards]\ndone = [invented]").is_err());
    assert!(sm::parse("[checks]\ntests = {\"program\":\"cargo\",\"timeout_secs\":0}").is_err());
    assert!(sm::parse("[checks]\ntests = {\"program\":\"cargo\",\"shell\":true}").is_err());
}

#[test]
fn contract_guards_require_current_runtime_evidence() {
    let root = tempfile::tempdir().unwrap();
    let _workspace = crate::tools::apply_patch::journal::ready(root.path()).unwrap();
    let workflow = policy();
    let mut ledger = VerificationState::default();
    ledger.bind("guarded", &workflow, root.path()).unwrap();
    assert!(ledger.require("done").is_err());
    // Model/context data never enters the ledger.
    let forged = json!({"checks":{"tests":{"verified":true}}});
    assert_eq!(forged["checks"]["tests"]["verified"], true);
    assert!(ledger.require("_complete").is_err());
    let revision = ledger.start_check("tests").unwrap();
    ledger.finish_check(
        "tests",
        revision,
        Outcome::Passed,
        Some(0),
        "call-1",
        CheckEvidence::capture(&workflow.checks["tests"], root.path()).unwrap(),
    );
    assert!(ledger.require("done").is_ok());
    ledger.invalidate();
    assert!(ledger.require("done").is_err());
    // A late success from an older revision cannot authorize completion.
    ledger.finish_check(
        "tests",
        revision,
        Outcome::Passed,
        Some(0),
        "call-2",
        CheckEvidence::capture(&workflow.checks["tests"], root.path()).unwrap(),
    );
    assert!(ledger.require("_complete").is_err());
}

#[test]
fn contract_failed_rerun_revokes_success_and_policy_is_pinned() {
    let root = tempfile::tempdir().unwrap();
    let _workspace = crate::tools::apply_patch::journal::ready(root.path()).unwrap();
    let workflow = policy();
    let mut ledger = VerificationState::default();
    ledger.bind("guarded", &workflow, root.path()).unwrap();
    let rev = ledger.start_check("tests").unwrap();
    ledger.finish_check(
        "tests",
        rev,
        Outcome::Passed,
        Some(0),
        "pass",
        CheckEvidence::capture(&workflow.checks["tests"], root.path()).unwrap(),
    );
    ledger.start_check("tests").unwrap();
    assert!(ledger.require("done").is_err());
    ledger.finish_check(
        "tests",
        rev,
        Outcome::Failed,
        Some(1),
        "fail",
        CheckEvidence::capture(&workflow.checks["tests"], root.path()).unwrap(),
    );
    assert!(ledger.require("done").is_err());
    assert!(ledger.bind("other", &workflow, root.path()).is_err());
    let mut edited = workflow.clone();
    edited.guards.clear();
    assert!(ledger.bind("guarded", &edited, root.path()).is_err());
}

#[test]
fn contract_receipts_do_not_survive_new_task() {
    let root = tempfile::tempdir().unwrap();
    let _workspace = crate::tools::apply_patch::journal::ready(root.path()).unwrap();
    let workflow = policy();
    let user = "contract-task-replay";
    let task = super::task_control::begin(user).unwrap();
    bind(user, "guarded", &workflow, root.path()).unwrap();
    super::task_control::with_verification(user, |ledger| {
        let rev = ledger.start_check("tests")?;
        ledger.finish_check(
            "tests",
            rev,
            Outcome::Passed,
            Some(0),
            "old",
            CheckEvidence::capture(&workflow.checks["tests"], root.path()).unwrap(),
        );
        Ok(())
    })
    .unwrap();
    assert!(require(user, "_complete").is_ok());
    drop(task);
    let _next = super::task_control::begin(user).unwrap();
    bind(user, "guarded", &workflow, root.path()).unwrap();
    assert!(require(user, "_complete").is_err());
}

#[test]
fn contract_force_transition_cannot_skip_guard() {
    let workflow = policy();
    let mut ctx = json!({"user_id":"contract-direct", "active_state":"working"});
    assert!(!sm::transition_to(&workflow, &mut ctx, "done"));
    assert_eq!(ctx["active_state"], "working");
}

#[test]
fn contract_context_mutations_cannot_clear_state_change_workflow_or_mark_done() {
    let user = "contract-context-authority";
    let _task = super::task_control::begin(user).unwrap();
    bind(user, "guarded", &policy(), std::path::Path::new(".")).unwrap();
    let mut before = crate::db::contexts::Context {
        user_id: user.into(),
        active_state: Some("working".into()),
        ..Default::default()
    };
    before.settings.sm_file = Some("guarded".into());
    before.settings.active_state = before.active_state.clone();
    let mut after = before.clone();
    after.settings.done = true;
    assert!(validate_context(&before, &after).is_err());
    after = before.clone();
    after.active_state = None;
    after.settings.active_state = None;
    assert!(validate_context(&before, &after).is_err());
    after = before.clone();
    after.settings.sm_file = Some("standard".into());
    assert!(validate_context(&before, &after).is_err());
    after = before.clone();
    after.active_state = Some("done".into());
    after.settings.active_state = after.active_state.clone();
    assert!(validate_context(&before, &after).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn contract_runner_records_real_exit_status_and_rejects_model_command_override() {
    for (user, program, verified) in [
        ("contract-real-pass", "/bin/true", true),
        ("contract-real-fail", "/bin/false", false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut workflow = policy();
        workflow.checks.get_mut("tests").unwrap().program = program.into();
        workflow.checks.get_mut("tests").unwrap().args.clear();
        let _task = super::task_control::begin(user).unwrap();
        bind(user, "guarded", &workflow, root.path()).unwrap();
        assert!(run(
            user,
            "override",
            &json!({"name":"tests","program":"/bin/true"})
        )
        .await
        .is_err());
        let result: serde_json::Value =
            serde_json::from_str(&run(user, "actual", &json!({"name":"tests"})).await.unwrap())
                .unwrap();
        assert_eq!(result["receipt"]["verified"], verified);
        assert_eq!(require(user, "_complete").is_ok(), verified);
        before_tool(user, "read_file").unwrap();
        assert_eq!(require(user, "_complete").is_ok(), verified);
        before_tool(user, "write_file").unwrap();
        assert!(require(user, "_complete").is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn contract_timeout_and_cancellation_never_produce_verified_receipts() {
    for (user, cancelled) in [("contract-timeout", false), ("contract-cancelled", true)] {
        let root = tempfile::tempdir().unwrap();
        let mut workflow = policy();
        let check = workflow.checks.get_mut("tests").unwrap();
        check.program = "/bin/sleep".into();
        check.args = vec!["10".into()];
        check.timeout_secs = 1;
        let _task = super::task_control::begin(user).unwrap();
        bind(user, "guarded", &workflow, root.path()).unwrap();
        if cancelled {
            let token = super::task_control::cancellation(user).unwrap();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                token.cancel();
            });
        }
        let result: serde_json::Value = serde_json::from_str(
            &run(user, "bounded", &json!({"name":"tests"}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["receipt"]["verified"], false);
        assert_eq!(
            result["receipt"]["outcome"],
            if cancelled { "cancelled" } else { "timed_out" }
        );
        assert!(require(user, "done").is_err());
    }
}

#[tokio::test]
async fn contract_database_and_completion_tools_reject_before_persistence() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(root.path()).unwrap();
    let user = "contract-database-boundary";
    let _task = super::task_control::begin(user).unwrap();
    let workflow = sm::load_file("verified-coding").unwrap();
    bind(
        user,
        "verified-coding",
        &workflow,
        std::path::Path::new("."),
    )
    .unwrap();
    let mut ctx = crate::db::contexts::Context {
        user_id: user.into(),
        active_state: Some("working".into()),
        ..Default::default()
    };
    ctx.settings.sm_file = Some("verified-coding".into());
    ctx.settings.active_state = ctx.active_state.clone();
    db.save_context(&ctx).unwrap();
    for update in [
        json!({"settings.done":true}),
        json!({"active_state":"done"}),
        json!({"settings.sm_file":"standard"}),
    ] {
        assert!(db.merge_context_from_agent(user, update).is_err());
        let actual = db.load_context(user).unwrap();
        assert_eq!(actual.active_state, ctx.active_state);
        assert!(!actual.settings.done);
        assert_eq!(super::prompt::workflow_name(&actual), "verified-coding");
    }
    assert!(crate::tools::agent_control::run(
        &db,
        user,
        crate::tools::agent_control::AgentControlSignal::Complete
    )
    .await
    .is_err());
    assert!(!db.load_context(user).unwrap().settings.done);
    let _workspace = crate::tools::apply_patch::journal::ready(std::path::Path::new(".")).unwrap();
    super::task_control::with_verification(user, |ledger| {
        for name in ["build", "tests"] {
            let rev = ledger.start_check(name)?;
            let evidence =
                CheckEvidence::capture(&workflow.checks[name], std::path::Path::new("."))?;
            ledger.finish_check(
                name,
                rev,
                Outcome::Passed,
                Some(0),
                "runtime-check",
                evidence,
            );
        }
        Ok(())
    })
    .unwrap();
    db.merge_context_from_agent(user, json!({"active_state":"done"}))
        .unwrap();
    crate::tools::agent_control::run(
        &db,
        user,
        crate::tools::agent_control::AgentControlSignal::Complete,
    )
    .await
    .unwrap();
    assert!(db.load_context(user).unwrap().settings.done);
}
