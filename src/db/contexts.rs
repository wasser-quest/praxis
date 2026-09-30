use super::Database;
use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "sm_migration_tests.rs"]
mod sm_migration_tests;
#[cfg(test)]
#[path = "context_policy_tests.rs"]
mod context_policy_tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub user_id: String,
    #[serde(default)]
    pub turn: i32,
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Display name for the chat user. Distinct from `user_id`, which is the
    /// session identifier. Multiple sessions belonging to the same human user
    /// share a `username` while having different `user_id`s.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default, alias = "cl_file")]
    pub sm_file: Option<String>,
    #[serde(default)]
    pub active_state: Option<String>,
    #[serde(default)]
    pub active_templates: Vec<String>,
    #[serde(default)]
    pub settings: ContextSettings,
    #[serde(default)]
    pub custom_data: serde_json::Value,
    /// Statemachine-specific data; legacy cl_data is accepted on input only.
    /// Supports deep merge via dot-notation in set_context.
    #[serde(default, alias = "cl_data")]
    pub sm_data: serde_json::Value,
    /// Session identifier for multi-session support (e.g., "default", "session-2")
    #[serde(default)]
    pub session_id: String,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            turn: 0,
            mode: default_mode(),
            username: None,
            sm_file: None,
            active_state: None,
            active_templates: Vec::new(),
            settings: ContextSettings::default(),
            custom_data: serde_json::Value::Null,
            sm_data: serde_json::Value::Null,
            session_id: String::new(),
        }
    }
}

fn default_tag_prefix() -> String { crate::tags::DEFAULT_TAG_PREFIX.into() }

fn default_true() -> bool { true }

fn default_mode() -> String {
    "agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSettings {
    #[serde(default)]
    pub voice_enabled: bool,
    #[serde(default = "default_stt")]
    pub voice_stt_type: String,
    #[serde(default)]
    pub voice_vosk_model_path: Option<String>,
    /// Standard Vosk WebSocket server; bypasses local Vosk/model when set.
    #[serde(default)]
    pub voice_vosk_url: Option<String>,
    #[serde(default)]
    pub voice_whisper_model_path: Option<String>,
    #[serde(default)]
    pub voice_last_input: Option<String>,
    #[serde(default = "default_listen_timeout")]
    pub voice_listen_timeout_secs: i32,
    #[serde(default)]
    pub voice_owner_id: Option<String>,
    #[serde(default)]
    pub voice_tts_enabled: bool,
    #[serde(default = "default_tts")]
    pub voice_tts_type: String,
    /// Private ComfyUI origin. Unset fields inherit legacy custom_data/env defaults.
    #[serde(default)]
    pub comfyui_base_url: Option<String>,
    #[serde(default)]
    pub comfyui_tts_workflow: Option<String>,
    /// Shared speech mode: single (provider language) or de-ja (one mixed WAV).
    #[serde(default)]
    pub comfyui_tts_language_mode: Option<String>,
    #[serde(default)]
    pub comfyui_qwen_reference_audio: Option<String>,
    #[serde(default)]
    pub comfyui_qwen_reference_text: Option<String>,
    #[serde(default)]
    pub comfyui_qwen_language: Option<String>,
    /// Relative to ComfyUI's SERVER input directory, not a local Praxis path.
    #[serde(default)]
    pub comfyui_xtts_reference_audio: Option<String>,
    #[serde(default)]
    pub comfyui_xtts_language: Option<String>,
    #[serde(default)]
    pub comfyui_timeout_seconds: Option<usize>,
    #[serde(default)]
    pub voice_elevenlabs_voice_id: Option<String>,
    #[serde(default = "default_elevenlabs_stt_model")]
    pub elevenlabs_stt_model: String,
    #[serde(default)]
    pub elevenlabs_stt_language: Option<String>,
    #[serde(default)]
    pub elevenlabs_stt_tag_audio_events: bool,
    #[serde(default = "default_elevenlabs_stt_no_verbatim")]
    pub elevenlabs_stt_no_verbatim: bool,
    /// Minimum STT confidence (0.0..1.0) below which a transcript is flagged
    /// as low-confidence (dashboard warning + transcript-check rule).
    #[serde(default = "default_stt_low_confidence_threshold")]
    pub stt_low_confidence_threshold: f64,
    #[serde(default = "default_elevenlabs_tts_model")]
    pub elevenlabs_tts_model: String,
    #[serde(default = "default_elevenlabs_stability")]
    pub elevenlabs_stability: f32,
    #[serde(default = "default_elevenlabs_similarity_boost")]
    pub elevenlabs_similarity_boost: f32,
    #[serde(default)]
    pub elevenlabs_style: Option<f32>,
    #[serde(default)]
    pub elevenlabs_speed: Option<f32>,
    #[serde(default)]
    pub elevenlabs_tts_language: Option<String>,
    #[serde(default)]
    pub use_tts: bool,
    /// Independent web-reply synthesis/autoplay permission; use_tts cannot override OFF.
    /// Manual replay of already stored audio remains available.
    #[serde(default)]
    pub web_chat_tts: bool,
    #[serde(default = "default_true")]
    pub use_stt: bool,
    #[serde(default)]
    pub voice_muted: bool,
    #[serde(default = "default_deafened")]
    pub voice_deafened: bool,
    #[serde(default)]
    pub voice_discord_guild_id: Option<String>,
    #[serde(default)]
    pub rvc_on: bool,
    #[serde(default)]
    pub rvc_server: Option<String>,
    #[serde(default)]
    pub rvc_model_path: Option<String>,
    #[serde(default)]
    pub rvc_index_path: Option<String>,
    #[serde(default)]
    pub voice_audio_output_path: Option<String>,
    #[serde(default)]
    pub qwen_tts_server: Option<String>,
    #[serde(default)]
    pub qwen_tts_model: Option<String>,
    #[serde(default)]
    pub qwen_tts_speaker: Option<String>,
    #[serde(default)]
    pub qwen_tts_language: Option<String>,
    #[serde(default)]
    pub qwen_voice_clone_audio_path: Option<String>,
    #[serde(default)]
    pub qwen_voice_clone_enabled: bool,
    #[serde(default)]
    pub qwen_voice_clone_prompt: Option<String>,
    #[serde(default)]
    pub minimax_voice_id: Option<String>,
    #[serde(default)]
    pub minimax_tts_model: Option<String>,
    #[serde(default)]
    pub mimo_voice_id: Option<String>,
    #[serde(default)]
    pub mimo_tts_type: Option<String>,
    /// Optional MiMo TTS API base URL override. Falls back to the
    /// `MIMO_TTS_API_BASE` environment variable, then the built-in default.
    #[serde(default)]
    pub mimo_tts_base_url: Option<String>,
    /// Optional MiMo TTS model override. When null, the model is derived
    /// from `mimo_tts_type` (builtin/voicedesign/voiceclone).
    #[serde(default)]
    pub mimo_tts_model: Option<String>,
    #[serde(default)]
    pub history_with_toolcalls: bool,
    #[serde(default)]
    pub only_tool_calls_no_history: bool,
    #[serde(default)]
    pub download: bool,
    #[serde(default)]
    pub feedback_enabled: bool,
    #[serde(default = "default_feedback_max")]
    pub feedback_max_per_5min: i32,
    #[serde(default = "default_feedback_window")]
    pub feedback_window_secs: i32,
    #[serde(default = "default_allowed")]
    pub allowed_guilds: Vec<String>,
    #[serde(default = "default_allowed")]
    pub allowed_channels: Vec<String>,
    #[serde(default)]
    pub max_llm_turns: Option<i32>,
    #[serde(default)]
    pub max_tool_calls: Option<i32>,
    #[serde(default)]
    pub summarize_char_limit: Option<i32>,
    #[serde(default)]
    pub active_templates: Vec<String>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub path: String,
    #[serde(default, alias = "cl_file")]
    pub sm_file: Option<String>,
    #[serde(default)]
    pub active_state: Option<String>,
    #[serde(default)]
    pub current_template: String,
    #[serde(default)]
    pub tags_enabled: bool,
    /// Literal prefix for workflow/template commands; configured by the user.
    #[serde(default = "default_tag_prefix")]
    pub tag_prefix: String,
    /// Named decisions/*.json profile. None or "off" disables automatic routing.
    #[serde(default)]
    pub decision_profile: Option<String>,
    /// Explicit toggle for Decision Router. When false, routing is skipped regardless of decision_profile.
    #[serde(default = "default_true")]
    pub use_decision_router: bool,
    /// Toggle extended-reasoning output for the active model, per provider.
    /// "auto" (default) leaves provider defaults untouched.
    #[serde(default = "default_thinking_mode")]
    pub thinking_mode: String,
    /// Show provider-supplied reasoning separately; never use it as a final answer.
    #[serde(default)]
    pub show_thinking: bool,
    #[serde(default)]
    pub llm_turn: i32,
    #[serde(default)]
    pub compaction_enabled: bool,
    #[serde(default)]
    pub compaction_summary: String,
    #[serde(default)]
    pub history_token_limit: Option<usize>,
    /// Read-only legacy compatibility; no longer written to saved contexts.
    /// Per-call _output/read_tool_result replace destructive global clipping.
    #[serde(default, skip_serializing)]
    pub tool_result_limit: Option<usize>,
    /// Tools the current state activates. Each entry is a tool name (built-in or
    /// plugin) or a group name from `tool_group_definitions`, e.g.
    /// `["coding", "execute_terminal", "brave_web_search"]`. When non-empty this is
    /// the allow-list sent to the model; activated tools get full schemas.
    #[serde(default)]
    pub activated_tools: Vec<String>,
    /// Legacy: explicit tool names that receive full schemas. Merged into `activated_tools`.
    #[serde(default)]
    pub full_tool_schemas: Vec<String>,
    /// Tool categories that receive full parameter schemas. Options:
    /// Action, Discovery, SkillLoader, AgentControl, Memory, Context
    #[serde(default)]
    pub full_tool_categories: Vec<String>,
    /// Legacy: group names to activate. Merged into `activated_tools`.
    #[serde(default)]
    pub tool_groups: Option<Vec<String>>,
    /// Tool group definitions: group name -> members (tool names or other group
    /// names, including plugin tools). There are no built-in groups; a workflow
    /// `[tool_groups]` section is merged in on every routing pass and users may
    /// add groups via `set_context settings.tool_group_definitions.<name>`.
    #[serde(default)]
    pub tool_group_definitions: std::collections::HashMap<String, Vec<String>>,
    /// Mode for tool discovery in search_tools results.
    /// Full = full schemas, DescriptionOnly = name+desc only, None = no tools.
    #[serde(default)]
    pub tool_discovery_mode: String,
    #[serde(default)]
    pub compaction_token_limit: Option<usize>,
    #[serde(default)]
    pub compaction_template: Option<String>,
    /// When true, bypass the GPU router and use the pgpu free router endpoint
    /// (configured in pgpu as `/free/v1` on the dashboard port).
    #[serde(default)]
    pub use_freerouter: bool,
    #[serde(default)]
    pub system_template: Option<String>,
    #[serde(default)]
    pub active_skill: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub vision_provider: Option<String>,
    #[serde(default)]
    pub vision_model: Option<String>,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub voice_wake_words: Vec<String>,
    #[serde(default)]
    pub voice_auto_pause_enabled: bool,
    #[serde(default)]
    pub feedback_mode: Vec<String>,
    #[serde(default)]
    pub feedback_channel_id: Option<String>,
    /// Default Discord channel for uploads/messages when the tool call omits
    /// channel_id and no originating channel is known.
    #[serde(default)]
    pub upload_channel_id: Option<String>,
    #[serde(default)]
    pub message_on_toolcalling: bool,
    #[serde(default = "default_true")]
    pub vm_screenshot_enabled: bool,
    #[serde(default = "default_screenshot_limit")]
    pub vm_screenshot_limit: usize,
    #[serde(default = "default_keyboard_layout")]
    pub vm_keyboard_layout: String,
    #[serde(default = "default_tool_history_limit")]
    pub tool_history_limit: usize,
}

fn default_stt() -> String {
    "vosk".to_string()
}
fn default_tts() -> String {
    // ElevenLabs is the primary TTS engine (works headless on Linux).
    // windows_sapi is Windows-only and unavailable on this deployment.
    "elevenlabs".to_string()
}
fn default_listen_timeout() -> i32 {
    120
}
fn default_deafened() -> bool {
    true
}
fn default_feedback_max() -> i32 {
    10
}
fn default_feedback_window() -> i32 {
    300
}
fn default_allowed() -> Vec<String> {
    vec!["*".to_string()]
}
fn default_elevenlabs_stt_model() -> String {
    "scribe_v2".to_string()
}
fn default_elevenlabs_stt_no_verbatim() -> bool {
    true
}
fn default_stt_low_confidence_threshold() -> f64 {
    0.70
}
fn default_elevenlabs_tts_model() -> String {
    "eleven_multilingual_v2".to_string()
}
fn default_elevenlabs_stability() -> f32 {
    0.5
}
fn default_elevenlabs_similarity_boost() -> f32 {
    0.75
}
fn default_elevenlabs_speed() -> Option<f32> {
    Some(0.8)
}
fn default_thinking_mode() -> String {
    "auto".to_string()
}
fn default_screenshot_limit() -> usize {
    5000
}
fn default_keyboard_layout() -> String {
    "us".to_string()
}
fn default_tool_history_limit() -> usize {
    50
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            voice_enabled: false,
            voice_stt_type: default_stt(),
            voice_vosk_model_path: None,
            voice_vosk_url: None,
            voice_whisper_model_path: None,
            voice_last_input: None,
            voice_listen_timeout_secs: default_listen_timeout(),
            voice_owner_id: None,
            voice_tts_enabled: false,
            voice_tts_type: default_tts(),
            comfyui_base_url: None,
            comfyui_tts_workflow: None,
            comfyui_tts_language_mode: None,
            comfyui_qwen_reference_audio: None,
            comfyui_qwen_reference_text: None,
            comfyui_qwen_language: None,
            comfyui_xtts_reference_audio: None,
            comfyui_xtts_language: None,
            comfyui_timeout_seconds: None,
            voice_elevenlabs_voice_id: None,
            elevenlabs_stt_model: default_elevenlabs_stt_model(),
            elevenlabs_stt_language: None,
            elevenlabs_stt_tag_audio_events: false,
            elevenlabs_stt_no_verbatim: default_elevenlabs_stt_no_verbatim(),
            stt_low_confidence_threshold: default_stt_low_confidence_threshold(),
            elevenlabs_tts_model: default_elevenlabs_tts_model(),
            elevenlabs_stability: default_elevenlabs_stability(),
            elevenlabs_similarity_boost: default_elevenlabs_similarity_boost(),
            elevenlabs_style: None,
            elevenlabs_speed: default_elevenlabs_speed(),
            elevenlabs_tts_language: None,
            use_tts: false,
            use_stt: true,
            voice_muted: false,
            voice_deafened: default_deafened(),
            voice_discord_guild_id: None,
            web_chat_tts: false,
            rvc_on: false,
            rvc_server: None,
            rvc_model_path: None,
            rvc_index_path: None,
            voice_audio_output_path: None,
            qwen_tts_server: None,
            qwen_tts_model: None,
            qwen_tts_speaker: None,
            qwen_tts_language: None,
            qwen_voice_clone_audio_path: None,
            qwen_voice_clone_enabled: false,
            qwen_voice_clone_prompt: None,
            minimax_voice_id: None,
            minimax_tts_model: None,
            mimo_voice_id: None,
            mimo_tts_type: None,
            mimo_tts_base_url: None,
            mimo_tts_model: None,
            history_with_toolcalls: true,
            only_tool_calls_no_history: false,
            download: false,
            feedback_enabled: false,
            feedback_max_per_5min: default_feedback_max(),
            feedback_window_secs: default_feedback_window(),
            allowed_guilds: default_allowed(),
            allowed_channels: default_allowed(),
            max_llm_turns: None,
            max_tool_calls: None,
            summarize_char_limit: None,
            active_templates: Vec::new(),
            done: false,
            path: String::new(),
            sm_file: None,
            active_state: None,
            current_template: String::new(),
            tags_enabled: false,
            tag_prefix: default_tag_prefix(),
            decision_profile: None,
            thinking_mode: "auto".to_string(),
            show_thinking: false,
            llm_turn: 0,
            compaction_enabled: false,
            compaction_summary: String::new(),
            history_token_limit: None,
            tool_result_limit: None,
            compaction_token_limit: None,
            compaction_template: None,
            activated_tools: Vec::new(),
            full_tool_schemas: Vec::new(),
            full_tool_categories: Vec::new(),
            tool_groups: None,
            tool_group_definitions: std::collections::HashMap::new(),
            tool_discovery_mode: String::new(),
            use_freerouter: false,
            use_decision_router: true,
            system_template: None,
            active_skill: None,
            provider: None,
            model: None,
            vision_provider: None,
            vision_model: None,
            agent_name: "assistant".to_string(),
            voice_wake_words: vec!["*".to_string()],
            voice_auto_pause_enabled: false,
            feedback_mode: Vec::new(),
            feedback_channel_id: None,
            upload_channel_id: None,
            message_on_toolcalling: false,
            vm_screenshot_enabled: true,
            vm_screenshot_limit: default_screenshot_limit(),
            vm_keyboard_layout: default_keyboard_layout(),
            tool_history_limit: default_tool_history_limit(),
        }
    }
}

impl Database {
    pub fn load_context(&self, user_id: &str) -> anyhow::Result<Context> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT data FROM contexts WHERE user_id = ?1",
            rusqlite::params![user_id],
            |row| row.get::<_, String>(0),
        );

        let mut base_ctx = match result {
            Ok(data) => decode_context(&data)?,
            Err(rusqlite::Error::QueryReturnedNoRows) => Context {
                user_id: user_id.to_string(),
                ..Default::default()
            },
            Err(e) => return Err(e.into()),
        };

        // The lookup key, never a stale/edited JSON field, defines ownership.
        base_ctx.user_id = user_id.to_string();

        // If a non-default session is active, load the session-specific context
        if !base_ctx.session_id.is_empty() && base_ctx.session_id != "default" {
            let key = format!("{}:::{}", user_id, base_ctx.session_id);
            match conn.query_row(
                "SELECT data FROM contexts WHERE user_id = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            ) {
                Ok(data) => {
                    let mut session_ctx = decode_context(&data)?;
                    session_ctx.user_id = user_id.to_string();
                    session_ctx.session_id = base_ctx.session_id.clone();
                    return Ok(session_ctx);
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => return Err(e.into()),
            }
            // Session context doesn't exist yet; FORK from base
            let forked = base_ctx.clone();
            let forked_json = serde_json::to_string(&forked)?;
            conn.execute(
                "INSERT OR REPLACE INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![key, forked_json],
            )?;
            base_ctx.user_id = user_id.to_string();
            return Ok(base_ctx);
        }

        Ok(base_ctx)
    }

    pub fn save_context(&self, ctx: &Context) -> anyhow::Result<()> {
        let conn = self.conn();
        let data = serde_json::to_string(ctx)?;
        if !ctx.session_id.is_empty() && ctx.session_id != "default" {
            let key = format!("{}:::{}", ctx.user_id, ctx.session_id);
            conn.execute(
                "INSERT OR REPLACE INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![key, data],
            )?;
            // Ensure base row exists with the current session_id so resolve_user_key works
            let base_json = match conn.query_row(
                "SELECT data FROM contexts WHERE user_id = ?1",
                rusqlite::params![ctx.user_id],
                |row| row.get::<_, String>(0),
            ) {
                Ok(base_data) => {
                    let mut j = serde_json::from_str::<serde_json::Value>(&base_data)?;
                    normalize_legacy_keys(&mut j);
                    j["session_id"] = serde_json::json!(ctx.session_id);
                    serde_json::to_string(&j)?
                }
                Err(_) => {
                    // No base row yet – create one by cloning the current context
                    // (turn, settings, etc. become the canonical base for future forks)
                    serde_json::to_string(ctx)?
                }
            };
            conn.execute(
                "INSERT OR REPLACE INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![ctx.user_id, base_json],
            )?;
        } else {
            conn.execute(
                "INSERT OR REPLACE INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![ctx.user_id, data],
            )?;
        }
        // Publish while the DB lock still orders saves. Only the public web-
        // speech flag is sent, never credentials or other context contents.
        crate::dashboard::stream::chat_tts_settings(&ctx.user_id, ctx.settings.web_chat_tts);
        Ok(())
    }

    /// Atomic commit after an asynchronous routing decision. Never changes the
    /// active-session pointer or overwrites a concurrent user edit.
    pub fn compare_and_save_context(&self, expected: &Context, next: &Context) -> anyhow::Result<bool> {
        anyhow::ensure!(expected.user_id==next.user_id && expected.session_id==next.session_id,"Routing must preserve user/session identity");
        let mut conn=self.conn();let tx=conn.transaction()?;
        let raw:String=tx.query_row("SELECT data FROM contexts WHERE user_id=?1",[&expected.user_id],|r|r.get(0))?;
        let base=decode_context(&raw)?;
        if base.session_id!=expected.session_id { return Ok(false); }
        let key=Self::context_key(&expected.user_id,&expected.session_id);
        let raw:String=tx.query_row("SELECT data FROM contexts WHERE user_id=?1",[&key],|r|r.get(0))?;
        let current=decode_context(&raw)?;
        if serde_json::to_value(current)?!=serde_json::to_value(expected)? {return Ok(false);}
        tx.execute("UPDATE contexts SET data=?2,updated_at=datetime('now') WHERE user_id=?1",rusqlite::params![key,serde_json::to_string(next)?])?;
        tx.commit()?;Ok(true)
    }

    pub fn merge_context(
        &self,
        user_id: &str,
        updates: serde_json::Value,
    ) -> anyhow::Result<Context> {
        self.merge_context_with_actor(user_id, updates, false)
    }

    /// Model/tool-originated updates cannot grant user-only skill activation.
    pub fn merge_context_from_agent(&self, user_id: &str, updates: serde_json::Value) -> anyhow::Result<Context> {
        self.merge_context_with_actor(user_id, updates, true)
    }

    fn merge_context_with_actor(&self, user_id: &str, updates: serde_json::Value, from_agent: bool) -> anyhow::Result<Context> {
        anyhow::ensure!(updates.is_object(), "Context updates must be an object");
        let mut ctx = self.load_context(user_id)?;
        let previous_template = ctx.settings.system_template.clone();
        let previous_skill = ctx.settings.active_skill.clone();
        let previous_show_thinking = ctx.settings.show_thinking;
        let previous_tag_prefix = ctx.settings.tag_prefix.clone();
        let previous_download = ctx.settings.download;
        let previous_feedback_enabled = ctx.settings.feedback_enabled;
        let previous_decision_profile = ctx.settings.decision_profile.clone();
        let previous_workflow = crate::gateway::prompt::workflow_name(&ctx).to_string();
        let mut data: serde_json::Value = serde_json::to_value(&ctx)?;
        let mut updates = updates;
        normalize_legacy_keys(&mut updates);
        let obj = data.as_object_mut().unwrap();
        for (key, value) in updates.as_object().unwrap() {
            if key.contains('.') {
                set_nested_value(obj, key, value.clone());
            } else {
                merge_value(obj.entry(key.clone()).or_insert(serde_json::Value::Null), value);
            }
        }

        // Both historical state locations are accepted; keep the runtime's
        // canonical top-level state and the settings mirror in sync.
        let state_update = updates.get("active_state")
            .or_else(|| updates.get("settings.active_state"))
            .or_else(|| updates.pointer("/settings/active_state"));
        if let Some(active) = state_update {
            data["active_state"] = active.clone();
            if data["settings"].is_object() { data["settings"]["active_state"] = active.clone(); }
        }
        ctx = serde_json::from_value(data)?;
        anyhow::ensure!(!from_agent || ctx.settings.decision_profile==previous_decision_profile,
            "Decision profile selection is user/workflow-author policy, not model writable");
        if from_agent && previous_decision_profile.as_deref().is_some_and(|p|p!="off") {
            anyhow::ensure!(state_update.is_none() && crate::gateway::prompt::workflow_name(&ctx)==previous_workflow,
                "State/workflow selection is managed by the Decision router; continue the task without overriding it");
        }
        if ctx.settings.decision_profile!=previous_decision_profile {
            if let Some(name)=ctx.settings.decision_profile.as_deref().filter(|p|*p!="off") {
                crate::gateway::decision_profiles::load(std::path::Path::new("decisions"),name)?;
            }
        }
        if from_agent && state_update.is_some() {
            if let Some(target) = ctx.active_state.as_deref() {
                let workflow = crate::sm::load_file(crate::gateway::prompt::workflow_name(&ctx))
                    .map_err(|e| anyhow::anyhow!("Cannot select workflow state: {e}"))?;
                anyhow::ensure!(target != "_default" && workflow.states.contains_key(target),
                    "Undefined workflow state: {target}; current state unchanged");
            }
        }
        anyhow::ensure!(!from_agent || ctx.settings.download == previous_download,
            "Attachment download permission is user-only");
        anyhow::ensure!(!from_agent || ctx.settings.feedback_enabled == previous_feedback_enabled,
            "External feedback permission is user-only");
        anyhow::ensure!((0..=1000).contains(&ctx.settings.feedback_max_per_5min), "feedback_max_per_5min must be in 0..=1000");
        anyhow::ensure!((1..=86400).contains(&ctx.settings.feedback_window_secs), "feedback_window_secs must be in 1..=86400");
        crate::tags::validate_tag_prefix(&ctx.settings.tag_prefix)?;
        anyhow::ensure!(!from_agent || ctx.settings.tag_prefix == previous_tag_prefix,
            "Tag prefix is user/template-author configuration, not model writable");
        anyhow::ensure!(!from_agent || ctx.settings.show_thinking == previous_show_thinking,
            "Thinking visibility is user-only");
        anyhow::ensure!(ctx.user_id == user_id, "Context user_id cannot be changed");
        if ctx.settings.system_template != previous_template {
            if let Some(name) = &ctx.settings.system_template {
                crate::gateway::templates::resolve_template(std::path::Path::new("templates"), name)?;
            }
        }
        if ctx.settings.active_skill != previous_skill {
            anyhow::ensure!(!from_agent, "Persistent skill selection is user-only; ask the user to select it. Use use_skill for permitted task-local loading");
            if let Some(name) = &ctx.settings.active_skill {
                crate::skills::lookup_skill(self, std::path::Path::new("skills"), name)?;
            }
        }
        self.save_context(&ctx)?;
        Ok(ctx)
    }

    pub fn delete_context(&self, user_id: &str) -> anyhow::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM tool_outputs WHERE owner_id=?1", [user_id])?;
        // '%' and '_' in user IDs are literal, never SQL wildcard selectors.
        let prefix = format!("{user_id}:::");
        for table in [
            "messages",
            "memory_profile_selections",
            "memory_profiles",
            "memory",
            "contexts",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1 OR substr(user_id, 1, length(?2)) = ?2"),
                rusqlite::params![user_id, prefix],
            )?;
        }
        tx.commit()?;
        crate::dashboard::stream::chat_tts_settings(user_id, false);
        Ok(())
    }

    pub fn increment_turn(&self, ctx: &mut Context) {
        ctx.turn += 1;
    }

    /// Fork a context: create a new context under `new_user_id` by cloning
    /// the context at `parent_user_id`. Settings, custom_data, sm_data, and
    /// other state are copied. The new context starts with `turn = 0` and
    /// no associated messages. Returns the newly-created Context.
    pub fn fork_context(
        &self,
        parent_user_id: &str,
        new_user_id: &str,
        username: Option<&str>,
    ) -> anyhow::Result<Context> {
        let parent = self.load_context(parent_user_id)?;
        let mut forked = parent.clone();
        forked.user_id = new_user_id.to_string();
        forked.session_id = String::new();
        forked.turn = 0;
        forked.settings.llm_turn = 0;
        forked.settings.done = false;
        if let Some(name) = username {
            if !name.is_empty() {
                forked.username = Some(name.to_string());
            }
        } else if forked.username.is_none() {
            forked.username = parent.username.clone();
        }
        self.save_context(&forked)?;
        Ok(forked)
    }

    // ── Session Management ──────────────────────────────────────────────────

    fn context_key(user_id: &str, session_id: &str) -> String {
        if session_id.is_empty() || session_id == "default" {
            user_id.to_string()
        } else {
            format!("{}:::{}", user_id, session_id)
        }
    }

    /// Read the explicitly requested row, not the base user's active session.
    /// Missing sessions fork the base row without activating or changing it.
    pub fn load_session_context(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> anyhow::Result<Context> {
        use rusqlite::OptionalExtension;

        let key = Self::context_key(user_id, session_id);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let stored: Option<String> = tx.query_row(
            "SELECT data FROM contexts WHERE user_id = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        ).optional()?;
        let missing = stored.is_none();
        let source = match stored {
            Some(data) => Some(data),
            None if key != user_id => tx.query_row(
                "SELECT data FROM contexts WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get::<_, String>(0),
            ).optional()?,
            None => None,
        };
        let mut ctx = match source {
            Some(data) => decode_context(&data)?,
            None => Context::default(),
        };
        ctx.user_id = user_id.to_string();
        ctx.session_id = session_id.to_string();
        if missing {
            tx.execute(
                "INSERT INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![key, serde_json::to_string(&ctx)?],
            )?;
        }
        tx.commit()?;
        Ok(ctx)
    }

    /// Save only this explicit session. Activation remains save_context's job;
    /// never feed an already-composed key into its session resolver.
    pub fn save_session_context(&self, ctx: &Context) -> anyhow::Result<()> {
        let key = Self::context_key(&ctx.user_id, &ctx.session_id);
        let conn = self.conn();
        conn.execute(
            "INSERT INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            rusqlite::params![key, serde_json::to_string(ctx)?],
        )?;
        Ok(())
    }

    pub fn create_session(
        &self,
        user_id: &str,
        session_id: &str,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO user_sessions (user_id, session_id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, session_id) DO UPDATE SET updated_at = datetime('now')",
            rusqlite::params![user_id, session_id, name],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, user_id: &str, session_id: &str) -> anyhow::Result<()> {
        let conn = self.conn();
        super::tool_outputs::clear_session(&conn, user_id, session_id)?;
        conn.execute(
            "DELETE FROM memory_profile_selections WHERE user_id=?1 AND session_id=?2",
            rusqlite::params![
                user_id,
                if session_id == "default" {
                    ""
                } else {
                    session_id
                }
            ],
        )?;
        conn.execute(
            "DELETE FROM user_sessions WHERE user_id = ?1 AND session_id = ?2",
            rusqlite::params![user_id, session_id],
        )?;
        let key = Self::context_key(user_id, session_id);
        let _ = conn.execute(
            "DELETE FROM messages WHERE user_id = ?1",
            rusqlite::params![key],
        );
        let _ = conn.execute(
            "DELETE FROM contexts WHERE user_id = ?1",
            rusqlite::params![key],
        );
        Ok(())
    }

    pub fn list_sessions(&self, user_id: &str) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT session_id, name FROM user_sessions WHERE user_id = ?1 ORDER BY updated_at DESC"
        )?;
        let rows = stmt
            .query_map(rusqlite::params![user_id], |row| {
                let sid: String = row.get(0)?;
                let name: Option<String> = row.get(1)?;
                Ok((sid.clone(), name.unwrap_or(sid)))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rename_session(
        &self,
        user_id: &str,
        session_id: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE user_sessions SET name = ?1, updated_at = datetime('now') WHERE user_id = ?2 AND session_id = ?3",
            rusqlite::params![name, user_id, session_id],
        )?;
        Ok(())
    }

    pub fn clear_session_messages(&self, user_id: &str, session_id: &str) -> anyhow::Result<()> {
        let key = Self::context_key(user_id, session_id);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        super::tool_outputs::clear_session(&tx, user_id, session_id)?;
        tx.execute("DELETE FROM messages WHERE user_id = ?1", rusqlite::params![key])?;
        tx.commit()?;
        Ok(())
    }
}

fn decode_context(data: &str) -> anyhow::Result<Context> {
    let mut value = serde_json::from_str(data)?;
    normalize_legacy_keys(&mut value);
    Ok(serde_json::from_value(value)?)
}

/// Canonicalize legacy workflow names and dotted data paths on input.
pub fn canonical_context_key(key: &str) -> std::borrow::Cow<'_, str> {
    match key {
        "cl_file" => "sm_file".into(),
        "settings.cl_file" => "settings.sm_file".into(),
        "cl_data" => "sm_data".into(),
        _ => match key.strip_prefix("cl_data.") {
            Some(rest) => format!("sm_data.{rest}").into(),
            None => key.into(),
        },
    }
}

pub fn normalize_legacy_keys(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        // Preserve legacy-only entries if both namespaces occur; explicit
        // canonical values (including null) win conflicts deterministically.
        let canonical_data = obj.get("sm_data").cloned();
        if let Some(mut legacy) = obj.remove("cl_data") {
            if let Some(canonical) = obj.remove("sm_data") { merge_value(&mut legacy, &canonical); }
            obj.insert("sm_data".into(), legacy);
        }
        let legacy_paths: Vec<_> = obj.keys().filter(|key| key.starts_with("cl_data.")).cloned().collect();
        for old in legacy_paths {
            let new = canonical_context_key(&old).into_owned();
            if let Some(legacy) = obj.remove(&old) {
                let mut existing = canonical_data.as_ref();
                let mut conflict = false;
                for part in old[8..].split('.') {
                    if existing.is_some_and(|v| !v.is_object()) { conflict = true; break; }
                    existing = existing.and_then(|data| data.get(part));
                }
                if !conflict && existing.is_none() { obj.entry(new).or_insert(legacy); }
            }
        }
        let nested_canonical = obj.get("settings").and_then(|v| v.as_object()).is_some_and(|s| s.contains_key("sm_file"));
        for (old, new) in [("cl_file", "sm_file"), ("settings.cl_file", "settings.sm_file")] {
            if let Some(legacy) = obj.remove(old) {
                if old != "settings.cl_file" || !nested_canonical {
                    obj.entry(new.to_string()).or_insert(legacy);
                }
            }
        }
        if let Some(settings) = obj.get_mut("settings").and_then(|v| v.as_object_mut()) {
            if let Some(legacy) = settings.remove("cl_file") {
                settings.entry("sm_file".to_string()).or_insert(legacy);
            }
        }
    }
}

fn merge_value(target: &mut serde_json::Value, update: &serde_json::Value) {
    if let (Some(target), Some(update)) = (target.as_object_mut(), update.as_object()) {
        for (key, value) in update {
            merge_value(target.entry(key.clone()).or_insert(serde_json::Value::Null), value);
        }
    } else {
        *target = update.clone();
    }
}

/// Set a nested value in a JSON object using dot-notation (e.g., "custom_data.device")
/// Creates intermediate objects as needed.
fn set_nested_value(obj: &mut serde_json::Map<String, serde_json::Value>, path: &str, value: serde_json::Value) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let mut current = obj;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part: set the value
            current.insert(part.to_string(), value.clone());
        } else {
            // Intermediate part: navigate or create object
            if !current.contains_key(*part) || !current[*part].is_object() {
                current.insert(part.to_string(), serde_json::json!({}));
            }
            current = current.get_mut(*part).unwrap().as_object_mut().unwrap();
        }
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use tempfile::TempDir;

    fn stored_session_row(db: &Database, key: &str) -> String {
        db.conn().query_row(
            "SELECT data FROM contexts WHERE user_id = ?1",
            [key],
            |row| row.get(0),
        ).unwrap()
    }

    #[test]
    fn backend_explicit_session_roundtrip_has_one_scoped_key() {
        let (db, _dir) = test_db();
        let base = Context {
            user_id: "alice".into(),
            username: Some("Alice".into()),
            custom_data: serde_json::json!({"origin":"base"}),
            ..Default::default()
        };
        db.save_context(&base).unwrap();
        let mut other = base.clone();
        other.session_id = "t".into();
        other.custom_data = serde_json::json!({"origin":"other session"});
        db.save_context(&other).unwrap(); // t is active, but helpers request s.
        db.save_context(&Context { user_id: "bob".into(), ..Default::default() }).unwrap();
        let base_before = stored_session_row(&db, "alice");
        let other_before = stored_session_row(&db, "alice:::t");
        let bob_before = stored_session_row(&db, "bob");

        let mut session = base.clone();
        session.session_id = "s".into();
        session.turn = 7;
        session.custom_data = serde_json::json!({"origin":"session s", "revision":1});
        db.save_session_context(&session).unwrap();
        let mut loaded = db.load_session_context("alice", "s").unwrap();
        assert_eq!(serde_json::to_value(&loaded).unwrap(), serde_json::to_value(&session).unwrap());
        loaded.custom_data["revision"] = serde_json::json!(2);
        db.save_session_context(&loaded).unwrap();
        assert_eq!(db.load_session_context("alice", "s").unwrap().custom_data["revision"], 2);

        let keys: Vec<String> = {
            let conn = db.conn();
            let mut stmt = conn.prepare("SELECT user_id FROM contexts ORDER BY user_id").unwrap();
            let keys = stmt.query_map([], |row| row.get(0)).unwrap()
                .collect::<Result<Vec<_>, _>>().unwrap();
            keys
        };
        assert_eq!(keys, vec!["alice", "alice:::s", "alice:::t", "bob"]);
        let stored: Context = decode_context(&stored_session_row(&db, "alice:::s")).unwrap();
        assert_eq!(stored.user_id, "alice");
        assert_eq!(stored.session_id, "s");
        assert_eq!(stored_session_row(&db, "alice"), base_before);
        assert_eq!(stored_session_row(&db, "alice:::t"), other_before);
        assert_eq!(stored_session_row(&db, "bob"), bob_before);
        assert_eq!(db.load_context("alice").unwrap().custom_data["origin"], "other session");
        for default in ["", "default"] {
            let loaded = db.load_session_context("alice", default).unwrap();
            assert_eq!(loaded.custom_data["origin"], "base");
            assert_eq!(loaded.session_id, default);
        }
        assert_eq!(stored_session_row(&db, "alice"), base_before);
    }

    #[test]
    fn backend_missing_session_forks_base_not_active_session() {
        let (db, _dir) = test_db();
        let mut base = Context {
            user_id: "alice".into(),
            username: Some("Alice".into()),
            custom_data: serde_json::json!({"origin":"base", "keep":true}),
            sm_data: serde_json::json!({"project":"base project"}),
            ..Default::default()
        };
        base.settings.model = Some("base-model".into());
        db.save_context(&base).unwrap();
        let mut active = base.clone();
        active.session_id = "t".into();
        active.custom_data = serde_json::json!({"origin":"active session"});
        active.settings.model = Some("active-model".into());
        db.save_context(&active).unwrap();
        let base_before = stored_session_row(&db, "alice");
        let active_before = stored_session_row(&db, "alice:::t");

        let fork = db.load_session_context("alice", "s").unwrap();
        let mut expected = decode_context(&base_before).unwrap();
        expected.user_id = "alice".into();
        expected.session_id = "s".into();
        assert_eq!(serde_json::to_value(&fork).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_eq!(fork.settings.model.as_deref(), Some("base-model"));
        assert_eq!(stored_session_row(&db, "alice:::s"), serde_json::to_string(&fork).unwrap());
        assert_eq!(serde_json::to_value(db.load_session_context("alice", "s").unwrap()).unwrap(), serde_json::to_value(&fork).unwrap());
        assert_eq!(stored_session_row(&db, "alice"), base_before);
        assert_eq!(stored_session_row(&db, "alice:::t"), active_before);
        let count: i64 = db.conn().query_row(
            "SELECT count(*) FROM contexts WHERE user_id IN ('alice:::s', 'alice:::s:::s')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        let fresh = db.load_session_context("new-user", "s").unwrap();
        assert_eq!(fresh.user_id, "new-user");
        assert_eq!(fresh.session_id, "s");
        assert_eq!(fresh.mode, Context::default().mode);
        assert_eq!(stored_session_row(&db, "new-user:::s"), serde_json::to_string(&fresh).unwrap());
    }

    #[test]
    fn backend_delete_context_treats_user_wildcards_literally() {
        let (db, _dir) = test_db();
        for uid in ["a_%", "a_%:::work", "alice:::work", "a_X:::work"] {
            db.add_memory(uid, "keep scoped", Some("fact")).unwrap();
        }
        db.delete_context("a_%").unwrap();
        assert!(db.get_memories("a_%", 10).unwrap().is_empty());
        assert!(db.get_memories("a_%:::work", 10).unwrap().is_empty());
        assert_eq!(db.get_memories("alice:::work", 10).unwrap().len(), 1);
        assert_eq!(db.get_memories("a_X:::work", 10).unwrap().len(), 1);
    }

    #[test]
    fn backend_sm_aliases_emit_only_canonical_schema() {
        let ctx: Context = serde_json::from_value(serde_json::json!({
            "user_id": "alice", "cl_file": "old", "settings": {"cl_file": "legacy"}, "cl_data": {"keep": true}
        })).unwrap();
        assert_eq!(ctx.sm_file.as_deref(), Some("old"));
        assert_eq!(ctx.settings.sm_file.as_deref(), Some("legacy"));
        let json = serde_json::to_value(ctx).unwrap();
        assert!(json.get("cl_file").is_none());
        assert!(json["settings"].get("cl_file").is_none());
        assert_eq!(json["sm_data"]["keep"], true);
        assert!(json.get("cl_data").is_none());
    }

    #[test]
    fn backend_legacy_context_updates_merge_without_resetting_settings() {
        let (db, _dir) = test_db();
        db.merge_context("alice", serde_json::json!({"settings.max_llm_turns": 17, "sm_file": "first"})).unwrap();
        let ctx = db.merge_context("alice", serde_json::json!({"cl_file": "next", "settings": {"cl_file": "legacy"}})).unwrap();
        assert_eq!(ctx.sm_file.as_deref(), Some("next"));
        assert_eq!(ctx.settings.sm_file.as_deref(), Some("legacy"));
        assert_eq!(ctx.settings.max_llm_turns, Some(17));
        let ctx = db.merge_context("alice", serde_json::json!({"settings.cl_file": "old", "settings.sm_file": "new"})).unwrap();
        assert_eq!(ctx.settings.sm_file.as_deref(), Some("new"));
        let ctx = db.merge_context("alice", serde_json::json!({"settings.active_state": "next"})).unwrap();
        assert_eq!(ctx.active_state.as_deref(), Some("next"));
        assert_eq!(ctx.settings.active_state, ctx.active_state);
        assert!(db.merge_context("alice", serde_json::json!({"user_id": "bob"})).is_err());
    }

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_load_default_context() {
        let (db, _dir) = test_db();
        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.user_id, "user1");
        assert_eq!(ctx.mode, "agent");
        assert_eq!(ctx.turn, 0);
    }

    #[test]
    fn test_save_and_load_context() {
        let (db, _dir) = test_db();
        let ctx = Context {
            user_id: "user1".to_string(),
            turn: 5,
            mode: "coding".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();
        let loaded = db.load_context("user1").unwrap();
        assert_eq!(loaded.turn, 5);
        assert_eq!(loaded.mode, "coding");
    }

    #[test]
    fn test_merge_context() {
        let (db, _dir) = test_db();
        let ctx = Context {
            user_id: "user1".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();

        let updates = serde_json::json!({"mode": "coding", "turn": 3});
        let merged = db.merge_context("user1", updates).unwrap();
        assert_eq!(merged.mode, "coding");
        assert_eq!(merged.turn, 3);
    }

    #[test]
    fn test_merge_context_dot_notation() {
        let (db, _dir) = test_db();
        let ctx = Context {
            user_id: "user1".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();

        // Set custom_data.device via dot-notation
        let updates = serde_json::json!({"custom_data.device": "main"});
        let merged = db.merge_context("user1", updates).unwrap();
        assert_eq!(merged.custom_data["device"], "main");

        // Set another key without overwriting the first
        let updates = serde_json::json!({"custom_data.style": "analytical"});
        let merged = db.merge_context("user1", updates).unwrap();
        assert_eq!(merged.custom_data["device"], "main");
        assert_eq!(merged.custom_data["style"], "analytical");
    }

    #[test]
    fn test_merge_context_nested_dot_notation() {
        let (db, _dir) = test_db();
        let ctx = Context {
            user_id: "user1".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();

        // Set deeply nested value
        let updates = serde_json::json!({"custom_data.app.settings.theme": "dark"});
        let merged = db.merge_context("user1", updates).unwrap();
        assert_eq!(merged.custom_data["app"]["settings"]["theme"], "dark");

        // Set sibling key without overwriting
        let updates = serde_json::json!({"custom_data.app.settings.lang": "en"});
        let merged = db.merge_context("user1", updates).unwrap();
        assert_eq!(merged.custom_data["app"]["settings"]["theme"], "dark");
        assert_eq!(merged.custom_data["app"]["settings"]["lang"], "en");
    }

    #[test]
    fn test_increment_turn() {
        let (db, _dir) = test_db();
        let mut ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.turn, 0);
        db.increment_turn(&mut ctx);
        assert_eq!(ctx.turn, 1);
    }
}
