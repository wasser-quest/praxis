//! Storage namespaced by host-issued plugin owner and authenticated user.
//! Kept on disable/uninstall; no plugin gets a raw connection through this API.
use super::Database;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

fn key(key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::runtime::features::identifier(key) && key != "." && key != "..",
        "Invalid service storage key"
    );
    Ok(())
}
fn encode(value: &Value) -> anyhow::Result<String> {
    let value = serde_json::to_string(value)?;
    anyhow::ensure!(value.len() <= 64 * 1024, "Service storage value too large");
    Ok(value)
}
pub(crate) fn get(
    db: &Database,
    owner: &str,
    user: &str,
    name: &str,
) -> anyhow::Result<Option<Value>> {
    key(name)?;
    let value: Option<String> = db
        .conn()
        .query_row(
            "SELECT value FROM service_storage WHERE owner=?1 AND user_id=?2 AND key=?3",
            params![owner, user, name],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
pub(crate) fn put(
    db: &Database,
    owner: &str,
    user: &str,
    name: &str,
    value: &Value,
) -> anyhow::Result<()> {
    key(name)?;
    let value = encode(value)?;
    db.conn().execute("INSERT INTO service_storage(owner,user_id,key,value) VALUES(?1,?2,?3,?4) ON CONFLICT(owner,user_id,key) DO UPDATE SET value=excluded.value", params![owner,user,name,value])?;
    Ok(())
}
pub(crate) fn compare_exchange(
    db: &Database,
    owner: &str,
    user: &str,
    name: &str,
    expected: Option<&Value>,
    value: &Value,
) -> anyhow::Result<bool> {
    key(name)?;
    let value = encode(value)?;
    let count = if let Some(expected) = expected {
        let expected = encode(expected)?;
        db.conn().execute("UPDATE service_storage SET value=?4 WHERE owner=?1 AND user_id=?2 AND key=?3 AND value=?5", params![owner,user,name,value,expected])?
    } else {
        db.conn().execute(
            "INSERT OR IGNORE INTO service_storage(owner,user_id,key,value) VALUES(?1,?2,?3,?4)",
            params![owner, user, name, value],
        )?
    };
    Ok(count == 1)
}
