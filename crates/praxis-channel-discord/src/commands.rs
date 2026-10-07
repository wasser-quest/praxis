use crate::host::{ChannelHost, ContextJson};
use std::sync::Arc;
use serenity::model::application::CommandInteraction;
use serenity::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::Mutex;

#[allow(dead_code)]
const DISCORD_BOT_PERMISSIONS: u64 = 1024 // VIEW_CHANNEL
    | 2048 // SEND_MESSAGES
    | 4096 // SEND_TTS_MESSAGES
    | 32768 // ATTACH_FILES
    | 65536 // READ_MESSAGE_HISTORY
    | 262144 // USE_EXTERNAL_EMOJIS
    | 64 // ADD_REACTIONS
    | 1048576 // CONNECT
    | 2097152 // SPEAK
    | 2147483648; // USE_APPLICATION_COMMANDS

struct InteractionTracker(Mutex<HashMap<u64, std::time::Instant>>);

impl InteractionTracker {
    fn new() -> Self {
        InteractionTracker(Mutex::new(HashMap::new()))
    }

    async fn is_duplicate(&self, id: u64) -> bool {
        let mut tracker = self.0.lock().await;
        let now = std::time::Instant::now();
        if let Some(last_seen) = tracker.get(&id) {
            if now.duration_since(*last_seen) < Duration::from_secs(5) {
                return true;
            }
        }
        tracker.retain(|_, v| now.duration_since(*v) < Duration::from_secs(10));
        tracker.insert(id, now);
        false
    }
}

lazy_static::lazy_static! {
    static ref INTERACTION_TRACKER: InteractionTracker = InteractionTracker::new();
}

pub async fn setup_commands(http: &serenity::http::Http) -> anyhow::Result<()> {
    tracing::info!("Setting up discord commands");

    let pair_cmd = serenity::builder::CreateCommand::new("pair")
        .description("Get a pairing code to link your Discord account")
        .dm_permission(true);

    match http.create_global_command(&pair_cmd).await {
        Ok(pair_created) => tracing::info!(
            "Registered pair command: {} (id: {})",
            pair_created.name,
            pair_created.id
        ),
        Err(e) => {
            tracing::warn!(
                "Failed to register pair command: {}. Bot may need re-invite with applications.commands scope.",
                e
            );
            return Ok(());
        }
    }

    let mute_cmd = serenity::builder::CreateCommand::new("mute")
        .description("Mute the bot's microphone in voice channels")
        .dm_permission(true);
    http.create_global_command(&mute_cmd).await?;

    let unmute_cmd = serenity::builder::CreateCommand::new("unmute")
        .description("Unmute the bot's microphone in voice channels")
        .dm_permission(true);
    http.create_global_command(&unmute_cmd).await?;

    let deafen_cmd = serenity::builder::CreateCommand::new("deafen")
        .description("Deafen the bot in voice channels (bot won't hear)")
        .dm_permission(true);
    http.create_global_command(&deafen_cmd).await?;

    let undeafen_cmd = serenity::builder::CreateCommand::new("undeafen")
        .description("Undeafen the bot in voice channels")
        .dm_permission(true);
    http.create_global_command(&undeafen_cmd).await?;

    let tts_cmd = serenity::builder::CreateCommand::new("tts")
        .description("Toggle text-to-speech on/off")
        .dm_permission(true);
    http.create_global_command(&tts_cmd).await?;

    let join_cmd = serenity::builder::CreateCommand::new("join")
        .description("Join your voice channel")
        .dm_permission(false);
    http.create_global_command(&join_cmd).await?;

    let disconnect_cmd = serenity::builder::CreateCommand::new("disconnect")
        .description("Disconnect the bot from the voice channel")
        .dm_permission(false);
    http.create_global_command(&disconnect_cmd).await?;

    // (deprecated) `/mode` toggled `custom_data.mode` between agent and chat.
    // Use `/context set custom_data.mode=chat` instead.

    let compact_cmd = serenity::builder::CreateCommand::new("compact")
        .description("Compact conversation history into a summary")
        .dm_permission(true);
    http.create_global_command(&compact_cmd).await?;

    let stop_cmd = serenity::builder::CreateCommand::new("stop")
        .description("Stop the active agent loop")
        .dm_permission(true);
    http.create_global_command(&stop_cmd).await?;

    let clear_cmd = serenity::builder::CreateCommand::new("clear")
        .description("Delete all messages from history and context")
        .dm_permission(true);
    http.create_global_command(&clear_cmd).await?;

    let session_cmd = serenity::builder::CreateCommand::new("session")
        .description("Manage sessions: list, create, switch, rename, delete")
        .add_option(
            serenity::builder::CreateCommandOption::new(
                serenity::model::application::CommandOptionType::String,
                "action",
                "Action to perform: list, create, switch, rename, delete, clear"
            )
            .required(true)
            .add_string_choice("list", "list")
            .add_string_choice("create", "create")
            .add_string_choice("switch", "switch")
            .add_string_choice("rename", "rename")
            .add_string_choice("delete", "delete")
            .add_string_choice("clear", "clear")
        )
        .add_option(
            serenity::builder::CreateCommandOption::new(
                serenity::model::application::CommandOptionType::String,
                "name",
                "Session name (for create/switch/rename/delete)"
            )
            .required(false)
        )
        .dm_permission(true);
    http.create_global_command(&session_cmd).await?;

    // /context — generic getter/setter for any context variable, including
    // dot-notation paths like `settings.voice_tts_enabled`. Replaces the
    // ad-hoc `/mode` toggle and miscellaneous ctx mutations that used to
    // live inline in the message handler.
    let context_cmd = serenity::builder::CreateCommand::new("context")
        .description("Get/set context variables (supports dot-notation, e.g. settings.max_llm_turns)")
        .add_option(
            serenity::builder::CreateCommandOption::new(
                serenity::model::application::CommandOptionType::String,
                "command",
                "Full command, e.g. `set settings.max_llm_turns=20 custom_data.device=main`"
            )
            .required(true)
        )
        .dm_permission(true);
    http.create_global_command(&context_cmd).await?;

    let skill_cmd = serenity::builder::CreateCommand::new("skill")
        .description("Activate a skill for your messages; use off to disable or list to browse")
        .add_option(serenity::builder::CreateCommandOption::new(
            serenity::model::application::CommandOptionType::String,
            "skillname", "Registered skill name, off, or list (default)"
        ).required(false))
        .dm_permission(true);
    http.create_global_command(&skill_cmd).await?;

    let thinking_cmd = serenity::builder::CreateCommand::new("thinking")
        .description("Set thinking level for your model (off/low/medium/high/xhigh)")
        .add_option(serenity::builder::CreateCommandOption::new(
            serenity::model::application::CommandOptionType::String,
            "mode", "on, off, or auto (provider default)"
        )
        .required(true)
        .add_string_choice("off — no thinking", "off")
        .add_string_choice("low", "low")
        .add_string_choice("medium", "medium")
        .add_string_choice("high", "high")
        .add_string_choice("xhigh — maximum reasoning", "xhigh")
        .add_string_choice("on — alias for high", "on")
        .add_string_choice("auto — provider default", "auto"))
        .dm_permission(true);
    http.create_global_command(&thinking_cmd).await?;
    let show = serenity::builder::CreateCommand::new("show_thinking")
        .description("Show the model's complete reasoning output (separate from its answer)")
        .add_option(serenity::builder::CreateCommandOption::new(
            serenity::model::application::CommandOptionType::Boolean,
            "enabled", "Show reasoning, including long output split into messages"
        ).required(true)).dm_permission(true);
    http.create_global_command(&show).await?;

    tracing::info!("All discord commands registered");
    Ok(())
}

/// Same allow-list semantics for messages and skill interactions; DMs have no
/// guild restriction but still obey the channel allow-list.
pub fn channel_allowed(settings: &serde_json::Value, guild: Option<&str>, channel: &str) -> bool {
    fn allowed(values: &serde_json::Value, id: &str) -> bool {
        values.as_array().is_some_and(|list| {
            list.iter()
                .any(|value| value.as_str().is_some_and(|value| value == "*" || value == id))
        })
    }
    guild.map_or(true, |guild| allowed(&settings["allowed_guilds"], guild))
        && allowed(&settings["allowed_channels"], channel)
}

fn skill_command_context(
    host: &dyn ChannelHost,
    discord_user_id: &str,
    guild: Option<&str>,
    channel: &str,
) -> anyhow::Result<ContextJson> {
    let pairing = host
        .pairing_by_discord(discord_user_id)?
        .ok_or_else(|| anyhow::anyhow!("Please pair first with /pair"))?;
    let context = host.load_context(&pairing.user_id)?;
    anyhow::ensure!(
        channel_allowed(&context["settings"], guild, channel),
        "This channel or guild is not allowed"
    );
    Ok(context)
}

/// Authorize before filesystem access, including list/off/unknown arguments.
/// A pending pairing code or an internal context ID is not a completed pairing.
fn apply_skill_command_from_dir(
    host: &dyn ChannelHost,
    discord_user_id: &str,
    guild: Option<&str>,
    channel: &str,
    argument: &str,
    directory: &std::path::Path,
) -> anyhow::Result<String> {
    skill_command_context(host, discord_user_id, guild, channel)?;
    let name = argument.trim();
    let off = name.eq_ignore_ascii_case("off");
    let list = name.is_empty() || name.eq_ignore_ascii_case("list") || name.starts_with("list ");
    if list || name.starts_with("search ") {
        let result = if let Some(query) = name.strip_prefix("search ") {
            host.skill_search(directory, query)?
        } else {
            host.skill_browse(directory, name.strip_prefix("list ").unwrap_or(""))?
        };
        // Recheck after index/filesystem access, before any metadata disclosure.
        let ctx = skill_command_context(host, discord_user_id, guild, channel)?;
        let lines = result
            .skills
            .iter()
            .map(|s| {
                format!(
                    "{}{}{}{} — {}",
                    s.name,
                    s.version.as_ref().map(|v| format!(" v{v}")).unwrap_or_default(),
                    if s.skill_hidden { " [hidden]" } else { "" },
                    if s.user_only { " [user-only]" } else { "" },
                    truncate_chars(&s.description, 90)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let more = result
            .next_after
            .as_deref()
            .map(|cursor| format!("\nNext page: /skill skillname:list {cursor}"))
            .unwrap_or_default();
        return Ok(format!(
            "Active skill: {}\nAvailable skills:\n{}{}\nUse /skill skillname:NAME, off, or search KEYWORDS.",
            ctx["settings"]["active_skill"].as_str().unwrap_or("off"),
            if lines.is_empty() { "(none)" } else { &lines },
            more
        ));
    }
    // off needs neither a readable skill directory nor an enabled loader.
    if !off {
        host.skill_exists(directory, name)?;
        anyhow::ensure!(host.tool_enabled("use_skill")?, "use_skill is disabled");
    }
    let mut ctx = skill_command_context(host, discord_user_id, guild, channel)?;
    let user = ctx["user_id"].as_str().unwrap_or_default().to_string();
    ctx["settings"]["active_skill"] =
        if off { serde_json::Value::Null } else { serde_json::json!(name) };
    host.save_context(&user, &ctx)?;
    Ok(if off {
        "Active skill disabled.".to_string()
    } else {
        format!("Skill '{name}' is active for your messages. This loads instructions; it does not execute actions or change tool permissions.")
    })
}

/// Bounded display text; the channel never forwards raw host output.
fn truncate_chars(value: &str, max: usize) -> String {
    let mut text: String = value.chars().take(max).collect();
    if value.chars().count() > max {
        text.push('…');
    }
    text
}

pub fn apply_skill_command(
    host: &dyn ChannelHost,
    discord_user_id: &str,
    guild: Option<&str>,
    channel: &str,
    argument: &str,
) -> anyhow::Result<String> {
    let mut ctx = skill_command_context(host, discord_user_id, guild, channel)?;
    let user = ctx["user_id"].as_str().unwrap_or_default().to_string();
    let name = argument.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("list") {
        let skills = host
            .registered_skills()
            .iter()
            .map(|s| format!("{} — {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(format!(
            "Active skill: {}\nAvailable skills:\n{}\nUse /skill skillname:NAME or /skill skillname:off.",
            ctx["settings"]["active_skill"].as_str().unwrap_or("off"),
            if skills.is_empty() { "(none)" } else { &skills }
        ));
    }
    if name.eq_ignore_ascii_case("off") {
        ctx["settings"]["active_skill"] = serde_json::Value::Null;
    } else {
        anyhow::ensure!(
            host.registered_skill(name),
            "Unknown skill '{name}'. Use /skill to list registered skills"
        );
        anyhow::ensure!(host.tool_enabled("use_skill")?, "use_skill is disabled");
        ctx["settings"]["active_skill"] = serde_json::json!(name);
    }
    host.save_context(&user, &ctx)?;
    Ok(match ctx["settings"]["active_skill"].as_str() {
        Some(name) => format!("Skill '{name}' is active for your messages. This loads instructions; it does not execute actions or change tool permissions."),
        None => "Active skill disabled.".to_string(),
    })
}

pub async fn handle_skill_command(host: &Arc<dyn ChannelHost>, ctx: &Context, command: &CommandInteraction) -> anyhow::Result<()> {
    // A first index build may take longer than Discord's interaction deadline.
    // Defer ephemerally without disclosing metadata, then authorize inside the worker.
    command.create_response(&ctx.http, serenity::builder::CreateInteractionResponse::Defer(
        serenity::builder::CreateInteractionResponseMessage::new().ephemeral(true)
    )).await?;
    let host = host.clone();
    let user = command.user.id.to_string();
    let guild = command.guild_id.map(|g| g.to_string());
    let channel = command.channel_id.to_string();
    let argument = command.data.options.iter().find(|o| o.name == "skillname").and_then(|o| o.value.as_str()).unwrap_or("list").to_string();
    let result = tokio::task::spawn_blocking(move || {
        apply_skill_command_from_dir(host.as_ref(), &user, guild.as_deref(), &channel, &argument, std::path::Path::new("skills"))
    }).await?;
    let response = match result { Ok(text) => text, Err(error) => format!("Error: {error}") };
    command.edit_response(&ctx.http, serenity::builder::EditInteractionResponse::new()
        .content(truncate_chars(&response, 1900))
    ).await?;
    Ok(())
}

pub async fn handle_pair_command(
    host: &dyn ChannelHost,
    ctx: &Context,
    command: &CommandInteraction,
) -> anyhow::Result<()> {
    let interaction_id = command.id.to_string().parse::<u64>().unwrap_or(0);
    if INTERACTION_TRACKER.is_duplicate(interaction_id).await {
        tracing::warn!("Duplicate pair command ignored: {}", interaction_id);
        return Ok(());
    }

    command
        .create_response(
            &ctx.http,
            serenity::builder::CreateInteractionResponse::Defer(
                serenity::builder::CreateInteractionResponseMessage::new()
                    .content("Generating pairing code..."),
            ),
        )
        .await?;

    let discord_user_id = command.user.id.to_string();

    let code = generate_pairing_code();
    let expires_at = (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
    host.create_pending_pairing(&code, &discord_user_id, &expires_at)?;

    let dm_channel = match command.user.create_dm_channel(&ctx.http).await {
        Ok(ch) => ch,
        Err(e) => {
            command
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::default()
                        .content("Could not send you a DM. Please enable DMs from server members and try again."),
                )
                .await
                .ok();
            anyhow::bail!(
                "Failed to create DM channel for user {}: {}",
                discord_user_id,
                e
            );
        }
    };

    match dm_channel
        .send_message(
            &ctx.http,
            serenity::builder::CreateMessage::default().content(format!(
                "Your pairing code is: **{}**\n\nTo complete pairing, run:\n```praxis pair {}```",
                code, code
            )),
        )
        .await
    {
        Ok(_) => {}
        Err(e) => {
            command
                .edit_response(
                    &ctx.http,
                    serenity::builder::EditInteractionResponse::default().content(
                        "Could not send DM. Please enable DMs from server members and try again.",
                    ),
                )
                .await
                .ok();
            anyhow::bail!("Failed to send DM to user {}: {}", discord_user_id, e);
        }
    }

    command
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::default()
                .content("Check your DMs for the pairing code!"),
        )
        .await?;

    tracing::info!(
        "Sent pairing DM to user {} with code {}",
        discord_user_id,
        code
    );

    Ok(())
}

fn generate_pairing_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let part1: u16 = rng.gen_range(1000..9999);
    let part2: u16 = rng.gen_range(1000..9999);
    format!("{}-{}", part1, part2)
}

#[cfg(test)]
mod discord_tests {
    use super::*;

    #[test]
    fn test_generate_pairing_code() {
        let code = generate_pairing_code();
        assert_eq!(code.len(), 9);
        assert!(code.contains('-'));
        let parts: Vec<&str> = code.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 4);
    }

    #[test]
    fn test_register_commands_list() {
        let cmds = crate::commands::register_commands();
        assert!(!cmds.is_empty());
    }
}

pub fn register_commands() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "pair",
            "description": "Get a pairing code to link your Discord account"
        }),
        serde_json::json!({
            "name": "mute",
            "description": "Mute the bot's microphone in voice channels"
        }),
        serde_json::json!({
            "name": "unmute",
            "description": "Unmute the bot's microphone in voice channels"
        }),
        serde_json::json!({
            "name": "deafen",
            "description": "Deafen the bot in voice channels"
        }),
        serde_json::json!({
            "name": "undeafen",
            "description": "Undeafen the bot in voice channels"
        }),
        serde_json::json!({
            "name": "tts",
            "description": "Toggle text-to-speech on/off"
        }),
        serde_json::json!({
            "name": "join",
            "description": "Join your voice channel"
        }),
        serde_json::json!({
            "name": "disconnect",
            "description": "Disconnect the bot from the voice channel"
        }),
        serde_json::json!({
            "name": "compact",
            "description": "Compact conversation history into a summary"
        }),
        serde_json::json!({
            "name": "stop",
            "description": "Stop the active agent loop"
        }),
        serde_json::json!({
            "name": "clear",
            "description": "Delete all messages from history and context"
        }),
        serde_json::json!({
            "name": "context",
            "description": "Get/set context variables (supports dot-notation)"
        }),
        serde_json::json!({
            "name": "skill",
            "description": "Activate a skill; use off to disable or list to browse",
            "options": [{"type": 3, "name": "skillname", "description": "Registered name, off, or list", "required": false}]
        }),
        serde_json::json!({
            "name": "thinking",
            "description": "Toggle extended reasoning (thinking) for your model",
            "options": [{"type": 3, "name": "mode", "description": "off, low, medium, high, xhigh, on (alias high), or auto", "required": true, "choices": [
                {"name": "off", "value": "off"},
                {"name": "low", "value": "low"},
                {"name": "medium", "value": "medium"},
                {"name": "high", "value": "high"},
                {"name": "xhigh", "value": "xhigh"},
                {"name": "on", "value": "on"},
                {"name": "auto", "value": "auto"}
            ]}]
        }),
    ]
}
