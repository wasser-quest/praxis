use super::Database;
use serde::{Deserialize, Serialize};

pub const DEFAULT_HISTORY_TOKENS: usize = 32_000;

#[cfg(test)]
#[path = "history_budget_tests.rs"]
mod history_budget_tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallData {
    pub id: String,
    pub function: FunctionCallData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallData {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Dashboard-only metadata; audio bytes are fetched separately, never sent to the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_mime: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<serde_json::Value>>,
    /// Discord mirror metadata (direction/author/channel_id/channel_kind).
    /// Only set for mirrored Discord messages; never sent to the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_meta: Option<serde_json::Value>,
    /// Per-message provider-reported token usage (assistant messages only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    /// Wall-clock milliseconds for model generation (excluding tool execution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
}

/// Roles that only exist for the dashboard/TUI chat mirror and must never be
/// replayed into the LLM conversation history.
pub const MIRROR_ROLES: [&str; 2] = ["discord_user", "discord_bot"];

impl Message {
    fn empty_meta() -> Option<serde_json::Value> {
        None
    }

    pub fn user(content: String) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "user".to_string(),
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: Self::empty_meta(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "assistant".to_string(),
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: Self::empty_meta(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    pub fn assistant_with_usage(
        content: String,
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        total_tokens: Option<u32>,
        generation_ms: Option<u64>,
    ) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "assistant".to_string(),
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: Self::empty_meta(),
            prompt_tokens,
            completion_tokens,
            total_tokens,
            generation_ms,
        }
    }

    pub fn assistant_with_tool_calls(content: String, tool_calls: Vec<ToolCallData>) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "assistant".to_string(),
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: Some(tool_calls),
            content_parts: None,
            discord_meta: Self::empty_meta(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    pub fn tool(content: String, tool_call_id: String) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "tool".to_string(),
            content,
            tool_call_id: Some(tool_call_id),
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: Self::empty_meta(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    pub fn tool_with_image(
        content: String,
        tool_call_id: String,
        content_parts: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: "tool".to_string(),
            content,
            tool_call_id: Some(tool_call_id),
            tool_name: None,
            tool_calls: None,
            content_parts: Some(content_parts),
            discord_meta: Self::empty_meta(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    /// A mirrored Discord chat message (dashboard/TUI display only).
    /// `direction` is "user" or "bot".
    pub fn discord_mirror(
        content: String,
        direction: &str,
        author: &str,
        channel_id: &str,
        channel_kind: &str,
    ) -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: if direction == "bot" {
                "discord_bot".to_string()
            } else {
                "discord_user".to_string()
            },
            content,
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: Some(serde_json::json!({
                "direction": direction,
                "author": author,
                "channel_id": channel_id,
                "channel_kind": channel_kind,
            })),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    /// True when this row is a Discord-mirror message (not LLM history).
    pub fn is_discord_mirror(&self) -> bool {
        MIRROR_ROLES.contains(&self.role.as_str()) || self.discord_meta.is_some()
    }
}

pub fn estimate_message_tokens(msg: &Message) -> usize {
    let mut tokens = 4usize;
    tokens += msg.role.len() / 4 + 1;
    tokens += msg.content.len() / 4 + 1;
    if let Some(ref tcid) = msg.tool_call_id {
        tokens += tcid.len() / 4 + 1;
    }
    if let Some(ref tool_calls) = msg.tool_calls {
        for tc in tool_calls {
            tokens += 4;
            tokens += tc.id.len() / 4 + 1;
            tokens += tc.function.name.len() / 4 + 1;
            tokens += tc.function.arguments.len() / 4 + 1;
        }
    }
    tokens
}

impl Database {
    pub(crate) fn resolve_user_key(&self, conn: &rusqlite::Connection, user_id: &str) -> String {
        if let Ok(data) = conn.query_row(
            "SELECT data FROM contexts WHERE user_id = ?1",
            rusqlite::params![user_id],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(ctx) = serde_json::from_str::<super::contexts::Context>(&data) {
                if !ctx.session_id.is_empty() && ctx.session_id != "default" {
                    return format!("{}:::{}", user_id, ctx.session_id);
                }
            }
        }
        user_id.to_string()
    }

    pub fn add_message(&self, user_id: &str, msg: &Message) -> anyhow::Result<i64> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let tool_calls_json = msg
            .tool_calls
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default());
        let content_parts_json = msg
            .content_parts
            .as_ref()
            .map(|cp| serde_json::to_string(cp).unwrap_or_default());
        let discord_meta_json = msg
            .discord_meta
            .as_ref()
            .map(|dm| serde_json::to_string(dm).unwrap_or_default());
        conn.execute(
            "INSERT INTO messages (user_id, role, content, tool_call_id, tool_name, tool_calls, content_parts, discord_meta, prompt_tokens, completion_tokens, total_tokens, generation_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![key, msg.role, msg.content, msg.tool_call_id, msg.tool_name, tool_calls_json, content_parts_json, discord_meta_json, msg.prompt_tokens, msg.completion_tokens, msg.total_tokens, msg.generation_ms],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Save speech on the original reply, before notifying SSE subscribers. A
    /// disconnected dashboard can recover it from history without regenerating TTS.
    pub fn save_message_audio(
        &self,
        user_id: &str,
        message_id: i64,
        mime: &str,
        audio: &[u8],
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(!audio.is_empty(), "Empty TTS audio");
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        Ok(conn.execute(
            "UPDATE messages SET audio = ?1, audio_mime = ?2 WHERE user_id = ?3 AND id = ?4 AND role = 'assistant'",
            rusqlite::params![audio, mime, key, message_id],
        )? == 1)
    }

    pub fn get_message_audio(
        &self,
        user_id: &str,
        message_id: i64,
    ) -> anyhow::Result<Option<(String, Vec<u8>)>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        Ok(conn.query_row(
            "SELECT audio_mime, audio FROM messages WHERE user_id = ?1 AND id = ?2 AND audio IS NOT NULL",
            rusqlite::params![key, message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?)
    }

    pub fn get_messages(&self, user_id: &str, limit: i32) -> anyhow::Result<Vec<Message>> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let mut stmt = conn.prepare(
            "SELECT role, content, tool_call_id, tool_name, tool_calls, content_parts, discord_meta, id, prompt_tokens, completion_tokens, total_tokens, generation_ms FROM messages WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2"
        )?;

        let messages = stmt
            .query_map(rusqlite::params![key, limit], |row| {
                let tool_calls_json: Option<String> = row.get(4)?;
                let tool_calls: Option<Vec<ToolCallData>> =
                    tool_calls_json.and_then(|json| serde_json::from_str(&json).ok());
                let content_parts_json: Option<String> = row.get(5)?;
                let content_parts: Option<Vec<serde_json::Value>> =
                    content_parts_json.and_then(|json| serde_json::from_str(&json).ok());
                let discord_meta_json: Option<String> = row.get(6)?;
                let discord_meta: Option<serde_json::Value> =
                    discord_meta_json.and_then(|json| serde_json::from_str(&json).ok());
                Ok(Message {
                    id: Some(row.get(7)?),
                    audio_mime: None,
                    role: row.get(0)?,
                    content: row.get(1)?,
                    tool_call_id: row.get(2)?,
                    tool_name: row.get(3)?,
                    tool_calls,
                    content_parts,
                    discord_meta,
                    prompt_tokens: row.get(8)?,
                    completion_tokens: row.get(9)?,
                    total_tokens: row.get(10)?,
                    generation_ms: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(messages.into_iter().rev().collect())
    }

    pub fn get_messages_with_token_budget(
        &self,
        user_id: &str,
        token_budget: usize,
    ) -> anyhow::Result<(Vec<Message>, usize)> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let mut stmt = conn.prepare(
            "SELECT role, content, tool_call_id, tool_name, tool_calls, content_parts, discord_meta, id, prompt_tokens, completion_tokens, total_tokens, generation_ms FROM messages WHERE user_id = ?1 ORDER BY id DESC",
        )?;

        let mut messages = Vec::new();
        let mut total_tokens = 0usize;
        let rows = stmt.query_map(rusqlite::params![key], |row| {
            let tool_calls_json: Option<String> = row.get(4)?;
            let tool_calls: Option<Vec<ToolCallData>> =
                tool_calls_json.and_then(|json| serde_json::from_str(&json).ok());
            let content_parts_json: Option<String> = row.get(5)?;
            let content_parts: Option<Vec<serde_json::Value>> =
                content_parts_json.and_then(|json| serde_json::from_str(&json).ok());
            let discord_meta_json: Option<String> = row.get(6)?;
            let discord_meta: Option<serde_json::Value> =
                discord_meta_json.and_then(|json| serde_json::from_str(&json).ok());
            Ok(Message {
                id: Some(row.get(7)?),
                audio_mime: None,
                role: row.get(0)?,
                content: row.get(1)?,
                tool_call_id: row.get(2)?,
                tool_name: row.get(3)?,
                tool_calls,
                content_parts,
                discord_meta,
                prompt_tokens: row.get(8)?,
                completion_tokens: row.get(9)?,
                total_tokens: row.get(10)?,
                generation_ms: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
            })
        })?;

        // Budget whole user turns, not individual rows: a tool receipt must
        // never be detached from the assistant call that produced it. Keep the
        // current turn intact even if oversized; provider preflight must fit it
        // against the actual complete-request context, not silently lose input.
        let mut turn = Vec::new();
        let mut turn_tokens = 0usize;
        for row in rows {
            let msg = row?;
            if msg.is_discord_mirror() { continue; }
            turn_tokens = turn_tokens.saturating_add(estimate_message_tokens(&msg));
            let boundary = msg.role == "user";
            turn.push(msg);
            if boundary {
                if !messages.is_empty() && total_tokens.saturating_add(turn_tokens) > token_budget {
                    turn.clear();
                    break;
                }
                total_tokens = total_tokens.saturating_add(turn_tokens);
                messages.append(&mut turn);
                turn_tokens = 0;
                if total_tokens >= token_budget { break; }
            }
        }
        if !turn.is_empty() && (messages.is_empty() || total_tokens.saturating_add(turn_tokens) <= token_budget) {
            total_tokens = total_tokens.saturating_add(turn_tokens);
            messages.append(&mut turn);
        }

        messages.reverse();
        Ok((messages, total_tokens))
    }

    /// Full chat transcript for the dashboard/TUI: includes Discord-mirror
    /// rows (they are display-only but must appear in the chat history).
    pub fn get_chat_messages_with_token_budget(
        &self,
        user_id: &str,
        token_budget: usize,
    ) -> anyhow::Result<(Vec<Message>, usize)> {
        self.get_chat_messages_after(user_id, token_budget, 0)
    }

    /// Chat transcript with an `after_id` filter: rows with id <= after_id are
    /// skipped. Used by the dashboard chat so `/clear` can hide everything up
    /// to the clear marker while the Messages tab keeps showing the full
    /// history (messages are never deleted by a chat clear).
    pub fn get_chat_messages_after(
        &self,
        user_id: &str,
        token_budget: usize,
        after_id: i64,
    ) -> anyhow::Result<(Vec<Message>, usize)> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let mut stmt = conn.prepare(
            "SELECT id, role, content, tool_call_id, tool_name, tool_calls, content_parts, discord_meta, audio_mime, prompt_tokens, completion_tokens, total_tokens, generation_ms FROM messages WHERE user_id = ?1 ORDER BY id DESC",
        )?;

        let mut messages = Vec::new();
        let mut total_tokens = 0usize;
        let rows = stmt.query_map(rusqlite::params![key], |row| {
            let id: i64 = row.get(0)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            let tool_calls: Option<Vec<ToolCallData>> =
                tool_calls_json.and_then(|json| serde_json::from_str(&json).ok());
            let content_parts_json: Option<String> = row.get(6)?;
            let content_parts: Option<Vec<serde_json::Value>> =
                content_parts_json.and_then(|json| serde_json::from_str(&json).ok());
            let discord_meta_json: Option<String> = row.get(7)?;
            let discord_meta: Option<serde_json::Value> =
                discord_meta_json.and_then(|json| serde_json::from_str(&json).ok());
            Ok((id, Message {
                id: Some(id),
                audio_mime: row.get(8)?,
                role: row.get(1)?,
                content: row.get(2)?,
                tool_call_id: row.get(3)?,
                tool_name: row.get(4)?,
                tool_calls,
                content_parts,
                discord_meta,
                prompt_tokens: row.get(9)?,
                completion_tokens: row.get(10)?,
                total_tokens: row.get(11)?,
                generation_ms: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
            }))
        })?;

        for row in rows {
            let (id, msg) = row?;
            if id <= after_id {
                continue;
            }
            let tokens = estimate_message_tokens(&msg);
            if total_tokens + tokens > token_budget && !messages.is_empty() {
                break;
            }
            total_tokens += tokens;
            messages.push(msg);
        }

        messages.reverse();
        Ok((messages, total_tokens))
    }

    /// Highest message id for a user (0 when the table is empty). Used as the
    /// clear marker: everything at or below it stays in the Messages tab but
    /// is hidden from the chat view.
    pub fn max_message_id(&self, user_id: &str) -> anyhow::Result<i64> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let val: Option<i64> = conn
            .query_row(
                "SELECT MAX(id) FROM messages WHERE user_id = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .unwrap_or(None);
        Ok(val.unwrap_or(0))
    }

    pub fn count_messages(&self, user_id: &str) -> anyhow::Result<usize> {
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE user_id = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Compaction must keep the original rows: delete/reinsert changes reply
    /// identities and discards their saved audio (including still-running TTS).
    /// Atomically replace summarized rows with a handoff. CAS the prior summary
    /// and session so a concurrent manual clear/switch cannot erase new work.
    pub fn commit_compaction(&self, user: &str, session: &str, previous: &str, summary: &str, prefix: &[Message]) -> anyhow::Result<()> {
        anyhow::ensure!(!summary.trim().is_empty() && !prefix.is_empty(), "Empty compaction");
        let ids: Vec<i64> = prefix.iter().filter_map(|m|m.id).collect();
        anyhow::ensure!(ids.len()==prefix.len(), "Compaction requires saved message IDs");
        let mut conn=self.conn();
        let tx=conn.transaction()?;
        let key=self.resolve_user_key(&tx,user);
        let raw:String=tx.query_row("SELECT data FROM contexts WHERE user_id=?1",[&key],|r|r.get(0))?;
        let ctx:crate::db::contexts::Context=serde_json::from_str(&raw)?;
        anyhow::ensure!(ctx.session_id==session && ctx.settings.compaction_summary==previous,"Session/summary changed during compaction");
        let ids=serde_json::to_string(&ids)?;
        let count:i64=tx.query_row("SELECT count(*) FROM messages WHERE user_id=?1 AND id IN (SELECT value FROM json_each(?2))",rusqlite::params![key,ids],|r|r.get(0))?;
        anyhow::ensure!(count as usize==prefix.len(),"History changed during compaction");
        tx.execute("UPDATE contexts SET data=json_set(data,'$.settings.compaction_summary',?2),updated_at=datetime('now') WHERE user_id=?1",rusqlite::params![key,summary])?;
        let updated:String=tx.query_row("SELECT data FROM contexts WHERE user_id=?1",[&key],|r|r.get(0))?;
        let change=super::context_events::record(&tx,&key,Some(&raw),&updated,"compaction")?;
        tx.execute("DELETE FROM messages WHERE user_id=?1 AND id IN (SELECT value FROM json_each(?2)) AND role NOT IN ('discord_user','discord_bot')",rusqlite::params![key,ids])?;
        tx.commit()?;
        if let Some(change)=change { crate::runtime::events::send(user,"context_changed",&super::context_events::live_notice(&change)); }
        Ok(())
    }

    pub fn retain_chat_messages(&self, user_id: &str, messages: &[Message]) -> anyhow::Result<()> {
        let ids: Vec<_> = messages.iter().filter_map(|m| m.id).collect();
        anyhow::ensure!(ids.len() == messages.len(), "Cannot retain messages without DB ids");
        let conn = self.conn();
        let key = self.resolve_user_key(&conn, user_id);
        conn.execute(
            "DELETE FROM messages WHERE user_id = ?1 AND id NOT IN (SELECT value FROM json_each(?2))",
            rusqlite::params![key, serde_json::to_string(&ids)?],
        )?;
        Ok(())
    }

    pub fn clear_messages(&self, user_id: &str) -> anyhow::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let key = self.resolve_user_key(&tx, user_id);
        let session = super::tool_outputs::active_session(&tx, user_id)?;
        super::tool_outputs::clear_session(&tx, user_id, &session)?;
        tx.execute("DELETE FROM messages WHERE user_id = ?1", rusqlite::params![key])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn compaction_commit_updates_only_the_active_session_handoff() {
        let (db,_dir)=test_db();
        let mut ctx=db.load_context("user1").unwrap();
        ctx.session_id="other-session".into();
        db.save_context(&ctx).unwrap();
        db.add_message("user1",&Message::user("old goal".into())).unwrap();
        db.add_message("user1",&Message::assistant("old verified result".into())).unwrap();
        let prefix=db.get_messages("user1",100).unwrap();
        db.add_message("user1",&Message::user("current task".into())).unwrap();
        db.commit_compaction("user1","other-session","","verified handoff",&prefix).unwrap();
        assert_eq!(db.load_context("user1").unwrap().settings.compaction_summary,"verified handoff");
        assert_eq!(db.load_session_context("user1","default").unwrap().settings.compaction_summary,"");
        assert_eq!(db.get_messages("user1",100).unwrap()[0].content,"current task");
    }

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
        (db, dir)
    }

    fn ensure_context(db: &Database, user_id: &str) {
        let ctx = crate::db::contexts::Context {
            user_id: user_id.to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();
    }

    #[test]
    fn test_add_and_get_message() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        let msg = Message::user("Hello".to_string());
        let id = db.add_message("user1", &msg).unwrap();
        assert!(id > 0);

        let messages = db.get_messages("user1", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello");
    }

    #[test]
    fn audio_persists_with_stable_reply_ids_and_not_in_llm_history() {
        let (db, dir) = test_db();
        ensure_context(&db, "user1");
        let first = db.add_message("user1", &Message::assistant("same text".into())).unwrap();
        let second = db.add_message("user1", &Message::assistant("same text".into())).unwrap();
        assert!(second > first, "add_message must return row id, not rows affected");
        assert!(db.save_message_audio("user1", second, "audio/wav", b"synthetic audio").unwrap());
        drop(db);

        let db = Database::new(dir.path()).unwrap(); // migration is idempotent; bytes survive restart
        let (chat, _) = db.get_chat_messages_with_token_budget("user1", usize::MAX).unwrap();
        assert_eq!(chat[0].id, Some(first));
        assert_eq!(chat[0].audio_mime, None);
        assert_eq!(chat[1].id, Some(second));
        assert_eq!(chat[1].audio_mime.as_deref(), Some("audio/wav"));
        assert_eq!(db.get_message_audio("user1", second).unwrap(), Some(("audio/wav".into(), b"synthetic audio".to_vec())));
        let (llm, _) = db.get_messages_with_token_budget("user1", usize::MAX).unwrap();
        assert!(llm.iter().all(|m| m.audio_mime.is_none()));
        assert!(!serde_json::to_string(&chat).unwrap().contains("synthetic audio"));
        assert!(db.get_chat_messages_after("user1", usize::MAX, second).unwrap().0.is_empty());
        assert!(db.get_message_audio("user1", second).unwrap().is_some(), "chat-only clear keeps audio");
        db.retain_chat_messages("user1", &chat[1..]).unwrap();
        let retained = db.get_messages("user1", 10).unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].id, Some(second), "compaction preserves reply identity");
        assert!(db.get_message_audio("user1", second).unwrap().is_some(), "retained replies keep their audio");
        assert!(db.retain_chat_messages("user1", &[Message::assistant("unsaved".into())]).is_err());
        db.clear_messages("user1").unwrap();
        assert!(db.get_message_audio("user1", second).unwrap().is_none());
    }

    #[test]
    fn audio_is_scoped_to_user_session_and_existing_assistant_rows() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        ensure_context(&db, "user2");
        let id = db.add_message("user1", &Message::assistant("reply".into())).unwrap();
        let user_msg = db.add_message("user1", &Message::user("question".into())).unwrap();
        assert!(!db.save_message_audio("user2", id, "audio/mpeg", b"bytes").unwrap());
        assert!(!db.save_message_audio("user1", user_msg, "audio/mpeg", b"bytes").unwrap());
        assert!(!db.save_message_audio("user1", 99999, "audio/mpeg", b"bytes").unwrap());
        assert!(db.save_message_audio("user1", id, "audio/mpeg", b"").is_err());
        assert!(db.save_message_audio("user1", id, "audio/mpeg", b"bytes").unwrap());
        assert!(db.get_message_audio("user2", id).unwrap().is_none());
        let mut ctx = db.load_context("user1").unwrap();
        ctx.session_id = "other-session".into();
        db.save_context(&ctx).unwrap();
        assert!(db.get_message_audio("user1", id).unwrap().is_none());
        assert!(!db.save_message_audio("user1", id, "audio/wav", b"late audio").unwrap());
        ctx.session_id = "default".into();
        db.save_context(&ctx).unwrap();
        assert!(db.get_message_audio("user1", id).unwrap().is_some());
        db.delete_context("user1").unwrap();
        assert!(db.get_message_audio("user1", id).unwrap().is_none());
    }

    #[test]
    fn test_get_messages_with_limit() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        for i in 0..5 {
            let msg = Message::user(format!("Message {}", i));
            db.add_message("user1", &msg).unwrap();
        }

        let messages = db.get_messages("user1", 3).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "Message 2");
    }

    #[test]
    fn test_clear_messages() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        let msg = Message::user("Hello".to_string());
        db.add_message("user1", &msg).unwrap();
        db.clear_messages("user1").unwrap();
        let messages = db.get_messages("user1", 10).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_clear_messages_is_per_user() {
        // /deletemessages deletes only the target user's rows; other users
        // (other pairings/sessions) keep their messages.
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        ensure_context(&db, "user2");
        db.add_message("user1", &Message::user("mine".into())).unwrap();
        db.add_message("user2", &Message::user("theirs".into())).unwrap();
        db.clear_messages("user1").unwrap();
        assert!(db.get_messages("user1", 10).unwrap().is_empty());
        let other = db.get_messages("user2", 10).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].content, "theirs");
    }

    #[test]
    fn test_message_order() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        for i in 0..3 {
            let msg = Message::user(format!("msg{}", i));
            db.add_message("user1", &msg).unwrap();
        }
        let messages = db.get_messages("user1", 10).unwrap();
        assert_eq!(messages[0].content, "msg0");
        assert_eq!(messages[2].content, "msg2");
    }

    #[test]
    fn test_message_with_tool_calls() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        let msg = Message::assistant_with_tool_calls(
            String::new(),
            vec![ToolCallData {
                id: "call_123".to_string(),
                function: FunctionCallData {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                },
            }],
        );
        db.add_message("user1", &msg).unwrap();

        let messages = db.get_messages("user1", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");
        let tool_calls = messages[0].tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].function.name, "read_file");
    }

    #[test]
    fn test_discord_mirror_persisted_and_history_filtered() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        db.add_message("user1", &Message::user("real user msg".into()))
            .unwrap();
        db.add_message(
            "user1",
            &Message::discord_mirror(
                "discord question".into(),
                "user",
                "Marvin",
                "12345",
                "dm",
            ),
        )
        .unwrap();
        db.add_message(
            "user1",
            &Message::discord_mirror("discord answer".into(), "bot", "bot", "12345", "dm"),
        )
        .unwrap();
        db.add_message("user1", &Message::assistant("real reply".into()))
            .unwrap();

        // Chat transcript (dashboard/TUI): mirror rows included with meta.
        let (chat, _) = db.get_chat_messages_with_token_budget("user1", usize::MAX).unwrap();
        assert_eq!(chat.len(), 4);
        assert_eq!(chat[1].role, "discord_user");
        assert_eq!(chat[1].is_discord_mirror(), true);
        let meta = chat[1].discord_meta.as_ref().unwrap();
        assert_eq!(meta["author"], "Marvin");
        assert_eq!(meta["channel_id"], "12345");
        assert_eq!(chat[2].role, "discord_bot");

        // LLM history: mirror rows filtered out, order preserved.
        let (history, _) = db.get_messages_with_token_budget("user1", usize::MAX).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "real user msg");
        assert_eq!(history[1].content, "real reply");
    }

    #[test]
    fn test_chat_clear_marker_hides_but_keeps_rows() {
        let (db, _dir) = test_db();
        ensure_context(&db, "user1");
        db.add_message("user1", &Message::user("old one".into())).unwrap();
        db.add_message("user1", &Message::assistant("old two".into())).unwrap();
        let marker = db.max_message_id("user1").unwrap();
        assert!(marker > 0);

        // New messages after the clear marker.
        db.add_message("user1", &Message::user("new one".into())).unwrap();
        db.add_message("user1", &Message::assistant("new two".into())).unwrap();

        // Chat view after /clear: only rows above the marker.
        let (chat, _) = db.get_chat_messages_after("user1", usize::MAX, marker).unwrap();
        assert_eq!(chat.len(), 2);
        assert_eq!(chat[0].content, "new one");
        assert_eq!(chat[1].content, "new two");

        // Full transcript (Messages tab): all four rows still there.
        let (full, _) = db.get_chat_messages_with_token_budget("user1", usize::MAX).unwrap();
        assert_eq!(full.len(), 4);
    }
}
