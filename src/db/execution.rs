//! Dashboard audit records are separate from model conversation history.
use super::{contexts::Context, Database};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn key(ctx: &Context) -> String {
    if ctx.session_id.is_empty() || ctx.session_id == "default" {
        ctx.user_id.clone()
    } else {
        format!("{}:::{}", ctx.user_id, ctx.session_id)
    }
}

pub(super) fn insert_event(
    tx: &rusqlite::Transaction<'_>,
    ctx: &Context,
    kind: &str,
    payload: &Value,
    prompt_hash: Option<&str>,
) -> anyhow::Result<i64> {
    let user_key = key(ctx);
    let cursor: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id),0) FROM messages WHERE user_id=?1",
        [&user_key],
        |row| row.get(0),
    )?;
    tx.execute("INSERT INTO execution_events(user_key,kind,payload,prompt_hash,message_cursor) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![user_key, kind, serde_json::to_string(payload)?, prompt_hash, cursor])?;
    Ok(tx.last_insert_rowid())
}

impl Database {
    pub fn record_execution_event(
        &self,
        ctx: &Context,
        kind: &str,
        payload: &Value,
    ) -> anyhow::Result<i64> {
        self.execution_event(ctx, kind, payload, None)
    }

    fn execution_event(
        &self,
        ctx: &Context,
        kind: &str,
        payload: &Value,
        prompts: Option<&[String]>,
    ) -> anyhow::Result<i64> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let prompt_hash = if let Some(prompts) = prompts {
            let body = serde_json::to_string(prompts)?;
            let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
            tx.execute(
                "INSERT OR IGNORE INTO execution_prompts(hash,body) VALUES(?1,?2)",
                rusqlite::params![hash, body],
            )?;
            Some(hash)
        } else {
            None
        };
        let id = insert_event(&tx, ctx, kind, payload, prompt_hash.as_deref())?;
        tx.commit()?;
        Ok(id)
    }

    pub fn record_model_attempt(
        &self,
        ctx: &Context,
        request: &crate::gateway::llm::provider::ChatRequest,
        provider: &str,
        logical_call: &str,
        attempt: u64,
        limits: &Value,
    ) -> anyhow::Result<i64> {
        let prompts: Vec<_> = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.clone().unwrap_or_default())
            .collect();
        let tools: Vec<_> = request
            .tools
            .as_ref()
            .into_iter()
            .flatten()
            .map(|t| &t.function)
            .collect();
        let snapshot = json!({"active_state":ctx.active_state,"workflow":crate::gateway::prompt::workflow_name(ctx),
            "system_template":ctx.settings.system_template,"thinking_mode":ctx.settings.thinking_mode,
            "activated_tools":ctx.settings.activated_tools,"history_token_limit":ctx.settings.history_token_limit,
            "compaction_enabled":ctx.settings.compaction_enabled,"compaction_token_limit":crate::gateway::compaction::threshold(&ctx.settings)});
        self.execution_event(ctx, "model_call", &json!({"logical_call":logical_call,"attempt":attempt,"status":"running",
            "provider":provider,"model":request.model,"state":ctx.active_state,"template":ctx.settings.system_template,
            "settings":snapshot,"limits":limits,"output_limit":request.max_tokens,"tools":tools,
            "decision_ir":crate::gateway::action_contracts::decision_ir_mapping_for(&ctx.user_id, ctx.active_state.as_deref().unwrap_or("")).ok(),
            "message_count":request.messages.len()}), Some(&prompts))
    }

    pub fn finish_model_attempt(
        &self,
        id: i64,
        status: &str,
        usage: Option<&crate::gateway::llm::provider::Usage>,
        elapsed_ms: u64,
        first_token_ms: Option<u64>,
        finish_reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        let raw: String = conn.query_row(
            "SELECT payload FROM execution_events WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        let mut value: Value = serde_json::from_str(&raw)?;
        value["status"] = json!(status);
        value["usage"] = json!(usage);
        value["elapsed_ms"] = json!(elapsed_ms);
        value["first_token_ms"] = json!(first_token_ms);
        value["finish_reason"] = json!(finish_reason);
        value["tokens_per_sec"] = json!(usage
            .filter(|_| elapsed_ms > 0)
            .map(|u| f64::from(u.completion_tokens) * 1000.0 / elapsed_ms as f64));
        conn.execute(
            "UPDATE execution_events SET payload=?2 WHERE id=?1",
            rusqlite::params![id, serde_json::to_string(&value)?],
        )?;
        Ok(())
    }

    pub fn execution_events(&self, user: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
        let ctx = self.load_context(user)?;
        let conn = self.conn();
        let mut query = conn.prepare("SELECT e.id,e.kind,e.payload,e.prompt_hash,e.message_cursor,e.created_at,p.body FROM execution_events e LEFT JOIN execution_prompts p ON p.hash=e.prompt_hash WHERE e.user_key=?1 ORDER BY e.id DESC LIMIT ?2")?;
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let rows = query.query_map(rusqlite::params![key(&ctx), limit.min(500) as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (id, kind, payload, hash, cursor, time, body) = row?;
            let prompts = if hash.as_ref().is_some_and(|hash| seen.insert(hash.clone())) {
                body.map(|b| serde_json::from_str::<Value>(&b))
                    .transpose()?
            } else {
                None
            };
            result.push(json!({"id":id,"kind":kind,"payload":serde_json::from_str::<Value>(&payload)?,"prompt_hash":hash,
                "message_cursor":cursor,"created_at":time,"system_prompts":prompts}));
        }
        result.reverse();
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use crate::db::Database;
    use serde_json::json;
    #[test]
    fn execution_audit_persists_and_isolates_sessions_without_polluting_history() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path()).unwrap();
        let mut ctx = db.load_context("audit-user").unwrap();
        db.save_context(&ctx).unwrap();
        db.record_execution_event(
            &ctx,
            "state_transition",
            &json!({"from_state":"a","to_state":"b"}),
        )
        .unwrap();
        assert!(db.get_messages("audit-user", 100).unwrap().is_empty());
        assert_eq!(db.execution_events("audit-user", 100).unwrap().len(), 1);
        ctx.session_id = "other".into();
        db.save_context(&ctx).unwrap();
        assert!(db.execution_events("audit-user", 100).unwrap().is_empty());
        db.record_execution_event(&ctx, "compaction", &json!({"status":"completed"}))
            .unwrap();
        drop(db);
        let db = Database::new(dir.path()).unwrap();
        assert_eq!(
            db.execution_events("audit-user", 100).unwrap()[0]["kind"],
            "compaction"
        );
        assert!(db.execution_events("another-user", 100).unwrap().is_empty());
    }
}
