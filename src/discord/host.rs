//! Everything a channel takes from the host.
//!
//! The kernel implements `ChannelHost` once; the channel crate defines only
//! what it needs and never links kernel internals, database types or
//! credentials. This is the same boundary an installed channel package gets
//! (`docs/PLUGINIZATION_HANDOFF.md` §6D).
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::sync::{broadcast, mpsc};

/// The scoped credentials a channel may receive. Nothing else is exposed.
#[derive(Debug, Clone, Default)]
pub struct DiscordCredentials {
    pub bot_token: Option<String>,
    pub application_id: Option<String>,
    pub elevenlabs_api_key: Option<String>,
}

/// A completed pairing between a channel account and an internal user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub user_id: String,
    pub discord_user_id: String,
}

/// A message mirrored into the user's history from a channel.
#[derive(Debug, Clone)]
pub struct MirroredMessage {
    pub user_id: String,
    pub content: String,
    /// "user" or "bot": which side of the channel conversation it was.
    pub direction: &'static str,
    pub author: String,
    pub channel_id: String,
    /// "dm" or "guild".
    pub channel_kind: &'static str,
}

/// A skill a channel command may list or run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub skill_hidden: bool,
    pub user_only: bool,
}

/// One page of a skill listing or search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPage {
    pub skills: Vec<SkillSummary>,
    pub next_after: Option<String>,
}

/// The host's context record as wire JSON. The channel reads fields and hands
/// the whole record back to save — exactly what a remote client does.
pub type ContextJson = Value;

/// The channel's outbound delivery feed: what host tools and the agent
/// produce for the connected account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelEvent {
    FileUpload {
        user_id: String,
        file_path: String,
        file_name: String,
        channel_id: String,
    },
    AgentFeedback {
        user_id: String,
        message: String,
    },
    ChannelMessage {
        user_id: String,
        channel_id: String,
        message: String,
    },
    ChannelEmbed {
        user_id: String,
        channel_id: String,
        embed: Value,
    },
    VoiceTts {
        user_id: String,
        audio_data: Vec<u8>,
    },
}

/// Everything the channel takes from the host. A minimal kernel can implement
/// only what it runs; the channel fails closed on unsupported operations.
#[async_trait]
pub trait ChannelHost: Send + Sync + 'static {
    // ── pairing and identity ──────────────────────────────────────────────
    fn pairing_by_discord(&self, discord_user: &str) -> anyhow::Result<Option<Pairing>>;
    fn pairing_by_user(&self, user: &str) -> anyhow::Result<Option<Pairing>>;
    fn list_pairings(&self) -> anyhow::Result<Vec<Pairing>>;
    /// Stage a pairing code for `/pair` (expires RFC 3339).
    fn create_pending_pairing(&self, code: &str, discord_user: &str, expires_at: &str)
        -> anyhow::Result<()>;
    fn create_pairing(
        &self,
        user: &str,
        discord_user: &str,
        guild: Option<&str>,
    ) -> anyhow::Result<()>;

    // ── context and sessions ──────────────────────────────────────────────
    fn load_context(&self, user: &str) -> anyhow::Result<ContextJson>;
    fn save_context(&self, user: &str, context: &ContextJson) -> anyhow::Result<()>;
    fn merge_context(&self, user: &str, updates: Value) -> anyhow::Result<ContextJson>;
    fn list_sessions(&self, user: &str) -> anyhow::Result<Vec<(String, String)>>;
    fn create_session(&self, user: &str, session: &str, name: Option<&str>) -> anyhow::Result<()>;
    fn rename_session(&self, user: &str, session: &str, name: &str) -> anyhow::Result<()>;
    fn delete_session(&self, user: &str, session: &str) -> anyhow::Result<()>;
    /// Execute a host context command (`/context …`). `Err` is the parse
    /// failure so a channel can show its own examples.
    fn context_command(&self, user: &str, line: &str) -> Result<String, String>;

    // ── history ───────────────────────────────────────────────────────────
    fn mirror_message(&self, message: MirroredMessage) -> anyhow::Result<()>;
    fn clear_messages(&self, user: &str) -> anyhow::Result<()>;
    /// Clear one session's messages (a channel `/clear` variant).
    fn clear_session_messages(&self, user: &str, session: &str) -> anyhow::Result<()>;

    // ── agent control and interactions ────────────────────────────────────
    async fn stop_agent(&self, user: &str);
    /// Input sender of a running task's open question, if it is asking.
    async fn question_sender(&self, user: &str) -> Option<mpsc::UnboundedSender<String>>;
    /// An interactive question answered by a channel message; true if one was.
    async fn interaction_reply(&self, channel: &str, content: &str, user: &str) -> bool;
    /// An interactive question answered by a reaction.
    async fn interaction_reaction(&self, channel: &str, emoji: &str, user: &str);

    // ── delivery and events ───────────────────────────────────────────────
    /// The channel's outbound feed (files, feedback, messages, embeds, TTS).
    fn events(&self) -> broadcast::Receiver<ChannelEvent>;
    /// Mirror an event into the dashboard stream.
    fn dashboard_event(&self, user: &str, kind: &str, payload: &str);

    // ── skills and tools ──────────────────────────────────────────────────
    /// Whether a tool is enabled (skill commands gate on `use_skill`).
    fn tool_enabled(&self, name: &str) -> anyhow::Result<bool>;
    /// Skills for channel commands in a directory of skill assets.
    fn skill_search(&self, directory: &Path, query: &str) -> anyhow::Result<SkillPage>;
    fn skill_browse(&self, directory: &Path, cursor: &str) -> anyhow::Result<SkillPage>;
    /// The registered skill catalog for channel commands.
    fn registered_skills(&self) -> Vec<SkillSummary>;
    /// Whether a named skill is registered (activation validates first).
    fn registered_skill(&self, name: &str) -> bool;
    /// Whether a named skill exists (activation validates before writing).
    fn skill_exists(&self, directory: &Path, name: &str) -> anyhow::Result<()>;
    /// The installation root (skill assets and local storage live under it).
    fn root(&self) -> &Path;
}
