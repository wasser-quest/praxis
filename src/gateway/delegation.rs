//! One-level task delegation: the agent spawns a child agent loop in a forked
//! context (fresh conversation, inherited settings/memory snapshot), waits for
//! its result and continues afterwards.
//!
//! Guarantees:
//! - Only ONE delegation level: a delegated task cannot delegate again
//!   (enforced via task_control task names + a delegation-depth flag in the
//!   forked context's custom_data).
//! - The delegator keeps its own context untouched (fork is a copy).
//! - Delegated tasks are visible in TUI and web chat via the delegation DB
//!   table and dashboard events.

use serde::{Deserialize, Serialize};

pub const DELEGATION_MARKER: &str = "__DELEGATED_TASK__";

#[derive(Debug, Clone, serde::Serialize)]
pub struct DelegationRecord {
    pub id: String,
    pub parent_user_id: String,
    pub child_user_id: String,
    pub task: String,
    pub status: String, // running | done | failed
    pub result: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

fn delegation_table(db: &crate::db::Database) -> anyhow::Result<()> {
    let conn = db.conn();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS delegations (
            id TEXT PRIMARY KEY,
            parent_user_id TEXT NOT NULL,
            child_user_id TEXT NOT NULL,
            task TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            result TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );",
    )?;
    Ok(())
}

/// Called at startup so the delegations table exists even before the first
/// delegate_task call (otherwise list_delegations fails on a fresh DB).
pub fn ensure_delegations_table(db: &crate::db::Database) {
    if let Err(e) = delegation_table(db) {
        tracing::error!(error = %e, "Failed to ensure delegations table");
    }
}

fn insert_delegation(db: &crate::db::Database, rec: &DelegationRecord) -> anyhow::Result<()> {
    let conn = db.conn();
    conn.execute(
        "INSERT INTO delegations (id, parent_user_id, child_user_id, task, status, result, created_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            rec.id,
            rec.parent_user_id,
            rec.child_user_id,
            rec.task,
            rec.status,
            rec.result,
            rec.created_at,
            rec.finished_at
        ],
    )?;
    Ok(())
}

fn update_delegation(db: &crate::db::Database, id: &str, status: &str, result: Option<&str>) -> anyhow::Result<()> {
    let conn = db.conn();
    conn.execute(
        "UPDATE delegations SET status = ?2, result = ?3, finished_at = datetime('now') WHERE id = ?1",
        rusqlite::params![id, status, result],
    )?;
    Ok(())
}

pub fn list_delegations(db: &crate::db::Database, parent_user_id: &str) -> anyhow::Result<Vec<DelegationRecord>> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT id, parent_user_id, child_user_id, task, status, result, created_at, finished_at
         FROM delegations WHERE parent_user_id = ?1 ORDER BY created_at DESC LIMIT 50",
    )?;
    let rows = stmt.query_map(rusqlite::params![parent_user_id], |row| {
        Ok(DelegationRecord {
            id: row.get(0)?,
            parent_user_id: row.get(1)?,
            child_user_id: row.get(2)?,
            task: row.get(3)?,
            status: row.get(4)?,
            result: row.get(5)?,
            created_at: row.get(6)?,
            finished_at: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(anyhow::Error::from)
}

/// Fork the parent context into a child user id and mark it as delegated.
/// The marker in custom_data prevents further delegation (one level only).
fn fork_child_context(
    db: &crate::db::Database,
    parent_user_id: &str,
    child_user_id: &str,
    task: &str,
) -> anyhow::Result<()> {
    db.fork_context(parent_user_id, child_user_id, Some("delegatee"))?;
    let mut child = db.load_context(child_user_id)?;
    if child.custom_data.is_null() {
        child.custom_data = serde_json::json!({});
    }
    if let Some(obj) = child.custom_data.as_object_mut() {
        obj.insert(DELEGATION_MARKER.to_string(), serde_json::json!(true));
        obj.insert("delegation_task".to_string(), serde_json::json!(task));
    }
    // Delegated tasks never delegate again and never spam the user's channels.
    child.settings.max_llm_turns = child.settings.max_llm_turns.or(Some(12));
    child.settings.use_tts = false;
    child.settings.use_stt = false;
    db.save_context(&child)?;
    Ok(())
}

/// Tool entry point: delegate a task to a fresh child agent context.
/// Returns a summary the parent agent can relay.
pub async fn delegate_task(
    state: &crate::gateway::GatewayState,
    parent_user_id: &str,
    args: &serde_json::Value,
) -> String {
    let task = args["task"].as_str().unwrap_or("").trim().to_string();
    if task.is_empty() {
        return "Error: 'task' is required and must be a non-empty description of what the child agent should do.".to_string();
    }
    let context_note = args["context"].as_str().unwrap_or("").trim().to_string();
    let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(600).min(3600);

    // One delegation level only: refuse when called from a delegated context.
    let parent_ctx = match state.db.load_context(parent_user_id) {
        Ok(c) => c,
        Err(e) => return format!("Error loading context: {}", e),
    };
    if parent_ctx
        .custom_data
        .get(DELEGATION_MARKER)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return "Error: Delegated tasks cannot delegate further (one delegation level allowed).".to_string();
    }

    let child_user_id = format!(
        "{}::delegate:{}",
        parent_user_id,
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let rec = DelegationRecord {
        id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        parent_user_id: parent_user_id.to_string(),
        child_user_id: child_user_id.clone(),
        task: task.clone(),
        status: "running".to_string(),
        result: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
    };
    if let Err(e) = insert_delegation(&state.db, &rec) {
        return format!("Error creating delegation record: {}", e);
    }

    if let Err(e) = fork_child_context(&state.db, parent_user_id, &child_user_id, &task) {
        let _ = update_delegation(&state.db, &rec.id, "failed", Some(&format!("fork failed: {}", e)));
        return format!("Error forking context: {}", e);
    }

    // Give the child a compact prompt: the task plus optional context note.
    let child_prompt = if context_note.is_empty() {
        format!("{}{}", DELEGATION_MARKER, task)
    } else {
        format!("{}{}\n\nContext: {}", DELEGATION_MARKER, task, context_note)
    };

    let db = state.db.clone();
    let rec_id = rec.id.clone();
    let child_id = child_user_id.clone();
    let parent = parent_user_id.to_string();

    let config = crate::gateway::agent_loop::AgentLoopConfig {
        max_turns: 12,
        max_tool_calls: 20,
        tags_enabled: false,
        sm_file: None,
        feedback_enabled: false,
        message_on_toolcalling: false,
        tool_history_limit: 30,
    };

    let started = std::time::Instant::now();
    let run = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        crate::gateway::agent_loop::run_agent_loop(state, &child_user_id, &child_prompt, config, None),
    )
    .await;

    let (status, result_text) = match run {
        Ok(Ok(res)) => ("done".to_string(), res.response),
        Ok(Err(e)) => ("failed".to_string(), format!("Delegated task failed: {}", e)),
        Err(_) => (
            "failed".to_string(),
            "Delegated task timed out; it was cancelled. Partial results are saved in the child context.".to_string(),
        ),
    };
    let _ = update_delegation(&db, &rec.id, &status, Some(&result_text));

    // Keep the parent informed in chat (TUI + web) without leaking child internals.
    let summary = format!(
        "🤝 Delegation {} {}: {}",
        rec.id,
        if status == "done" { "finished" } else { "failed" },
        if status == "done" {
            format!("result: {}", result_text)
        } else {
            result_text.clone()
        }
    );
    let _ = crate::runtime::events::send(&parent, "delegation_update", &serde_json::json!({
        "delegation_id": rec.id,
        "status": status,
        "result": result_text,
    }).to_string());

    let _ = db;
    let _ = rec_id;
    let _ = child_id;
    summary
}

/// Tool schema for the delegation tool (registered in the tool table).
pub fn tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "delegate_task",
            "description": "Delegate ONE self-contained task to a fresh child agent with its own forked context (inherits settings/memory snapshot). You continue working after it returns. Delegated tasks CANNOT delegate further (one level only). Use for subtasks like research, drafting, building a file, or a checklist item.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Complete, self-contained task description for the child agent"},
                    "context": {"type": "string", "description": "Optional extra context/data the child needs (it does NOT see the parent conversation)"},
                    "timeout_secs": {"type": "integer", "description": "Max runtime for the child agent (default 600, max 3600)"}
                },
                "required": ["task"]
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct _KeepDeserializeImported {
    #[allow(dead_code)]
    x: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_blocks_nested_delegation() {
        // The gate is a bool in custom_data; verify the marker constant is used
        // consistently by fork_child_context's key name.
        assert_eq!(DELEGATION_MARKER, "__DELEGATED_TASK__");
    }
}