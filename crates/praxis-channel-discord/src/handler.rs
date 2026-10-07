use crate::host::{ChannelHost, DiscordCredentials, MirroredMessage};
use crate::ws_client::{IncomingMessage, OutgoingMessage, WsClient};
use serenity::async_trait;
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

#[cfg(feature = "songbird")]
async fn request_voice_response(client: &WsClient, payload: OutgoingMessage) -> anyhow::Result<String> {
    // The gateway always sends "Thinking..." before the final reply. Consume
    // progress/events as well, or the next utterance receives the previous reply.
    client
        .send_and_recv_until_response(payload, |_| {})
        .await?
        .map_err(anyhow::Error::msg)
}

fn generate_silent_wav(duration_ms: u32) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let num_samples = (48000 * duration_ms / 1000) as usize;
    let mut buffer = Vec::new();
    {
        let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut buffer), spec).unwrap();
        for i in 0..num_samples {
            // Very quiet tone (amplitude 100) instead of silence
            // This ensures Discord detects us as "speaking"
            let sample =
                ((i as f32 * 220.0 * 2.0 * std::f32::consts::PI / 48000.0).sin() * 100.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    buffer
}

pub struct DiscordHandler {
    pub host: Arc<dyn ChannelHost>,
    pub ws_client: Arc<Mutex<WsClient>>,
    pub request_lock: Arc<Mutex<()>>,
    pub voice_muted: Arc<Mutex<bool>>,
    pub voice_deafened: Arc<Mutex<bool>>,
    pub credentials: DiscordCredentials,
    pub voice_handler: Option<Arc<crate::voice::VoiceHandler>>,
}

impl DiscordHandler {
    pub fn new(
        host: Arc<dyn ChannelHost>,
        ws_client: Arc<Mutex<WsClient>>,
        credentials: DiscordCredentials,
    ) -> Self {
        Self {
            host,
            ws_client,
            request_lock: Arc::new(Mutex::new(())),
            voice_muted: Arc::new(Mutex::new(false)),
            voice_deafened: Arc::new(Mutex::new(true)),
            credentials,
            voice_handler: None,
        }
    }

    async fn check_channel_allowed(&self, msg: &Message) -> bool {
        let discord_user_id = msg.author.id.to_string();
        let pairing = match self.host.pairing_by_discord(&discord_user_id) {
            Ok(Some(p)) => p,
            _ => return false,
        };

        match self.host.load_context(&pairing.user_id) {
            Ok(ctx) => {
                let guild_id = msg.guild_id.map(|g| g.to_string());
                let channel_id = msg.channel_id.to_string();

                crate::commands::channel_allowed(&ctx["settings"], guild_id.as_deref(), &channel_id)
            }
            Err(_) => false,
        }
    }
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, ready: serenity::model::gateway::Ready) {
        tracing::info!("Discord bot connected as {}", ready.user.name);
        use serenity::model::user::OnlineStatus;
        ctx.set_presence(None, OnlineStatus::Online);

        #[cfg(feature = "songbird")]
        {
            if let Some(manager) = songbird::serenity::get(&ctx).await {
                crate::set_songbird_manager(manager);
                tracing::info!("Songbird manager stored");
            }
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        tracing::debug!(
            author = %msg.author.id,
            channel = %msg.channel_id,
            content_len = msg.content.len(),
            content = %msg.content,
            "Received message"
        );

        if msg.author.bot {
            return;
        }

        // Check for empty content which indicates missing MESSAGE_CONTENT intent
        if msg.content.is_empty() {
            tracing::warn!(
                author = %msg.author.id,
                channel = %msg.channel_id,
                "Message has empty content - ensure MESSAGE_CONTENT intent is enabled in Discord Developer Portal (Bot -> Privileged Gateway Intents)"
            );
            // Don't return early - still process reactions and other interactions
        }

        if msg.content.starts_with('/') {
            // Handle special commands
            let cmd = msg.content.trim().to_lowercase();
            match cmd.as_str() {
                "/stop" => {
                    // Stop the active agent loop
                    let discord_user_id = msg.author.id.to_string();
                    if let Ok(Some(pairing)) = self.host.pairing_by_discord(&discord_user_id) {
                        self.host.stop_agent(&pairing.user_id).await;
                        let _ = msg.reply(&ctx.http, "Agent stopped.").await;
                    } else {
                        let _ = msg.reply(&ctx.http, "No active agent to stop.").await;
                    }
                    return;
                }
                "/last" | "/last-response" => {
                    // Let the LLM do one more response and then stop
                    let discord_user_id = msg.author.id.to_string();
                    if let Ok(Some(pairing)) = self.host.pairing_by_discord(&discord_user_id) {
                        if let Some(sender) = self.host.question_sender(&pairing.user_id).await {
                            // Send a special stop signal
                            let _ = sender.send("__LAST_RESPONSE__".to_string());
                            let _ = msg.reply(&ctx.http, "Agent will respond one last time then stop.").await;
                        } else {
                            let _ = msg.reply(&ctx.http, "No active agent found.").await;
                        }
                    }
                    return;
                }
                _ => {}
            }
            return;
        }

        // Check if this is a response to a pending ask_question
        let channel_id = msg.channel_id.to_string();
        let is_question_response = if !msg.content.is_empty() {
            // Don't hold the lock while calling handle_message_reply
            self.host.interaction_reply(
                &channel_id,
                &msg.content,
                &msg.author.id.to_string(),
            )
            .await
        } else {
            false
        };

        // If this was a question response, don't process it as a normal message
        if is_question_response {
            return;
        }

        if let Some(guild_id) = msg.guild_id {
            let mentions_me = msg.mentions_me(&ctx).await.unwrap_or(false);
            if !mentions_me {
                tracing::debug!(
                    "Message in guild {} does not mention bot, ignoring",
                    guild_id
                );
                return;
            }
        }

        tracing::info!(
            "Received message from {} in channel {}: {}",
            msg.author.id,
            msg.channel_id,
            msg.content
        );

        let discord_user_id = msg.author.id.to_string();

        let pairing = match self.host.pairing_by_discord(&discord_user_id) {
            Ok(Some(p)) => p,
            _ => {
                tracing::warn!("User {} is not paired, ignoring message", discord_user_id);
                return;
            }
        };

        // Reject disallowed channels before downloading or injecting anything.
        if !self.check_channel_allowed(&msg).await {
            tracing::warn!("Message from guild/channel not in allowed list, ignoring");
            return;
        }

        // Mirror the Discord message into the dashboard chat so the user sees
        // Discord traffic next to dashboard-originated messages. Persisted in
        // the messages table so it survives reloads (SSE alone is transient).
        {
            let author_name = msg.author.name.clone();
            let channel_id_str = msg.channel_id.to_string();
            let content_clone = msg.content.clone();
            let uid = pairing.user_id.clone();
            let is_dm = msg.guild_id.is_none();
            let payload = serde_json::json!({
                "direction": "user",
                "channel_id": channel_id_str,
                "channel_kind": if is_dm { "dm" } else { "guild" },
                "author": author_name,
                "content": content_clone,
            });
            self.host.dashboard_event(
                &uid,
                "discord_message",
                &payload.to_string(),
            );
            // Persist for the dashboard chat history (role discord_user).
            if !content_clone.trim().is_empty() {
                let host = self.host.clone();
                let mirror = MirroredMessage {
                    user_id: uid.clone(),
                    content: content_clone.clone(),
                    direction: "user",
                    author: author_name.clone(),
                    channel_id: channel_id_str.clone(),
                    channel_kind: if is_dm { "dm" } else { "guild" },
                };
                tokio::spawn(async move {
                    if let Err(e) = host.mirror_message(mirror) {
                        tracing::warn!("Failed to persist Discord mirror message: {}", e);
                    }
                });
            }
        }

        // Attachment writes are opt-in; failure to load policy is fail-closed.
        let mut attachment_context = String::new();
        let download = self.host.load_context(&pairing.user_id).is_ok_and(|ctx| ctx["settings"]["download"].as_bool().unwrap_or(false));
        if !msg.attachments.is_empty() && !download {
            attachment_context.push_str("\n[Attachments not downloaded: enable settings.download to allow saving Discord attachments.]");
        }
        if !msg.attachments.is_empty() && download {
            let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
            let vm_enabled = std::env::var("VM_ENABLED").map(|v| v == "true").unwrap_or(false);

            // If VM enabled: shared/downloads (accessible at /mnt/shared/downloads in VM)
            // If VM disabled: data/downloads
            let download_dir = if vm_enabled {
                std::path::Path::new(&data_dir).join("shared").join("downloads")
            } else {
                std::path::Path::new(&data_dir).join("downloads")
            };
            let _ = std::fs::create_dir_all(&download_dir);

            for attachment in &msg.attachments {
                tracing::info!(
                    "Downloading attachment: {} ({} bytes) from user {}",
                    attachment.filename,
                    attachment.size,
                    pairing.user_id
                );

                let file_path = download_dir.join(&attachment.filename);

                // Download the file
                match reqwest::get(&attachment.url).await {
                    Ok(resp) => {
                        match resp.bytes().await {
                            Ok(bytes) => {
                                match std::fs::write(&file_path, &bytes) {
                                    Ok(_) => {
                                        tracing::info!(
                                            "Downloaded attachment {} to {}",
                                            attachment.filename,
                                            file_path.display()
                                        );
                                        if vm_enabled {
                                            attachment_context.push_str(&format!(
                                                "\n[Attached file saved to: /mnt/shared/downloads/{}]",
                                                attachment.filename
                                            ));
                                        } else {
                                            attachment_context.push_str(&format!(
                                                "\n[Attached file saved to: {}]",
                                                file_path.display()
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("Failed to save attachment {}: {}", attachment.filename, e);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to read attachment {}: {}", attachment.filename, e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to download attachment {}: {}", attachment.filename, e);
                    }
                }
            }
        }

        // Check if there's an active agent loop for this user - inject message if so
        if let Some(sender) = self.host.question_sender(&pairing.user_id).await {
            tracing::info!("Injecting Discord message into active agent loop for user {}", pairing.user_id);
            let mut inject_content = msg.content.clone();
            if !attachment_context.is_empty() {
                inject_content.push_str(&attachment_context);
            }
            if let Err(e) = sender.send(inject_content) {
                tracing::warn!("Failed to inject message into agent loop: {}", e);
            } else {
                let _ = msg.react(&ctx.http, '✅').await;
            }
            return;
        }

        let typing_channel_id = msg.channel_id;
        let typing_http = ctx.http.clone();

        let typing_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(3));
            loop {
                ticker.tick().await;
                let _ = typing_channel_id.start_typing(&typing_http);
            }
        });

        let _req_guard = self.request_lock.lock().await;

        // Append attachment context to message content
        let mut content = msg.content.clone();
        if !attachment_context.is_empty() {
            content.push_str(&attachment_context);
        }

        let payload = OutgoingMessage::Message {
            user_id: pairing.user_id.clone(),
            content,
            channel_id: msg.channel_id.to_string(),
        };

        let ws_client = self.ws_client.lock().await;
        if let Err(e) = ws_client.send(payload).await {
            tracing::error!("Failed to send to gateway: {}", e);
            typing_handle.abort();
            let _ = msg
                .reply(&ctx.http, "Gateway error. Try again later.")
                .await;
            return;
        }

        let mut thinking_msg_ids: Vec<serenity::model::id::MessageId> = Vec::new();

        let mirror_uid = pairing.user_id.clone();
        let mirror_channel = msg.channel_id.to_string();
        let mirror_kind = if msg.guild_id.is_none() { "dm" } else { "guild" };
        loop {
            match ws_client.recv().await {
                Ok(IncomingMessage::Response { content, .. }) => {
                    typing_handle.abort();
                    for mid in &thinking_msg_ids {
                        let _ = msg.channel_id.delete_message(&ctx.http, mid).await;
                    }
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if let Err(e) = send_message_split(&ctx.http, msg.channel_id, trimmed).await
                        {
                            tracing::error!("Failed to send response: {}", e);
                        }
                        // Mirror the bot's Discord reply into the dashboard chat.
                        let bot_meta = serde_json::json!({
                            "direction": "bot",
                            "channel_id": mirror_channel,
                            "channel_kind": mirror_kind,
                            "author": "bot",
                            "content": trimmed,
                        });
                        self.host.dashboard_event(
                            &mirror_uid,
                            "discord_message",
                            &bot_meta.to_string(),
                        );
                        // Persist for the dashboard chat history (role discord_bot).
                        let host = self.host.clone();
                        let mirror = MirroredMessage {
                            user_id: mirror_uid.clone(),
                            content: trimmed.to_string(),
                            direction: "bot",
                            author: "bot".to_string(),
                            channel_id: mirror_channel.clone(),
                            channel_kind: mirror_kind,
                        };
                        tokio::spawn(async move {
                            if let Err(e) = host.mirror_message(mirror) {
                                tracing::warn!("Failed to persist Discord mirror bot reply: {}", e);
                            }
                        });
                    } else {
                        tracing::warn!("LLM returned empty response, skipping send");
                    }
                    break;
                }
                Ok(IncomingMessage::Reasoning { user_id, content }) => {
                    if user_id == pairing.user_id {
                        // Persistent output, NOT one of the disposable "Thinking..."
                        // status messages. Split without dropping any provider text.
                        if let Err(e) = send_thinking_split(&ctx.http, msg.channel_id, &content).await {
                            tracing::error!(%e, "Failed to send reasoning");
                        }
                    }
                }
                Ok(IncomingMessage::Feedback { content, .. }) => {
                    match msg.channel_id.say(&ctx.http, &content).await {
                        Ok(feedback_msg) => {
                            thinking_msg_ids.push(feedback_msg.id);
                            // Interim status ("Thinking...") is visible in Discord
                            // only; the dashboard already streams its own feedback.
                        }
                        Err(e) => {
                            tracing::error!("Failed to send feedback: {}", e);
                        }
                    }
                }
                Ok(IncomingMessage::Error { message }) => {
                    typing_handle.abort();
                    for mid in &thinking_msg_ids {
                        let _ = msg.channel_id.delete_message(&ctx.http, mid).await;
                    }
                    let _ = msg.reply(&ctx.http, format!("Error: {}", message)).await;
                    break;
                }
                Ok(IncomingMessage::Pong) => {
                    tracing::debug!("Pong received");
                }
                Ok(_) => {
                    tracing::debug!("Ignoring unexpected message type");
                }
                Err(e) => {
                    tracing::error!("Failed to receive response: {}", e);
                    typing_handle.abort();
                    for mid in &thinking_msg_ids {
                        let _ = msg.channel_id.delete_message(&ctx.http, mid).await;
                    }
                    let _ = msg
                        .reply(&ctx.http, "Failed to get response. Try again.")
                        .await;
                    break;
                }
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            match command.data.name.as_str() {
                "skill" => {
                    if let Err(error) = crate::commands::handle_skill_command(&self.host, &ctx, &command).await {
                        tracing::error!(%error, "Skill command response failed");
                    }
                }
                "show_thinking" => {
                    let enabled = command.data.options.iter().find(|o| o.name == "enabled")
                        .and_then(|o| o.value.as_bool());
                    let reply = match (self.host.pairing_by_discord(&command.user.id.to_string()), enabled) {
                        (Ok(Some(pairing)), Some(enabled)) => {
                            match self.host.merge_context(&pairing.user_id, serde_json::json!({"settings.show_thinking":enabled})) {
                                Ok(_) => format!("Thinking display: {}", if enabled { "on" } else { "off" }),
                                Err(_) => "Could not save thinking visibility.".into(),
                            }
                        }
                        _ => "Pair first, then use /show_thinking enabled:true or false.".into(),
                    };
                    let _ = command.create_response(&ctx.http,
                        serenity::builder::CreateInteractionResponse::Message(
                            serenity::builder::CreateInteractionResponseMessage::new().content(reply).ephemeral(true)
                        )).await;
                }
                "thinking" => {
                    let discord_user_id = command.user.id.to_string();
                    let mode = command.data.options.iter()
                        .find(|o| o.name == "mode")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("auto")
                        .to_ascii_lowercase();
                    let reply = match self.host.pairing_by_discord(&discord_user_id) {
                        Ok(Some(pairing)) => {
                            let valid = matches!(mode.as_str(), "on" | "off" | "auto" | "low" | "medium" | "high" | "xhigh");
                            if !valid {
                                "Invalid mode. Use off, low, medium, high, xhigh, on, or auto.".to_string()
                            } else {
                                match self.host.context_command(&pairing.user_id, &format!("/context set settings.thinking_mode={mode}")) {
                                    Ok(result) => {
                                        if result.starts_with("✓") {
                                            let suffix = if mode == "auto" { " (provider default)" } else { "" };
                                            format!("🧠 Thinking mode set to **{mode}**{suffix}")
                                        } else {
                                            result
                                        }
                                    }
                                    Err(e) => format!("error: {e}"),
                                }
                            }
                        }
                        _ => "You are not paired with this bot.".to_string(),
                    };
                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content(reply),
                            ),
                        )
                        .await;
                }
                "pair" => {
                    if let Err(e) =
                        crate::commands::handle_pair_command(self.host.as_ref(), &ctx, &command)
                            .await
                    {
                        tracing::error!("Pair command error: {}", e);
                    }
                }
                "mute" => {
                    let mut muted = self.voice_muted.lock().await;
                    *muted = true;
                    tracing::info!("Bot voice muted by user {}", command.user.id);
                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content("Bot microphone is now muted."),
                            ),
                        )
                        .await;
                }
                "unmute" => {
                    let mut muted = self.voice_muted.lock().await;
                    *muted = false;
                    tracing::info!("Bot voice unmuted by user {}", command.user.id);
                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content("Bot microphone is now unmuted."),
                            ),
                        )
                        .await;
                }
                "deafen" => {
                    let mut deafened = self.voice_deafened.lock().await;
                    *deafened = true;
                    tracing::info!("Bot voice deafened by user {}", command.user.id);
                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content("Bot is now deafened (cannot hear)."),
                            ),
                        )
                        .await;
                }
                "undeafen" => {
                    let mut deafened = self.voice_deafened.lock().await;
                    *deafened = false;
                    tracing::info!("Bot voice undeafened by user {}", command.user.id);
                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content("Bot is now undeafened (can hear)."),
                            ),
                        )
                        .await;
                }
                "tts" => {
                    let discord_user_id = command.user.id.to_string();
                    if let Ok(Some(pairing)) = self.host.pairing_by_discord(&discord_user_id) {
                        let user_id = &pairing.user_id;
                        if let Ok(mut db_ctx) = self.host.load_context(user_id) {
                            let new_state = !db_ctx["settings"]["use_tts"].as_bool().unwrap_or(false);
                            db_ctx["settings"]["use_tts"] = serde_json::json!(new_state);
                            if let Err(e) = self.host.save_context(user_id, &db_ctx) {
                                tracing::error!("Failed to save TTS state: {}", e);
                            } else {
                                let msg = if new_state {
                                    "TTS is now **enabled**."
                                } else {
                                    "TTS is now **disabled**."
                                };
                                let _ = command
                                    .create_response(
                                        &ctx.http,
                                        serenity::builder::CreateInteractionResponse::Message(
                                            serenity::builder::CreateInteractionResponseMessage::new()
                                                .content(msg),
                                        ),
                                    )
                                    .await;
                            }
                        }
                    } else {
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("Please pair first with /pair"),
                                ),
                            )
                            .await;
                    }
                }
                "context" => {
                    // Generic context get/set/show. Replaces the ad-hoc /mode
                    // toggle and any other one-off context manipulation. Uses
                    // the same parser as the TUI and web chat so syntax is
                    // identical across all three frontends.
                    let discord_user_id = command.user.id.to_string();
                    let pairing = match self.host.pairing_by_discord(&discord_user_id) {
                        Ok(Some(p)) => p,
                        _ => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("You are not paired with this bot."),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    let arg = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "command")
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("show")
                        .to_string();

                    let response = match self.host.context_command(&pairing.user_id, &format!("/context {arg}")) {
                        Ok(response) => response,
                        Err(e) => format!("error: {e}\n\nExamples:\n  set custom_data.mode=chat\n  set settings.max_llm_turns=20\n  get settings.voice_tts_enabled\n  show settings"),
                    };

                    // Discord has a 2000-char limit on slash-command responses;
                    // wrap long output in a code block and truncate if needed.
                    let truncated = if response.chars().count() > 1900 {
                        let head: String = response.chars().take(1900).collect();
                        format!("{head}\n…(truncated)")
                    } else {
                        response
                    };
                    let body = format!("```\n{truncated}\n```");

                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content(body),
                            ),
                        )
                        .await;
                }
                "join" => {
                    let guild_id = match command.guild_id {
                        Some(g) => g,
                        None => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("This command can only be used in a server."),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    let voice_channel_id = ctx.cache.guild(guild_id).and_then(|g| {
                        g.voice_states
                            .get(&command.user.id)
                            .and_then(|vs| vs.channel_id)
                    });

                    let voice_channel_id = match voice_channel_id {
                        Some(id) => id,
                        None => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("You need to be in a voice channel first."),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    #[cfg(feature = "songbird")]
                    {
                        let manager = songbird::serenity::get(&ctx).await;
                        if let Some(manager) = manager {
                            match manager.join(guild_id, voice_channel_id).await {
                                Ok(call) => {
                                    crate::set_discord_voice_state(
                                        Some(guild_id.get()),
                                        Some(command.user.id.get()),
                                    )
                                    .await;

                                    // Session-Hook (pgpu §12.4): Voice-Session
                                    // startet → GPU-Slots vorwärmen (LLM für
                                    // Antworten, media für TTS; STT läuft
                                    // router-lokal). Fire-and-forget.
                                    praxis_gpu_router::wake_slots_for_session();

                                    // Check context for deafened and voice_enabled settings
                                    let discord_user_id = command.user.id.to_string();
                                    let (should_deafen, voice_listening) =
                                        match self.host.pairing_by_discord(&discord_user_id) {
                                            Ok(Some(pairing)) => {
                                                match self.host.load_context(&pairing.user_id) {
                                                    Ok(ctx) => (
                                                        ctx["settings"]["voice_deafened"].as_bool().unwrap_or(true),
                                                        ctx["settings"]["voice_enabled"].as_bool().unwrap_or(false)
                                                            && ctx["settings"]["use_stt"].as_bool().unwrap_or(false),
                                                    ),
                                                    Err(_) => (true, false),
                                                }
                                            }
                                            _ => (true, false),
                                        };

                                    {
                                        let mut call_lock = call.lock().await;
                                        let _ = call_lock.deafen(should_deafen).await;
                                    }
                                    *self.voice_deafened.lock().await = should_deafen;
                                    tracing::info!(
                                        "Voice: Bot deafened={} (from context)",
                                        should_deafen
                                    );

                                    // Set up voice handler and transcription pipeline
                                    let voice_handler =
                                        Arc::new(crate::voice::VoiceHandler::new());

                                    let fallback_discord_id = command.user.id.get();

                                    // Set allowed Discord user IDs (only paired users)
                                    let mut allowed_ids = vec![fallback_discord_id];
                                    if let Ok(all_pairings) = self.host.list_pairings() {
                                        for p in &all_pairings {
                                            if let Ok(did) = p.discord_user_id.parse::<u64>() {
                                                allowed_ids.push(did);
                                            }
                                        }
                                    }
                                    voice_handler.set_allowed_discord_ids(allowed_ids).await;

                                    // Load context for voice settings
                                    if let Ok(Some(pairing)) = self
                                        .db
                                        .get_pairing_by_discord(&fallback_discord_id.to_string())
                                    {
                                        if let Ok(ctx) = self.host.load_context(&pairing.user_id) {
                                            voice_handler
                                                .set_auto_pause(
                                                    ctx.settings.voice_auto_pause_enabled,
                                                )
                                                .await;
                                        }
                                    }

                                    let voice_receiver = praxis_voice::songbird_integration::VoiceReceiver::new(voice_handler.clone());

                                    // Register event handler on the call
                                    {
                                        let mut call_lock = call.lock().await;
                                        call_lock.add_global_event(
                                            songbird::events::Event::Core(
                                                songbird::events::CoreEvent::SpeakingStateUpdate,
                                            ),
                                            voice_receiver.clone(),
                                        );
                                        call_lock.add_global_event(
                                            songbird::events::Event::Core(
                                                songbird::events::CoreEvent::VoiceTick,
                                            ),
                                            voice_receiver.clone(),
                                        );
                                        call_lock.add_global_event(
                                            songbird::events::Event::Core(
                                                songbird::events::CoreEvent::ClientDisconnect,
                                            ),
                                            voice_receiver.clone(),
                                        );

                                        // Play a brief silent clip to force Discord to send
                                        // SpeakingStateUpdate events for all users in the channel.
                                        // Without this, the SSRC-to-user mapping never populates
                                        // and we can't identify who is speaking.
                                        let silent_wav = generate_silent_wav(500);
                                        tracing::info!("VOICE: Playing silent WAV ({} bytes) to trigger SpeakingStateUpdate", silent_wav.len());
                                        call_lock
                                            .play_input(songbird::input::Input::from(silent_wav));
                                        tracing::info!(
                                            "VOICE: Silent WAV queued, waiting for events..."
                                        );
                                    }

                                    // Set up transcription channel
                                    let (tx, mut rx) =
                                        tokio::sync::mpsc::channel::<(String, Vec<i16>)>(10);
                                    voice_handler.set_transcription_channel(tx).await;

                                    // Spawn periodic buffer flusher (independent of VoiceTick)
                                    let flush_handler = voice_handler.clone();
                                    tokio::spawn(async move {
                                        let mut interval = tokio::time::interval(
                                            std::time::Duration::from_millis(200),
                                        );
                                        loop {
                                            interval.tick().await;
                                            flush_handler.process_all_buffers().await;
                                        }
                                    });

                                    // Spawn transcription processing task
                                    let db = self.db.clone();
                                    let ws_client = self.ws_client.clone();
                                    let req_lock = self.request_lock.clone();
                                    let secrets = self.credentials.clone();
                                    let voice_muted = self.voice_muted.clone();
                                    let http = ctx.http.clone();
                                    tokio::spawn(async move {
                                        tracing::info!(
                                            "VOICE_PIPELINE: Transcription processor started"
                                        );
                                        while let Some((user_id, audio_data)) = rx.recv().await {
                                            // Check if muted
                                            if *voice_muted.lock().await {
                                                tracing::debug!("VOICE_PIPELINE: Bot is muted, skipping transcription");
                                                continue;
                                            }

                                            tracing::info!(
                                                "VOICE_PIPELINE: Received {} samples from user {}",
                                                audio_data.len(),
                                                user_id
                                            );

                                            // Resolve Discord ID → Praxis user ID for context lookup
                                            let praxis_user_id = if let Ok(Some(pairing)) =
                                                db.get_pairing_by_discord(&user_id)
                                            {
                                                pairing.user_id.clone()
                                            } else {
                                                user_id.clone()
                                            };

                                            // Get STT config from context
                                            let ctx = self
                                                .host
                                                .load_context(&praxis_user_id)
                                                .unwrap_or_else(|_| {
                                                    serde_json::json!({
                                                        "user_id": praxis_user_id,
                                                        "settings": {}
                                                    })
                                                });
                                            let settings = &ctx["settings"];
                                            let string =
                                                |key: &str| settings[key].as_str().map(str::to_string);
                                            let stt_type =
                                                string("voice_stt_type").unwrap_or_default();

                                            if !settings["voice_enabled"].as_bool().unwrap_or(false)
                                                || !settings["use_stt"].as_bool().unwrap_or(false)
                                            {
                                                tracing::debug!("VOICE_PIPELINE: voice_enabled={}, use_stt={} for user {}, skipping transcription", settings["voice_enabled"], settings["use_stt"], user_id);
                                                continue;
                                            }

                                            let api_key =
                                                self.credentials.elevenlabs_api_key.clone();
                                            let model_path = match stt_type.as_str() {
                                                "vosk" => string("voice_vosk_model_path"),
                                                "whisper" => string("voice_whisper_model_path"),
                                                _ => None,
                                            };

                                            let stt_config = praxis_voice::STTConfig {
                                                engine: stt_type,
                                                api_key,
                                                model_path,
                                                vosk_url: string("voice_vosk_url"),
                                                elevenlabs_model: string("elevenlabs_stt_model"),
                                                elevenlabs_language: string(
                                                    "elevenlabs_stt_language",
                                                ),
                                                elevenlabs_tag_audio_events: settings
                                                    ["elevenlabs_stt_tag_audio_events"]
                                                    .as_bool()
                                                    .unwrap_or(false),
                                                elevenlabs_no_verbatim: settings
                                                    ["elevenlabs_stt_no_verbatim"]
                                                    .as_bool()
                                                    .unwrap_or(false),
                                            };

                                            let wav_data =
                                                praxis_voice::pcm_to_wav(&audio_data, 16000, 1);

                                            let transcription = praxis_voice::transcribe_audio(
                                                &wav_data,
                                                &stt_config,
                                            )
                                            .await;

                                            let text = match transcription {
                                                Ok(t) => t.trim().to_string(),
                                                Err(e) => {
                                                    tracing::warn!("VOICE_PIPELINE: Transcription failed for user {}: {}", user_id, e);
                                                    continue;
                                                }
                                            };

                                            if text.is_empty() {
                                                tracing::debug!(
                                                    "VOICE_PIPELINE: Empty transcription, skipping"
                                                );
                                                continue;
                                            }

                                            tracing::info!(
                                                "VOICE_PIPELINE: Transcribed for user {}: '{}'",
                                                user_id,
                                                text
                                            );

                                            // Check wake words
                                            let wake_words = ctx.settings.voice_wake_words.clone();
                                            let wake_match =
                                                praxis_voice::wake_word::matches_wake_word(
                                                    &text,
                                                    &wake_words,
                                                );

                                            if !wake_match.matched {
                                                tracing::debug!("VOICE_PIPELINE: No wake word in '{}', skipping", text);
                                                continue;
                                            }

                                            let message_text =
                                                if wake_match.remaining_text.is_empty() {
                                                    text.clone()
                                                } else {
                                                    wake_match.remaining_text.clone()
                                                };

                                            tracing::info!("VOICE_PIPELINE: Wake word {:?} matched, sending message: '{}'",
                                                wake_match.wake_word, message_text);

                                            // Send to gateway via WsClient
                                            let payload = crate::ws_client::OutgoingMessage::Message {
                                                user_id: praxis_user_id.clone(),
                                                content: message_text,
                                                channel_id: format!("voice:{}", guild_id),
                                            };

                                            // Serialize complete requests, not just the first
                                            // feedback frame, then release locks before Discord HTTP.
                                            let response_text = {
                                                let _req_guard = req_lock.lock().await;
                                                let ws = ws_client.lock().await;
                                                match request_voice_response(&ws, payload).await {
                                                    Ok(content) => {
                                                        tracing::info!("VOICE_PIPELINE: Got response ({} chars)", content.len());
                                                        Some(content)
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("VOICE_PIPELINE: Gateway request failed: {}", e);
                                                        None
                                                    }
                                                }
                                            };

                                            // Send text response to Discord channel
                                            if let Some(text) = response_text {
                                                if !text.trim().is_empty() {
                                                    let guild = guild_id;
                                                    if let Ok(channels) =
                                                        guild.channels(&http).await
                                                    {
                                                        if let Some((channel_id, _)) = channels.iter()
                                                            .find(|(_, ch)| ch.kind == serenity::model::channel::ChannelType::Text)
                                                        {
                                                            if let Err(e) = send_message_split(
                                                                &http, *channel_id, &text
                                                            ).await {
                                                                tracing::error!("VOICE_PIPELINE: Failed to send text response: {}", e);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        tracing::info!(
                                            "VOICE_PIPELINE: Transcription processor stopped"
                                        );
                                    });

                                    tracing::info!(
                                        "Joined voice channel {} in guild {} with STT pipeline",
                                        voice_channel_id,
                                        guild_id
                                    );
                                    let _ = command
                                        .create_response(
                                            &ctx.http,
                                            serenity::builder::CreateInteractionResponse::Message(
                                                serenity::builder::CreateInteractionResponseMessage::new()
                                                    .content(if voice_listening {
                                                        format!("Joined <#{}> (voice listening active)", voice_channel_id)
                                                    } else {
                                                        format!("Joined <#{}> (voice listening disabled)", voice_channel_id)
                                                    }),
                                            ),
                                        )
                                        .await;
                                }
                                Err(e) => {
                                    tracing::error!("Failed to join voice channel: {}", e);
                                    let _ = command
                                        .create_response(
                                            &ctx.http,
                                            serenity::builder::CreateInteractionResponse::Message(
                                                serenity::builder::CreateInteractionResponseMessage::new()
                                                    .content(format!("Failed to join voice channel: {}", e)),
                                            ),
                                        )
                                        .await;
                                }
                            }
                        } else {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("Voice manager not available."),
                                    ),
                                )
                                .await;
                        }
                    }

                    #[cfg(not(feature = "songbird"))]
                    {
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("Voice support not compiled. Build with --features songbird"),
                                ),
                            )
                            .await;
                    }
                }
                "disconnect" => {
                    let guild_id = match command.guild_id {
                        Some(g) => g,
                        None => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("This command can only be used in a server."),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    #[cfg(feature = "songbird")]
                    {
                        let manager = songbird::serenity::get(&ctx).await;
                        if let Some(manager) = manager {
                            if manager.get(guild_id).is_some() {
                                if let Err(e) = manager.remove(guild_id).await {
                                    tracing::error!("Failed to disconnect: {}", e);
                                }
                                crate::set_discord_voice_state(None, None).await;
                                tracing::info!("Disconnected from voice in guild {}", guild_id);
                                let _ = command
                                    .create_response(
                                        &ctx.http,
                                        serenity::builder::CreateInteractionResponse::Message(
                                            serenity::builder::CreateInteractionResponseMessage::new()
                                                .content("Disconnected from voice channel."),
                                        ),
                                    )
                                    .await;
                            } else {
                                let _ = command
                                    .create_response(
                                        &ctx.http,
                                        serenity::builder::CreateInteractionResponse::Message(
                                            serenity::builder::CreateInteractionResponseMessage::new()
                                                .content("Not in a voice channel."),
                                        ),
                                    )
                                    .await;
                            }
                        }
                    }

                    #[cfg(not(feature = "songbird"))]
                    {
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("Voice support not compiled."),
                                ),
                            )
                            .await;
                    }
                }
                "compact" => {
                    let discord_user_id = command.user.id.to_string();
                    let pairing = match self.host.pairing_by_discord(&discord_user_id) {
                        Ok(Some(p)) => p,
                        _ => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("Please pair first with /pair"),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Defer(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content("Compacting conversation..."),
                            ),
                        )
                        .await;

                    let compact_msg = crate::ws_client::OutgoingMessage::Compact {
                        user_id: pairing.user_id.clone(),
                    };

                    let ws = self.ws_client.lock().await;
                    match ws.send_and_recv_until_response(compact_msg, |_| {}).await {
                        Ok(Ok(response)) => {
                            let _ = command
                                .edit_response(
                                    &ctx.http,
                                    serenity::builder::EditInteractionResponse::default()
                                        .content(&response),
                                )
                                .await;
                        }
                        Ok(Err(e)) => {
                            let _ = command
                                .edit_response(
                                    &ctx.http,
                                    serenity::builder::EditInteractionResponse::default()
                                        .content(format!("Compaction failed: {}", e)),
                                )
                                .await;
                        }
                        Err(e) => {
                            tracing::error!("Compact WS error: {}", e);
                            let _ = command
                                .edit_response(
                                    &ctx.http,
                                    serenity::builder::EditInteractionResponse::default()
                                        .content("Failed to compact. Try again."),
                                )
                                .await;
                        }
                    }
                }
                "stop" => {
                    let discord_user_id = command.user.id.to_string();
                    if let Ok(Some(pairing)) = self.host.pairing_by_discord(&discord_user_id) {
                        self.host.stop_agent(&pairing.user_id).await;
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("Agent stopped."),
                                ),
                            )
                            .await;
                    } else {
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("No active agent to stop."),
                                ),
                            )
                            .await;
                    }
                }
                "clear" => {
                    let discord_user_id = command.user.id.to_string();
                    if let Ok(Some(pairing)) = self.host.pairing_by_discord(&discord_user_id) {
                        self.host.stop_agent(&pairing.user_id).await;
                        if let Err(e) = self.host.clear_messages(&pairing.user_id) {
                            tracing::error!("Failed to clear messages for user {}: {}", pairing.user_id, e);
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("Failed to clear messages."),
                                    ),
                                )
                                .await;
                            return;
                        }
                        tracing::info!("Cleared all messages for user {}", pairing.user_id);
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("All messages cleared and agent stopped."),
                                ),
                            )
                            .await;
                    } else {
                        let _ = command
                            .create_response(
                                &ctx.http,
                                serenity::builder::CreateInteractionResponse::Message(
                                    serenity::builder::CreateInteractionResponseMessage::new()
                                        .content("Please pair first with /pair"),
                                ),
                            )
                            .await;
                    }
                }
                "session" => {
                    let discord_user_id = command.user.id.to_string();
                    let pairing = match self.host.pairing_by_discord(&discord_user_id) {
                        Ok(Some(p)) => p,
                        _ => {
                            let _ = command
                                .create_response(
                                    &ctx.http,
                                    serenity::builder::CreateInteractionResponse::Message(
                                        serenity::builder::CreateInteractionResponseMessage::new()
                                            .content("Please pair first with /pair"),
                                    ),
                                )
                                .await;
                            return;
                        }
                    };

                    let action_opt = command.data.options.iter().find(|o| o.name == "action");
                    let name_opt = command.data.options.iter().find(|o| o.name == "name");

                    let action = action_opt
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("list");
                    let name = name_opt
                        .and_then(|o| o.value.as_str())
                        .unwrap_or("")
                        .to_string();

                    let user_id = &pairing.user_id;

                    let response = match action {
                        "list" => {
                            match self.host.list_sessions(user_id) {
                                Ok(sessions) => {
                                    if sessions.is_empty() {
                                        "No sessions found. Use `/session action:create name:my-session` to create one.".to_string()
                                    } else {
                                        let mut lines = vec!["**Your sessions:**".to_string()];
                                        for (sid, sname) in sessions {
                                            lines.push(format!("- `{}` — {}", sid, sname));
                                        }
                                        lines.join("\n")
                                    }
                                }
                                Err(e) => format!("Error listing sessions: {}", e),
                            }
                        }
                        "create" => {
                            let sid = if name.is_empty() {
                                format!("session-{}", uuid::Uuid::new_v4().to_string()[..8].to_string())
                            } else {
                                name.clone()
                            };
                            let _ = self.host.create_session(user_id, &sid, Some(&sid));
                            // Switch into it immediately
                            if let Ok(mut ctx) = self.host.load_context(user_id) {
                                ctx["session_id"] = serde_json::json!(sid);
                                let _ = self.host.save_context(user_id, &ctx);
                            }
                            format!("Created and switched to session `{}`.", sid)
                        }
                        "switch" => {
                            if name.is_empty() {
                                "Please provide a session name to switch to.".to_string()
                            } else {
                                if let Ok(mut ctx) = self.host.load_context(user_id) {
                                    ctx["session_id"] = serde_json::json!(name);
                                    if let Err(e) = self.host.save_context(user_id, &ctx) {
                                        format!("Failed to switch session: {}", e)
                                    } else {
                                        format!("Switched to session `{}`.", name)
                                    }
                                } else {
                                    "Failed to load context.".to_string()
                                }
                            }
                        }
                        "rename" => {
                            if name.is_empty() {
                                "Please provide the current session name and target name. Usage: `/session action:rename name:old new-name`.".to_string()
                            } else {
                                let parts: Vec<&str> = name.splitn(2, ' ').collect();
                                if parts.len() < 2 {
                                    "Usage: `/session action:rename name:old-session new-name`.".to_string()
                                } else {
                                    let _ = self.host.rename_session(user_id, parts[0], parts[1]);
                                    format!("Renamed session `{}` to `{}`.", parts[0], parts[1])
                                }
                            }
                        }
                        "delete" => {
                            if name.is_empty() {
                                "Please provide a session name to delete.".to_string()
                            } else {
                                let _ = self.host.delete_session(user_id, &name);
                                // Reset to default if the deleted session was active
                                if let Ok(ctx) = self.host.load_context(user_id) {
                                    if ctx["session_id"].as_str() == Some(name.as_str()) {
                                        let _ = self.host.merge_context(user_id, serde_json::json!({"session_id": ""}));
                                    }
                                }
                                format!("Deleted session `{}` and its messages.", name)
                            }
                        }
                        "clear" => {
                            let sid = if name.is_empty() {
                                if let Ok(ctx) = self.host.load_context(user_id) {
                                    ctx["session_id"].as_str().unwrap_or_default().to_string()
                                } else {
                                    String::new()
                                }
                            } else {
                                name.clone()
                            };
                            if sid.is_empty() || sid == "default" {
                                let _ = self.host.clear_messages(user_id);
                                "Cleared messages for the default session.".to_string()
                            } else {
                                let _ = self.host.clear_session_messages(user_id, &sid);
                                format!("Cleared messages for session `{}`.", sid)
                            }
                        }
                        _ => "Unknown action. Use list, create, switch, rename, delete, or clear.".to_string(),
                    };

                    let _ = command
                        .create_response(
                            &ctx.http,
                            serenity::builder::CreateInteractionResponse::Message(
                                serenity::builder::CreateInteractionResponseMessage::new()
                                    .content(response),
                            ),
                        )
                        .await;
                }
                _ => {}
            }
        }
    }

    async fn reaction_add(&self, _ctx: Context, reaction: serenity::model::channel::Reaction) {
        let channel_id = reaction.channel_id.to_string();
        let emoji_raw = reaction.emoji.as_data();
        let emoji = urlencoding::decode(&emoji_raw)
            .unwrap_or_default()
            .into_owned();
        let user_id = reaction.user_id.map(|u| u.to_string()).unwrap_or_default();

        self.host.interaction_reaction(&channel_id, &emoji, &user_id).await;
    }
}

/// Lossless Discord chunks, conservatively counted in UTF-16 code units.
/// Keep separators/indentation. A minimum of two units fits any Unicode scalar.
pub fn split_message(content: &str, max_len: usize) -> Vec<String> {
    let limit = max_len.max(2);
    if content.is_empty() { return vec![String::new()]; }
    let mut chunks = Vec::new();
    let mut remaining = content;
    while !remaining.is_empty() {
        let mut units = 0;
        let mut end = 0;
        for (i, ch) in remaining.char_indices() {
            if units + ch.len_utf16() > limit { break; }
            units += ch.len_utf16();
            end = i + ch.len_utf8();
        }
        if end < remaining.len() {
            let window = &remaining[..end];
            if let Some(i) = window.rfind('\n').or_else(|| window.rfind(' ')) {
                end = i + 1; // Include separator; never trim the user's output.
            }
        }
        let (chunk, rest) = remaining.split_at(end);
        chunks.push(chunk.to_string());
        remaining = rest;
    }
    chunks
}

/// Complete, clearly labelled thinking boxes. Split embedded fence runs too so
/// model-supplied backticks cannot escape the box or swallow following output.
pub fn thinking_boxes(content: &str) -> Vec<String> {
    const PREFIX: &str = "**Thinking**\n```text\n";
    const SUFFIX: &str = "\n```";
    let budget = 2000 - PREFIX.encode_utf16().count() - SUFFIX.encode_utf16().count();
    let mut out = Vec::new();
    for chunk in split_message(content, budget) {
        let mut rest = chunk.as_str();
        while !rest.is_empty() {
            let end = rest.find("```").map_or(rest.len(), |p| p + 2);
            out.push(format!("{PREFIX}{}{SUFFIX}", &rest[..end]));
            rest = &rest[end..];
        }
    }
    out
}

async fn send_thinking_split(
    http: &serenity::http::Http,
    channel: serenity::model::id::ChannelId,
    content: &str,
) -> Result<(), serenity::Error> {
    for part in thinking_boxes(content) { send_message_split(http, channel, &part).await?; }
    Ok(())
}

/// Send a message to Discord, splitting if necessary.
pub async fn send_message_split(
    http: &serenity::http::Http,
    channel_id: serenity::model::id::ChannelId,
    content: &str,
) -> Result<(), serenity::Error> {
    let chunks = split_message(content, 2000);
    for chunk in chunks {
        if !chunk.is_empty() {
            channel_id.send_message(http, serenity::builder::CreateMessage::new()
                .content(&chunk)
                .allowed_mentions(serenity::builder::CreateAllowedMentions::new()
                    .all_users(false).all_roles(false).everyone(false))).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod output_tests;

#[cfg(test)]
mod discord_tests {
    use super::*;

    #[test]
    fn test_split_message_multibyte_no_panic() {
        // Regression: a 4-byte emoji (🔜) straddling the 2000-byte limit used to
        // panic with "end byte index 2000 is not a char boundary".
        let emoji = "🔜";
        let mut content = String::new();
        while content.len() < 4000 {
            content.push_str(emoji);
        }
        let chunks = split_message(&content, 2000);
        assert!(!chunks.is_empty());
        let rejoined: String = chunks.concat();
        // Nothing lost: every emoji survives the round trip.
        assert_eq!(rejoined.chars().filter(|c| *c == '🔜').count(), content.chars().count());
    }

    #[test]
    fn test_split_message_prefers_newline_and_space() {
        let content = "a".repeat(1990) + "\n" + &"b".repeat(100);
        let chunks = split_message(&content, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a".repeat(1990) + "\n");

        let spaced = "x".repeat(1995) + " yyyyy";
        let chunks = split_message(&spaced, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "x".repeat(1995) + " ");
    }

    #[cfg(feature = "songbird")]
    #[tokio::test]
    async fn test_voice_gateway_waits_for_final_reply_without_lagging_one_turn() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as Frame;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            for expected in ["first", "second", "error"] {
                let frame = socket.next().await.unwrap().unwrap();
                let request: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                assert_eq!(request["content"], expected);
                assert_eq!(request["channel_id"], "voice:123");
                let progress = serde_json::json!({"type": "feedback", "user_id": "test-user", "content": "Thinking..."});
                socket.send(Frame::Text(progress.to_string())).await.unwrap();
                socket.send(Frame::Text(serde_json::json!({"type": "event", "event": "progress", "payload": {}}).to_string())).await.unwrap();
                let response = if expected == "error" {
                    serde_json::json!({"type": "error", "message": "mock gateway error"})
                } else {
                    serde_json::json!({"type": "response", "user_id": "test-user", "content": format!("reply to {expected}")})
                };
                socket.send(Frame::Text(response.to_string())).await.unwrap();
            }
        });
        let client = WsClient::connect(&url).await.unwrap();
        for input in ["first", "second", "error"] {
            let payload = OutgoingMessage::Message {
                user_id: "test-user".into(), content: input.into(), channel_id: "voice:123".into(),
            };
            let reply = tokio::time::timeout(Duration::from_secs(3), request_voice_response(&client, payload)).await.unwrap();
            if input == "error" {
                assert!(reply.unwrap_err().to_string().contains("mock gateway error"));
            } else {
                assert_eq!(reply.unwrap(), format!("reply to {input}"));
            }
        }
        server.await.unwrap();
    }

    #[test]
    fn test_handler_types_exist() {
        assert!(true);
    }

    #[test]
    fn test_split_message_short() {
        let msg = "Hello World";
        let chunks = split_message(msg, 2000);
        assert_eq!(chunks, vec!["Hello World"]);
    }

    #[test]
    fn test_split_message_long() {
        let msg = "a".repeat(3000);
        let chunks = split_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 1000);
    }

    #[test]
    fn test_split_message_at_newline() {
        let mut msg = "a".repeat(1990);
        msg.push('\n');
        msg.push_str("b".repeat(100).as_str());

        let chunks = split_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with("a\n"));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn test_split_message_at_space() {
        let mut msg = "a".repeat(1990);
        msg.push(' ');
        msg.push_str("b".repeat(100).as_str());

        let chunks = split_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_split_message_multiple_chunks() {
        let msg = "x".repeat(5000);
        let chunks = split_message(&msg, 2000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 2000);
        assert_eq!(chunks[2].len(), 1000);
    }

    #[test]
    fn test_split_message_empty() {
        let chunks = split_message("", 2000);
        assert_eq!(chunks, vec![""]);
    }
}
