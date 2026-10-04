use super::{contexts::Context, memory, memory_profiles as profiles, Database};
use serde_json::json;

fn fixture() -> (Database, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (Database::new(dir.path()).unwrap(), dir)
}
fn context(user: &str, mode: &str) -> Context {
    let mut ctx = Context {
        user_id: user.into(),
        ..Default::default()
    };
    ctx.settings.system_template = Some(mode.into());
    ctx
}

#[test]
fn memory_profiles_preserve_legacy_but_never_fall_back_across_modes() {
    let (db, _dir) = fixture();
    db.add_memory("alice", "legacy fact", Some("fact")).unwrap();
    let standard = profiles::snapshot(&db, &context("alice", "standard")).unwrap();
    assert_eq!(standard.profile, "standard");
    assert_eq!(standard.memory.learned_facts, ["legacy fact"]);
    let tutor = context("alice", "language_instructor");
    let view = profiles::snapshot(&db, &tutor).unwrap();
    assert_eq!(view.profile, "language_instructor");
    assert!(!view.exists);
    assert!(view.memory.learned_facts.is_empty());
    assert!(!profiles::load_profile(&db, &tutor, "language_instructor").unwrap());
    assert!(profiles::create_profile(&db, "alice", "language_instructor").unwrap());
    assert!(!profiles::create_profile(&db, "alice", "language_instructor").unwrap());
    assert!(profiles::load_profile(&db, &tutor, "language_instructor").unwrap());
    profiles::update_named(&db, "alice", "language_instructor", |m| {
        m.custom_variables.insert("xp".into(), json!(2));
        Ok(())
    })
    .unwrap();
    let view = profiles::snapshot(&db, &tutor).unwrap();
    assert!(view.loaded);
    assert_eq!(view.memory.custom_variables["xp"], 2);
    assert!(profiles::snapshot(&db, &context("alice", "code_assistant"))
        .unwrap()
        .memory
        .custom_variables
        .is_empty());
    assert!(
        !profiles::snapshot(&db, &context("bob", "language_instructor"))
            .unwrap()
            .exists
    );
    assert_eq!(
        memory::load_memory(&db, "alice").unwrap().learned_facts,
        ["legacy fact"]
    );
}

#[test]
fn memory_profiles_selection_is_mode_and_session_scoped_and_durable() {
    let (db, dir) = fixture();
    let tutor = context("alice", "language_instructor");
    profiles::create_profile(&db, "alice", "french_practice").unwrap();
    profiles::load_profile(&db, &tutor, "french_practice").unwrap();
    assert_eq!(
        profiles::snapshot(&db, &tutor).unwrap().profile,
        "french_practice"
    );
    assert_eq!(
        profiles::snapshot(&db, &context("alice", "code_assistant"))
            .unwrap()
            .profile,
        "code_assistant"
    );
    let mut second = tutor.clone();
    second.session_id = "second".into();
    assert_eq!(
        profiles::snapshot(&db, &second).unwrap().profile,
        "language_instructor"
    );
    drop(db);
    let db = Database::new(dir.path()).unwrap();
    assert_eq!(
        profiles::snapshot(&db, &tutor).unwrap().profile,
        "french_practice"
    );
}

#[test]
fn memory_profiles_shared_is_explicit_small_and_never_cross_user() {
    let (db, _dir) = fixture();
    profiles::update_named(&db, "alice", "shared", |m| {
        m.custom_variables.insert("name".into(), json!("Ada"));
        Ok(())
    })
    .unwrap();
    for mode in ["standard", "language_instructor", "code_assistant"] {
        let view = profiles::snapshot(&db, &context("alice", mode)).unwrap();
        assert_eq!(view.shared["name"], "Ada");
        assert!(!view.memory.custom_variables.contains_key("name"));
    }
    assert!(profiles::snapshot(&db, &context("bob", "standard"))
        .unwrap()
        .shared
        .is_empty());
    assert!(profiles::create_profile(&db, "alice", "shared").is_err());
    assert!(profiles::load_profile(&db, &context("alice", "standard"), "shared").is_err());
    assert!(profiles::update_named(&db, "alice", "shared", |m| {
        m.custom_variables.insert("srs_items".into(), json!([1, 2]));
        Ok(())
    })
    .is_err());
    assert_eq!(
        profiles::read_named(&db, "alice", "shared")
            .unwrap()
            .unwrap()
            .custom_variables
            .len(),
        1
    );
}

#[test]
fn memory_profiles_validate_and_fail_without_partial_writes() {
    let (db, _dir) = fixture();
    for bad in ["", "../bob", "/absolute", "a\\b", "a\nb", "a//b"] {
        assert!(
            profiles::create_profile(&db, "alice", bad).is_err(),
            "{bad:?}"
        );
    }
    let ctx = context("alice", "language_instructor");
    db.save_context(&ctx).unwrap();
    assert!(profiles::update_current(&db, "alice", None, |m| {
        m.learned_facts.push("wrong".into());
        Ok(())
    })
    .is_err());
    profiles::create_profile(&db, "alice", "language_instructor").unwrap();
    assert!(profiles::update_current(&db, "alice", Some("standard"), |_| Ok(())).is_err());
    assert!(
        profiles::update_current(&db, "alice", Some("language_instructor"), |m| {
            m.learned_facts.push("rollback".into());
            anyhow::bail!("synthetic abort")
        })
        .is_err()
    );
    assert!(profiles::snapshot(&db, &ctx)
        .unwrap()
        .memory
        .learned_facts
        .is_empty());
}

#[tokio::test]
async fn memory_profiles_prompt_uses_routed_preview_context_not_stale_database_mode() {
    let (db, dir) = fixture();
    crate::db::tools::init_default_tools(&db).unwrap();
    db.add_memory("alice", "legacy private fact", Some("fact"))
        .unwrap();
    profiles::create_profile(&db, "alice", "language_instructor").unwrap();
    profiles::update_named(&db, "alice", "language_instructor", |m| {
        m.custom_variables.insert("xp".into(), json!(2));
        Ok(())
    })
    .unwrap();
    // Database still says standard; preview/runtime has already routed to the tutor.
    let value = crate::gateway::prompt::build_context(
        &db,
        &context("alice", "language_instructor"),
        "synthetic",
        &crate::plugins::PluginRegistry::new(),
        0,
        dir.path(),
    )
    .await
    .unwrap();
    assert_eq!(value["memory"]["profile"], "language_instructor");
    assert_eq!(value["memory"]["variables"]["xp"], 2);
    assert_eq!(value["memory"]["facts"], json!([]));
    assert_eq!(
        db.load_context("alice").unwrap().settings.system_template,
        None
    );
}

#[test]
fn memory_profiles_legacy_upgrade_keeps_ids_and_data_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let initial = Database::new(dir.path()).unwrap();
        initial.conn().execute_batch("DROP TABLE memory_profile_selections; DROP TABLE memory_profiles; INSERT INTO memory(id,user_id,fact,category) VALUES (42,'alice','keep this','fact'); PRAGMA user_version=11;").unwrap();
    }
    let db = Database::new(dir.path()).unwrap();
    assert_eq!(db.get_memories("alice", 10).unwrap()[0].id, 42);
    assert_eq!(
        profiles::snapshot(&db, &context("alice", "standard"))
            .unwrap()
            .memory
            .learned_facts,
        ["keep this"]
    );
    let version: i32 = db
        .conn()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 16); // includes owner/user-scoped native service storage
    drop(db);
    assert_eq!(
        Database::new(dir.path())
            .unwrap()
            .get_memories("alice", 10)
            .unwrap()[0]
            .id,
        42
    );
}

#[test]
fn memory_profiles_tutor_aliases_share_the_relevant_learning_category() {
    for alias in ["language_learning", "daily_quiz", "tasks/daily_quiz"] {
        assert_eq!(
            profiles::mode(&context("alice", alias)).unwrap(),
            "language_instructor"
        );
    }
    assert_eq!(
        profiles::mode(&context("alice", "code_assistant")).unwrap(),
        "code_assistant"
    );
}

#[test]
fn memory_profiles_concurrent_updates_preserve_unrelated_keys() {
    let (db, _dir) = fixture();
    profiles::create_profile(&db, "alice", "language_instructor").unwrap();
    let threads: Vec<_> = (0..4)
        .map(|n| {
            let db = db.clone();
            std::thread::spawn(move || {
                profiles::update_named(&db, "alice", "language_instructor", |m| {
                    m.custom_variables.insert(format!("key{n}"), json!(n));
                    Ok(())
                })
                .unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(
        profiles::read_named(&db, "alice", "language_instructor")
            .unwrap()
            .unwrap()
            .custom_variables
            .len(),
        4
    );
}

#[test]
fn memory_profiles_delete_user_preserves_other_owners() {
    let (db, _dir) = fixture();
    for user in ["a_%", "another"] {
        profiles::create_profile(&db, user, "language_instructor").unwrap();
        profiles::load_profile(
            &db,
            &context(user, "language_instructor"),
            "language_instructor",
        )
        .unwrap();
    }
    db.delete_context("a_%").unwrap();
    assert!(
        !profiles::snapshot(&db, &context("a_%", "language_instructor"))
            .unwrap()
            .exists
    );
    assert!(
        profiles::snapshot(&db, &context("another", "language_instructor"))
            .unwrap()
            .exists
    );
}
