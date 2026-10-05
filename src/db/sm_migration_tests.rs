use super::*;
use serde_json::json;

#[test]
fn legacy_sm_data_load_merge_save_and_session_fork_are_lossless() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::new(temp.path()).unwrap();
    let legacy =
        json!({"user_id":"alice","cl_data":{"keep":true,"nested":{"old":1}},"cl_file":"legacy"});
    db.conn()
        .execute(
            "INSERT INTO contexts(user_id,data) VALUES (?1,?2)",
            rusqlite::params!["alice", legacy.to_string()],
        )
        .unwrap();
    let ctx = db.load_context("alice").unwrap();
    assert_eq!(ctx.sm_data["keep"], true);
    let ctx = db
        .merge_context(
            "alice",
            json!({"cl_data.nested.new":2,"sm_data.nested.more":3}),
        )
        .unwrap();
    assert_eq!(
        ctx.sm_data,
        json!({"keep":true,"nested":{"old":1,"new":2,"more":3}})
    );
    let stored: String = db
        .conn()
        .query_row("SELECT data FROM contexts WHERE user_id='alice'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert!(stored.get("cl_data").is_none());
    assert!(stored.get("cl_file").is_none());
    assert_eq!(stored["sm_data"]["keep"], true);
    assert_eq!(
        db.fork_context("alice", "bob", None).unwrap().sm_data,
        ctx.sm_data
    );
    assert_eq!(
        db.load_session_context("alice", "scratch").unwrap().sm_data,
        ctx.sm_data
    );
    assert_eq!(
        crate::tools::get_context::get_context(&db, "alice", "cl_data.nested.old")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    db.merge_context("alice", json!({"cl_data":null})).unwrap();
    assert!(db.load_context("alice").unwrap().sm_data.is_null());
}

#[test]
fn canonical_sm_data_wins_alias_conflicts_without_dropping_unrelated_legacy_data() {
    for (mut input, expected) in [
        (
            json!({"cl_data":{"legacy":1,"both":"old"},"sm_data":{"both":"new"}}),
            json!({"sm_data":{"legacy":1,"both":"new"}}),
        ),
        (json!({"cl_data.x":1,"sm_data.x":2}), json!({"sm_data.x":2})),
        (
            json!({"cl_data.x":1,"sm_data":{"x":2}}),
            json!({"sm_data":{"x":2}}),
        ),
        (
            json!({"cl_data.x":1,"sm_data":null}),
            json!({"sm_data":null}),
        ),
        (
            json!({"cl_data":{"x":1},"cl_data.x":2}),
            json!({"sm_data":{"x":1},"sm_data.x":2}),
        ),
    ] {
        normalize_legacy_keys(&mut input);
        assert_eq!(input, expected);
    }
    let old: Context =
        serde_json::from_value(json!({"user_id":"alice","cl_data":{"direct":true}})).unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap()["sm_data"]["direct"],
        true
    );
}

#[tokio::test]
async fn legacy_workflow_conditions_and_assignments_use_canonical_sm_data() {
    let workflow = crate::sm::parse("@name compatibility\n@steps [first, second]\n[state first]\ncl_data.stage = ready\n[state second]\nsm_data.stage = done\n[transitions]\nfirst -> second : when cl_data.stage == ready\n").unwrap();
    let mut value = json!({"active_state":"first", "sm_data":{}});
    crate::sm::apply_to_context(&workflow, &mut value).await;
    assert_eq!(value["sm_data"]["stage"], "ready");
    assert!(value.get("cl_data").is_none());
    assert_eq!(
        crate::sm::advance_workflow(&workflow, &value).await.as_deref(),
        Some("second")
    );
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI"]
async fn legacy_poml_keeps_a_render_only_sm_data_alias() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy.poml");
    std::fs::write(
        &path,
        "<poml><p>{{sm_data.key}} / {{cl_data.key}}</p></poml>",
    )
    .unwrap();
    let input = json!({"sm_data":{"key":"canonical"}, "cl_data":{"key":"old"}});
    let result = crate::gateway::poml::render_strict(&path.to_string_lossy(), &input)
        .await
        .unwrap();
    assert_eq!(result, "canonical / canonical");
    assert_eq!(input["cl_data"]["key"], "old"); // no mutation of saved/caller data
}

#[test]
fn automated_persistent_skill_selection_cannot_bypass_user_only() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::new(temp.path()).unwrap();
    for input in [
        json!({"settings.active_skill":"skill_creator"}),
        json!({"settings":{"active_skill":"skill_creator"}}),
    ] {
        assert!(db
            .merge_context_from_agent("alice", input)
            .unwrap_err()
            .to_string()
            .contains("user-only"));
        assert!(db
            .load_context("alice")
            .unwrap()
            .settings
            .active_skill
            .is_none());
    }
    db.merge_context("alice", json!({"settings.active_skill":"skill_creator"}))
        .unwrap();
    assert_eq!(
        db.load_context("alice")
            .unwrap()
            .settings
            .active_skill
            .as_deref(),
        Some("skill_creator")
    );
    db.merge_context_from_agent("alice", json!({"sm_data.ok":true}))
        .unwrap();
    assert!(db
        .merge_context_from_agent("alice", json!({"settings.active_skill":null}))
        .is_err());
    let workflow =
        crate::sm::parse("@name policy\n[state unsafe]\nsettings.active_skill = skill_creator\n")
            .unwrap();
    let mut ctx = json!({"settings":{"active_skill":null}});
    assert!(!crate::sm::transition_to(&workflow, &mut ctx, "unsafe"));
    assert_eq!(ctx, json!({"settings":{"active_skill":null}}));
}
