//! Context profiles: named snapshots of a session's settings, custom data and
//! workflow choice that can be applied to another session.
use super::admin::{Failure, Outcome};
use serde_json::{json, Value};

fn ensure_table(db: &crate::db::Database) {
    let _ = db.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS context_profiles (
            name TEXT PRIMARY KEY,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    );
}

pub fn validate_name(name: &str) -> Outcome<()> {
    if !name.is_empty()
        && name.len() <= 100
        && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(Failure::BadRequest("Invalid profile name".into()))
    }
}

pub fn list(db: &crate::db::Database) -> Outcome<Value> {
    ensure_table(db);
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT name, created_at FROM context_profiles ORDER BY created_at")?;
    let profiles: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({"name": row.get::<_, String>(0)?, "created_at": row.get::<_, String>(1)?}))
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(json!({ "profiles": profiles }))
}

/// Snapshot `source_user`'s settings, custom data and workflow (volatile
/// state, history and receipts are not copied).
pub fn save(db: &crate::db::Database, name: &str, source_user: &str) -> Outcome<Value> {
    let name = name.trim();
    validate_name(name)?;
    let ctx = db.load_context(source_user).map_err(|_| Failure::NotFound)?;
    let snapshot = json!({
        "settings": ctx.settings,
        "custom_data": ctx.custom_data,
        "sm_file": ctx.sm_file,
    });
    ensure_table(db);
    db.conn().execute(
        "INSERT INTO context_profiles (name, data, created_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET data = ?2, created_at = datetime('now')",
        rusqlite::params![name, snapshot.to_string()],
    )?;
    Ok(json!({"success": true, "name": name}))
}

pub fn apply(db: &crate::db::Database, name: &str, user_id: &str) -> Outcome<Value> {
    ensure_table(db);
    // Read in its own lock scope: merge_context takes the lock again.
    let update = {
        let conn = db.conn();
        let data: String = conn
            .query_row(
                "SELECT data FROM context_profiles WHERE name = ?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .map_err(|_| Failure::NotFound)?;
        let snapshot: Value = serde_json::from_str(&data)?;
        json!({
            "settings": snapshot.get("settings").cloned().unwrap_or_default(),
            "custom_data": snapshot.get("custom_data").cloned().unwrap_or_default(),
            "sm_file": snapshot.get("sm_file").cloned().unwrap_or_default(),
        })
    };
    let ctx = db
        .merge_context(user_id, update)
        .map_err(|e| Failure::BadRequest(e.to_string()))?;
    Ok(serde_json::to_value(ctx)?)
}

pub fn delete(db: &crate::db::Database, name: &str) -> Outcome<Value> {
    ensure_table(db);
    db.conn()
        .execute("DELETE FROM context_profiles WHERE name = ?1", rusqlite::params![name])?;
    Ok(json!({"success": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_snapshot_and_apply_settings_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let mut ctx = db.load_context("alice").unwrap();
        ctx.settings.system_template = Some("language_instructor".into());
        db.save_context(&ctx).unwrap();
        assert!(matches!(save(&db, "../x", "alice"), Err(Failure::BadRequest(_))));
        save(&db, "teacher", "alice").unwrap();
        assert_eq!(list(&db).unwrap()["profiles"][0]["name"], "teacher");
        let applied = apply(&db, "teacher", "bob").unwrap();
        assert_eq!(applied["settings"]["system_template"], "language_instructor");
        assert!(matches!(apply(&db, "missing", "bob"), Err(Failure::NotFound)));
        delete(&db, "teacher").unwrap();
        assert_eq!(list(&db).unwrap()["profiles"], json!([]));
    }
}
