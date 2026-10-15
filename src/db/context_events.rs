//! Append-only change log for contexts.
//!
//! Every write of a context row records what changed as one event with a
//! per-context sequence number: the changed JSON paths (before → after) and who
//! changed them (`user`, `model:set_context`, `tool:<name>`, `workflow`,
//! `compaction`, or `runtime`). The context row stays the current snapshot; the
//! log explains how it got there, lets clients catch up after a disconnect
//! (`after=<seq>`), and lets writers say "apply only if nothing changed since
//! seq N" instead of silently overwriting each other.
//!
//! Past events are never edited. To change something, append a new change
//! (see `Database::change_context`); the log then shows both.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Values larger than this are logged as size + hash, not content.
const MAX_LOGGED_VALUE_BYTES: usize = 512;
/// Events kept per context; older ones are pruned.
const MAX_EVENTS_PER_CONTEXT: i64 = 10_000;
/// Changed paths kept per event (a whole-context rewrite is summarized).
const MAX_CHANGES_PER_EVENT: usize = 64;

tokio::task_local! {
    static SOURCE: String;
}

/// Attribute context writes made while `call` runs to `source`.
pub async fn with_source<F: std::future::Future>(source: impl Into<String>, call: F) -> F::Output {
    SOURCE.scope(source.into(), call).await
}

pub fn current_source() -> String {
    SOURCE.try_with(Clone::clone).unwrap_or_else(|_| "runtime".into())
}

/// The write would overwrite changes the caller has not seen.
#[derive(Debug, thiserror::Error)]
#[error("context changed since seq {expected} (now {current}); reload and retry")]
pub struct Conflict {
    pub expected: i64,
    pub current: i64,
}

/// Leaf-level differences between two JSON documents, as dotted paths.
/// Objects are compared key by key; arrays and scalars as whole values.
pub fn diff(before: &Value, after: &Value) -> Vec<Value> {
    let mut changes = Vec::new();
    walk(String::new(), before, after, &mut changes);
    changes
}

fn walk(path: String, before: &Value, after: &Value, out: &mut Vec<Value>) {
    if before == after {
        return;
    }
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                walk(child, a.get(key).unwrap_or(&Value::Null), b.get(key).unwrap_or(&Value::Null), out);
            }
        }
        _ => out.push(json!({"path": path, "before": compact(before), "after": compact(after)})),
    }
}

/// Small values verbatim; large ones as `{"bytes": n, "sha256": "…"}`.
fn compact(value: &Value) -> Value {
    let text = value.to_string();
    if text.len() <= MAX_LOGGED_VALUE_BYTES {
        return value.clone();
    }
    json!({"bytes": text.len(), "sha256": format!("{:x}", Sha256::digest(text.as_bytes()))})
}

pub(crate) fn latest_seq(conn: &Connection, key: &str) -> anyhow::Result<i64> {
    Ok(conn
        .query_row("SELECT max(seq) FROM context_events WHERE user_key=?1", [key], |r| r.get::<_, Option<i64>>(0))?
        .unwrap_or(0))
}

pub(crate) fn current_data(conn: &Connection, key: &str) -> anyhow::Result<Option<String>> {
    Ok(conn
        .query_row("SELECT data FROM contexts WHERE user_id=?1", [key], |r| r.get(0))
        .optional()?)
}

/// Fail with `Conflict` unless `key` is still at `expected`.
pub(crate) fn check_expected(conn: &Connection, key: &str, expected: Option<i64>) -> anyhow::Result<()> {
    if let Some(expected) = expected {
        let current = latest_seq(conn, key)?;
        if current != expected {
            return Err(Conflict { expected, current }.into());
        }
    }
    Ok(())
}

/// Append the difference between `before` and `after` for `key`. Returns the
/// new event (with its seq), or `None` when nothing changed.
pub(crate) fn record(
    conn: &Connection,
    key: &str,
    before: Option<&str>,
    after: &str,
    source: &str,
) -> anyhow::Result<Option<Value>> {
    let created = before.is_none();
    // A new row is compared with the default context, so "created" lists only
    // what differs from the defaults, path by path.
    let before: Value = match before.and_then(|raw| serde_json::from_str(raw).ok()) {
        Some(value) => value,
        None => serde_json::to_value(super::contexts::Context::default())?,
    };
    let after: Value = serde_json::from_str(after)?;
    let mut changes = diff(&before, &after);
    if changes.is_empty() {
        return Ok(None);
    }
    let total = changes.len();
    changes.truncate(MAX_CHANGES_PER_EVENT);
    let kind = if created { "created" } else { "changed" };
    let payload = json!({"changes": changes, "changed_paths": total});
    let seq = latest_seq(conn, key)? + 1;
    conn.execute(
        "INSERT INTO context_events(user_key,seq,kind,source,payload) VALUES(?1,?2,?3,?4,?5)",
        params![key, seq, kind, source, payload.to_string()],
    )?;
    conn.execute(
        "DELETE FROM context_events WHERE user_key=?1 AND seq<=?2",
        params![key, seq - MAX_EVENTS_PER_CONTEXT],
    )?;
    Ok(Some(json!({"seq": seq, "kind": kind, "source": source, "changes": payload["changes"], "changed_paths": total})))
}

/// What goes onto the live event stream: which paths changed, by whom, at
/// which seq. Never the values; those stay behind the authenticated
/// `/v1/context/:user/events` endpoint.
pub(crate) fn live_notice(event: &Value) -> String {
    let paths: Vec<&Value> = event["changes"].as_array().map(|c| c.iter().map(|c| &c["path"]).collect()).unwrap_or_default();
    json!({"seq": event["seq"], "kind": event["kind"], "source": event["source"], "paths": paths,
        "changed_paths": event["changed_paths"]}).to_string()
}

pub(crate) fn list(conn: &Connection, key: &str, after: i64, limit: usize) -> anyhow::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT seq,kind,source,payload,created_at FROM context_events WHERE user_key=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![key, after, limit.clamp(1, 1000) as i64], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
    })?;
    rows.map(|row| {
        let (seq, kind, source, payload, at) = row?;
        let payload: Value = serde_json::from_str(&payload)?;
        Ok(json!({"seq": seq, "kind": kind, "source": source, "at": at,
            "changes": payload["changes"], "changed_paths": payload["changed_paths"]}))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_leaf_paths_and_summarizes_large_values() {
        let before = json!({"turn": 1, "settings": {"model": "a", "keep": true}, "sm_data": {}});
        let after = json!({"turn": 2, "settings": {"model": "b", "keep": true}, "sm_data": {"role": "code"}, "custom_data": {"big": "x".repeat(600)}});
        let changes = diff(&before, &after);
        let paths: Vec<_> = changes.iter().map(|c| c["path"].as_str().unwrap()).collect();
        assert_eq!(paths, vec!["custom_data", "settings.model", "sm_data.role", "turn"]);
        assert_eq!(changes[1]["before"], "a");
        assert_eq!(changes[1]["after"], "b");
        assert!(changes[0]["after"]["sha256"].is_string(), "large values are hashed");
        assert!(diff(&after, &after).is_empty());
    }

    #[tokio::test]
    async fn source_defaults_to_runtime_and_follows_the_scope() {
        assert_eq!(current_source(), "runtime");
        with_source("tool:set_context", async { assert_eq!(current_source(), "tool:set_context") }).await;
    }
}

#[cfg(test)]
mod database_tests {
    use crate::db::Database;
    use serde_json::json;

    #[tokio::test]
    async fn every_write_is_logged_with_its_source_and_conflicts_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let user = "log-user";
        assert_eq!(db.context_seq(user).unwrap(), 0);

        db.merge_context(user, json!({"settings.max_llm_turns": 7})).unwrap();
        db.merge_context_from_agent(user, json!({"sm_data.role": "code"})).unwrap();
        super::with_source("tool:example", async {
            let mut ctx = db.load_context(user).unwrap();
            ctx.turn = 3;
            db.save_context(&ctx).unwrap();
            // Saving the same state again is not a change.
            db.save_context(&ctx).unwrap();
        })
        .await;

        let events = db.context_events(user, 0, 100).unwrap();
        let summary: Vec<(i64, &str)> = events.iter().map(|e| (e["seq"].as_i64().unwrap(), e["source"].as_str().unwrap())).collect();
        assert_eq!(summary, vec![(1, "user"), (2, "model:set_context"), (3, "tool:example")]);
        let change = events[0]["changes"].as_array().unwrap().iter().find(|c| c["path"] == "settings.max_llm_turns").unwrap();
        assert_eq!(change["after"], 7);
        assert_eq!(db.context_events(user, 2, 100).unwrap().len(), 1, "catch up after a seq");

        // Optimistic concurrency: apply only if nobody changed it since seq 3.
        db.change_context(user, json!({"settings.max_llm_turns": 9}), Some(3)).unwrap();
        let stale = db.change_context(user, json!({"settings.max_llm_turns": 1}), Some(3)).unwrap_err();
        let conflict = stale.downcast_ref::<super::Conflict>().expect("conflict, not another error");
        assert_eq!((conflict.expected, conflict.current), (3, 4));
        assert_eq!(db.load_context(user).unwrap().settings.max_llm_turns, Some(9), "the stale write did not land");
    }
}
