use super::{action_contracts, task_control, workflow_graph};
use crate::db::Database;
use serde_json::json;

#[tokio::test]
async fn learning_flow_waits_for_real_input_and_context_claims_cannot_satisfy_guard() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("contexts")).unwrap();
    let text = "@routing graph\n@start setup\n[state setup]\n[state practice]\n[state feedback]\n[transitions]\nsetup -> practice\npractice -> feedback\n[user_reply_guards]\nfeedback = [practice]";
    std::fs::write(root.path().join("contexts/lesson.sm"), text).unwrap();
    let sm = crate::sm::parse(text).unwrap();
    let db = Database::new(&root.path().join("data")).unwrap();
    let user = "learning-input-guard";
    let mut ctx = db.load_context(user).unwrap();
    ctx.settings.sm_file = Some("lesson".into());
    ctx.active_state = Some("setup".into());
    ctx.settings.active_state = ctx.active_state.clone();
    db.save_context(&ctx).unwrap();
    let _task = task_control::begin(user).unwrap();
    action_contracts::bind(user, "lesson", &sm, root.path()).unwrap();
    action_contracts::validate_context(&ctx, &ctx).unwrap();
    let first = db
        .add_message(
            user,
            &crate::db::messages::Message::user("Teach me French".into()),
        )
        .unwrap();
    action_contracts::record_user_message(user, first).unwrap();
    let transition = workflow_graph::navigate(&db, root.path(), user, false, &json!({"edge":0}))
        .await
        .unwrap();
    let transition: serde_json::Value = serde_json::from_str(&transition).unwrap();
    assert_eq!(
        transition["graph"]["reply_facts"]["state_changed_since_input"],
        true
    );
    assert_eq!(
        transition["graph"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["to"] == "feedback")
            .unwrap()["eligible"],
        false
    );
    let before = json!(db.load_context(user).unwrap());
    let error = workflow_graph::navigate(&db, root.path(), user, false, &json!({"edge":0}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("new user message"), "{error}");
    assert_eq!(json!(db.load_context(user).unwrap()), before);
    assert_eq!(workflow_graph::history(user), vec!["setup"]);
    db.merge_context_from_agent(user, json!({"sm_data.learner_answer_received":true}))
        .unwrap();
    assert!(action_contracts::require(user, "feedback").is_err());
    let reply = db
        .add_message(user, &crate::db::messages::Message::user("bonjour".into()))
        .unwrap();
    action_contracts::record_user_message(user, reply).unwrap();
    action_contracts::require(user, "feedback").unwrap();
    workflow_graph::navigate(&db, root.path(), user, false, &json!({"edge":0}))
        .await
        .unwrap();
    assert_eq!(
        db.load_context(user).unwrap().active_state.as_deref(),
        Some("feedback")
    );
    assert!(
        action_contracts::require(user, "feedback").is_err(),
        "same input cannot re-enter feedback"
    );
}

#[test]
fn learning_flow_reply_guard_parser_rejects_unknown_states_and_non_graph_workflows() {
    for text in [
        "@routing graph\n[state practice]\n[state feedback]\n[user_reply_guards]\nfeedback = []",
        "@routing graph\n[state practice]\n[state feedback]\n[user_reply_guards]\nfeedback = [missing]",
        "[state practice]\n[state feedback]\n[user_reply_guards]\nfeedback = [practice]",
        "@routing graph\n[state practice]\n[state feedback]\n[user_reply_guards]\nfeedback = [practice, practice]",
    ] { assert!(crate::sm::parse(text).is_err(), "{text}"); }
}

#[tokio::test]
async fn learning_flow_uses_the_tutor_memory_namespace_across_stages() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let plugins = crate::plugins::PluginRegistry::new();
    crate::db::memory_profiles::create_profile(&db, "learner", "language_instructor").unwrap();
    for stage in ["setup", "lesson", "practice", "feedback", "review", "done"] {
        let mut ctx = db.load_context("learner").unwrap();
        ctx.settings.sm_file = Some("language-learning".into());
        ctx.active_state = Some(stage.into());
        ctx.settings.active_state = ctx.active_state.clone();
        super::prompt::route_context(root, &mut ctx, "Practice French", &plugins, None).await.unwrap();
        assert_eq!(ctx.settings.max_llm_turns, Some(1));
        assert!(!ctx.settings.use_decision_router);
        assert_eq!(
            crate::db::memory_profiles::mode(&ctx).unwrap(),
            "language_instructor"
        );
        db.save_context(&ctx).unwrap();
        crate::db::memory_profiles::load_profile(&db, &ctx, "language_instructor").unwrap();
        let sm = crate::sm::load_file_in(&root.join("contexts"), "language-learning").unwrap();
        super::workflow_preflight::validate(&db, &plugins, "language-learning", &sm, &ctx).unwrap();
        let snapshot = crate::db::memory_profiles::snapshot(&db, &ctx).unwrap();
        assert_eq!(snapshot.profile, "language_instructor");
        assert!(snapshot.exists);
    }
    let mut other = db.load_context("other-learner").unwrap();
    other.settings.system_template = Some("language-flow".into());
    assert!(
        !crate::db::memory_profiles::snapshot(&db, &other)
            .unwrap()
            .exists
    );
}
