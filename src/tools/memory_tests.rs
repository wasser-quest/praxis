use super::*;
use serde_json::json;

#[test]
fn memory_profile_tools_scope_load_create_and_shared_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut ctx = db.load_context("alice").unwrap();
    ctx.settings.system_template = Some("language_instructor".into());
    db.save_context(&ctx).unwrap();
    assert!(set(&db, "alice", &json!({"key":"xp","value":2})).is_err());
    let missing: Value = serde_json::from_str(
        &profile_load(&db, "alice", &json!({"name":"language_instructor"})).unwrap(),
    )
    .unwrap();
    assert_eq!(missing["loaded"], false);
    profile_create(&db, "alice", &json!({"name":"language_instructor"})).unwrap();
    profile_load(&db, "alice", &json!({"name":"language_instructor"})).unwrap();
    set(&db,"alice",&json!({"key":"xp","value":2,"expected_value":null,"expected_profile":"language_instructor"})).unwrap();
    assert!(crate::db::memory::load_memory(&db, "alice")
        .unwrap()
        .custom_variables
        .is_empty());
    assert!(set(
        &db,
        "alice",
        &json!({"key":"name","value":"Ada","scope":"shared"})
    )
    .is_err());
    set(&db,"alice",&json!({"key":"name","value":"Ada","scope":"shared","reason":"User explicitly asked to remember their name","expected_profile":"shared"})).unwrap();
    assert!(set(&db,"alice",&json!({"key":"xp","value":2,"scope":"shared","reason":"practice","expected_profile":"shared"})).is_err());
    let bob: Value =
        serde_json::from_str(&get(&db, "bob", &json!({"key":"name","scope":"shared"})).unwrap())
            .unwrap();
    assert_eq!(bob["exists"], false);
    crate::db::tools::set_plugin_tool_enabled(&db, "memory_profile_load", false).unwrap();
    assert!(profile_load(&db, "alice", &json!({"name":"standard"})).is_err());
}

#[test]
fn memory_tools_persist_typed_srs_and_preserve_other_users_and_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let cards = json!([{"id":"bonjour", "language":"French", "item":"bonjour", "due":"2026-09-15", "interval_days":1}]);
    set(&db, "alice", &json!({"key":"srs_items", "value":cards, "expected_value":null})).unwrap();
    set(&db, "alice", &json!({"key":"xp", "value":2})).unwrap();
    set(&db, "bob", &json!({"key":"xp", "value":99})).unwrap();
    assert_eq!(crate::db::memory::load_memory(&db, "alice").unwrap().custom_variables["srs_items"], cards);
    assert_eq!(crate::db::memory::load_memory(&db, "bob").unwrap().custom_variables["xp"], 99);
    // Already-applied retries succeed; stale edits cannot lose a newer update.
    set(&db, "alice", &json!({"key":"xp", "value":4, "expected_value":2})).unwrap();
    set(&db, "alice", &json!({"key":"xp", "value":4, "expected_value":2})).unwrap();
    assert!(set(&db, "alice", &json!({"key":"xp", "value":6, "expected_value":2})).is_err());
    let loaded: serde_json::Value = serde_json::from_str(&get(&db, "alice", &json!({"key":"xp"})).unwrap()).unwrap();
    assert_eq!(loaded["value"], 4);
    assert!(loaded["exists"].as_bool().unwrap());
    let ctx = db.load_context("alice").unwrap();
    assert!(ctx.custom_data.get("memory").is_none(), "memory is not a discarded context namespace");
    crate::db::tools::set_plugin_tool_enabled(&db, "memory_set", false).unwrap();
    assert!(set(&db, "alice", &json!({"key":"xp", "value":9})).is_err());
    drop(db);
    let db = crate::db::Database::new(dir.path()).unwrap();
    assert_eq!(crate::db::memory::load_memory(&db, "alice").unwrap().custom_variables["srs_items"], cards);
}
