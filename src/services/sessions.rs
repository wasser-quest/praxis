//! Frontend-independent session, context and message APIs. Dashboard, TUI,
//! gateway and future clients call these; none of them owns the queries.
use crate::db::Database;
use serde_json::{json, Value};

/// All chat sessions known to the server (context rows are authoritative;
/// message stats/preview resolve forked `user:::session` keys too).
pub fn chat_sessions(db: &Database) -> anyhow::Result<Value> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT user_id, updated_at FROM contexts ORDER BY updated_at DESC LIMIT 500",
        )
        ?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        ?
        .collect::<Result<Vec<_>, _>>()
        ?;
    drop(stmt);

    let mut sessions = Vec::new();
    for (user_id, updated_at) in rows {
        let username: Option<String> = conn
            .query_row(
                "SELECT data FROM contexts WHERE user_id = ?1 LIMIT 1",
                rusqlite::params![user_id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|data| {
                serde_json::from_str::<crate::db::contexts::Context>(&data).ok()
            })
            .and_then(|ctx| ctx.username.filter(|u| !u.trim().is_empty()));
        // Messages live under the session key or its forked `:::` sub-keys.
        let prefix = format!("{user_id}:::");
        let message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE user_id = ?1 OR substr(user_id, 1, length(?2)) = ?2",
                rusqlite::params![user_id, prefix],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let preview: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE user_id = ?1 AND role = 'user' AND content <> '' ORDER BY id ASC LIMIT 1",
                rusqlite::params![user_id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .or_else(|| {
                conn.query_row(
                    "SELECT content FROM messages WHERE substr(user_id, 1, length(?1)) = ?1 AND role = 'user' AND content <> '' ORDER BY id ASC LIMIT 1",
                    rusqlite::params![prefix],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            })
            .map(|text| {
                let flat: String = text.trim().chars().take(120).collect();
                flat
            });
        let title: Option<String> = conn.query_row(
            "SELECT json_extract(data, '$.custom_data.session_title') FROM contexts WHERE user_id=?1",
            rusqlite::params![user_id], |row| row.get(0),
        ).ok().flatten();
        sessions.push(serde_json::json!({
            "user_id": user_id,
            "session_title": title,
            "username": username,
            "updated_at": updated_at,
            "message_count": message_count,
            "preview": preview,
        }));
    }
    Ok(serde_json::json!({ "sessions": sessions }))
}

pub fn contexts(db: &Database) -> anyhow::Result<Value> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT user_id, data, updated_at FROM contexts ORDER BY updated_at DESC")
        ?;

    let contexts: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let user_id: String = row.get(0)?;
            let data: String = row.get(1)?;
            let updated_at: String = row.get(2)?;
            let mut data: serde_json::Value = serde_json::from_str(&data).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
            crate::db::contexts::normalize_legacy_keys(&mut data);
            Ok(serde_json::json!({
                "user_id": user_id,
                "data": data,
                "updated_at": updated_at,
            }))
        })
        ?
        .collect::<Result<Vec<_>, _>>()
        ?;

    Ok(serde_json::json!({ "contexts": contexts }))
}

/// Message history with token/speed telemetry. `chat_only` hides rows up to
/// the chat-clear marker; the full history stays available.
pub fn messages(db: &Database, user_id: &str, chat_only: bool) -> anyhow::Result<Value> {
    let budget = 500000usize;
        tracing::debug!(user_id = %user_id, chat_only, "[MESSAGES] fetching messages");
    let result = if chat_only {
        // Chat view: hide rows up to the clear marker (kept in Messages tab).
        let marker = db
            .load_context(&user_id)
            .ok()
            .and_then(|ctx| {
                ctx.custom_data
                    .get("chat_cleared_message_id")
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(0);
        db.get_chat_messages_after(&user_id, budget, marker)
    } else {
        db.get_chat_messages_with_token_budget(&user_id, budget)
    };
    match result {
        Ok((messages, total_tokens)) => {
            let msgs: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| {
                    let mut val = serde_json::json!({
                        "id": m.id,
                        "audio_mime": m.audio_mime,
                        "role": m.role,
                        "content": m.content,
                        "tool_call_id": m.tool_call_id,
                        "tool_name": m.tool_name,
                        "prompt_tokens": m.prompt_tokens,
                        "completion_tokens": m.completion_tokens,
                        "total_tokens": m.total_tokens,
                        "generation_ms": m.generation_ms,
                    });
                    if let Some(meta) = m.discord_meta.as_ref() {
                        val["discord_meta"] = meta.clone();
                    }
                    if let Some(ref tool_calls) = m.tool_calls {
                        val["tool_calls"] = serde_json::json!(tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                })
                            })
                            .collect::<Vec<_>>());
                    }
                    val
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "messages": msgs,
                "total_tokens": total_tokens,
                "message_count": messages.len(),
            }))
        }
        Err(error) => Err(error),
    }
}

/// Hide everything up to the newest message from the chat view only.
pub fn clear_chat_view(db: &Database, user_id: &str) -> anyhow::Result<i64> {
    let max_id = db.max_message_id(user_id)?;
    let _ = db.merge_context(
        user_id,
        serde_json::json!({ "custom_data": { "chat_cleared_message_id": max_id } }),
    );
    Ok(max_id)
}

pub fn fork(
    db: &Database,
    parent: &str,
    new_user_id: &str,
    username: Option<&str>,
) -> anyhow::Result<Value> {
    anyhow::ensure!(
        !new_user_id.is_empty() && new_user_id != parent,
        "Invalid fork target"
    );
    let ctx = db.fork_context(parent, new_user_id, username)?;
    Ok(serde_json::json!({
        "success": true, "user_id": ctx.user_id, "username": ctx.username,
        "parent_user_id": parent,
    }))
}

/// Workflow status of a session (dashboard `/sm/:user`).
pub fn sm_info(db: &Database, user_id: &str) -> anyhow::Result<Value> {
    let ctx = db.load_context(user_id)?;
    let sm_data = match ctx.sm_data.as_object() {
        Some(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        None => json!({}),
    };
    Ok(json!({
        "sm_file": crate::gateway::prompt::workflow_name(&ctx),
        "system_template": ctx.settings.system_template.as_deref().unwrap_or("standard"),
        "active_skill": ctx.settings.active_skill,
        "active_state": ctx.active_state,
        "active_templates": ctx.active_templates,
        "sm_data": sm_data,
    }))
}

/// Apply one `/context` command line (same parser as chat `/context`).
pub fn exec(db: &Database, user_id: &str, line: &str) -> Value {
    match crate::context_cmd::parse(line) {
        Ok(op) => json!({ "response": crate::context_cmd::apply(db, user_id, &op) }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::messages::Message;

    #[test]
    fn headless_session_apis_match_dashboard_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let ctx = db.load_context("u").unwrap();
        db.save_context(&ctx).unwrap();
        let mut reply = Message::assistant("old reply".into());
        reply.prompt_tokens = Some(12);
        reply.generation_ms = Some(340);
        db.add_message("u", &Message::user("hello there".into())).unwrap();
        db.add_message("u", &reply).unwrap();
        let marker = clear_chat_view(&db, "u").unwrap();
        db.add_message("u", &Message::user("after".into())).unwrap();

        let full = messages(&db, "u", false).unwrap();
        assert_eq!(full["message_count"], 3);
        // Prompt/token/speed telemetry is preserved for the Messages tab.
        assert_eq!(full["messages"][1]["prompt_tokens"], 12);
        assert_eq!(full["messages"][1]["generation_ms"], 340);
        let chat = messages(&db, "u", true).unwrap();
        assert_eq!(chat["message_count"], 1);
        assert!(marker > 0);

        let sessions = chat_sessions(&db).unwrap();
        assert_eq!(sessions["sessions"][0]["user_id"], "u");
        assert_eq!(sessions["sessions"][0]["message_count"], 3);
        assert_eq!(sessions["sessions"][0]["preview"], "hello there");
        assert_eq!(contexts(&db).unwrap()["contexts"][0]["user_id"], "u");
        assert!(fork(&db, "u", "u", None).is_err());
        assert_eq!(fork(&db, "u", "child", None).unwrap()["parent_user_id"], "u");
    }
}
