//! Plugin migration scope, reversibility and immutability.
use super::migrations::{self, PluginMigration};
use rusqlite::Connection;

fn migration(id: &str, up: &str, down: Option<&str>) -> PluginMigration {
    PluginMigration {
        id: id.into(),
        up: up.into(),
        down: down.map(str::to_owned),
    }
}

fn tables(data_dir: &std::path::Path, owner: &str) -> Vec<String> {
    let conn = Connection::open(migrations::database_path(data_dir, owner).unwrap()).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}

#[test]
fn migrations_apply_idempotently_and_isolate_owners() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    // A real core database so the plugin database is provably separate.
    let _core = crate::db::Database::new(data).unwrap();

    let alpha = migration(
        "alpha_0001",
        "CREATE TABLE alpha_items(id INTEGER PRIMARY KEY);",
        Some("DROP TABLE alpha_items;"),
    );
    let beta = migration(
        "beta_0001",
        "CREATE TABLE beta_items(id INTEGER PRIMARY KEY);",
        Some("DROP TABLE beta_items;"),
    );

    assert_eq!(
        migrations::apply(data, "alpha", std::slice::from_ref(&alpha)).unwrap(),
        vec!["alpha_0001"]
    );
    assert!(migrations::apply(data, "alpha", std::slice::from_ref(&alpha))
        .unwrap()
        .is_empty());
    assert_eq!(
        migrations::apply(data, "beta", std::slice::from_ref(&beta)).unwrap(),
        vec!["beta_0001"]
    );

    let alpha_tables = tables(data, "alpha");
    assert!(alpha_tables.contains(&"alpha_items".to_string()));
    assert!(!alpha_tables.contains(&"beta_items".to_string()));
    let beta_tables = tables(data, "beta");
    assert!(beta_tables.contains(&"beta_items".to_string()));
    assert!(!beta_tables.contains(&"alpha_items".to_string()));

    // The plugin database never contains host/core tables.
    assert!(!alpha_tables.contains(&"messages".to_string()));
}

#[test]
fn migrations_are_reversible_in_reverse_order() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let first = migration("ext_0001", "CREATE TABLE one(x);", Some("DROP TABLE one;"));
    let second = migration("ext_0002", "CREATE TABLE two(x);", Some("DROP TABLE two;"));
    let all = [first.clone(), second.clone()];
    assert_eq!(
        migrations::apply(data, "ext", &all).unwrap(),
        vec!["ext_0001", "ext_0002"]
    );
    assert_eq!(
        migrations::revert(data, "ext", &all).unwrap(),
        vec!["ext_0002", "ext_0001"]
    );
    assert!(migrations::history(data, "ext").unwrap().is_empty());
    let names = tables(data, "ext");
    assert!(!names.contains(&"one".to_string()) && !names.contains(&"two".to_string()));
}

#[test]
fn changed_script_is_rejected_and_recorded_history_is_immutable() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let original = migration("ext_0001", "CREATE TABLE original(x);", None);
    assert_eq!(
        migrations::apply(data, "ext", std::slice::from_ref(&original)).unwrap(),
        vec!["ext_0001"]
    );
    let tampered = migration("ext_0001", "CREATE TABLE tampered(x);", None);
    assert!(migrations::apply(data, "ext", std::slice::from_ref(&tampered)).is_err());
    let history = migrations::history(data, "ext").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].0, "ext_0001");
    assert!(tables(data, "ext").contains(&"original".to_string()));
}

#[test]
fn missing_down_script_blocks_revert() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let migration = migration("ext_0001", "CREATE TABLE one(x);", None);
    migrations::apply(data, "ext", std::slice::from_ref(&migration)).unwrap();
    assert!(migrations::revert(data, "ext", std::slice::from_ref(&migration)).is_err());
    assert_eq!(migrations::history(data, "ext").unwrap().len(), 1);
}

#[test]
fn migration_cannot_read_core_tables() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let _core = crate::db::Database::new(data).unwrap();
    // `messages` exists in the core database but not in the package database.
    let bad = migration("ext_0001", "CREATE TABLE leaked AS SELECT * FROM messages;", None);
    assert!(migrations::apply(data, "ext", std::slice::from_ref(&bad)).is_err());
    assert!(migrations::history(data, "ext").unwrap().is_empty());
}

#[test]
fn database_path_rejects_invalid_owners() {
    let dir = tempfile::tempdir().unwrap();
    for owner in ["", "../escape", "a/b", "a b"] {
        assert!(migrations::database_path(dir.path(), owner).is_err(), "{owner}");
    }
    assert!(migrations::database_path(dir.path(), "ext").is_ok());
}

#[test]
fn load_requires_an_in_package_up_script() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("pkg");
    std::fs::create_dir_all(package.join("migrations")).unwrap();
    std::fs::write(package.join("migrations/ext_0001.sql"), "SELECT 1;").unwrap();
    std::fs::write(package.join("migrations/ext_0001.down.sql"), "SELECT 1;").unwrap();
    let loaded = migrations::load(&package, &["ext_0001".into()]).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].down.is_some());

    // Missing up script, invalid id and escaping id all fail.
    assert!(migrations::load(&package, &["ext_0002".into()]).is_err());
    assert!(migrations::load(&package, &["../escape".into()]).is_err());
    assert!(migrations::load(&package, &["a/b".into()]).is_err());
}
