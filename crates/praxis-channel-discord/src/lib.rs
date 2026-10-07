pub mod commands;
pub mod host;
pub mod handler;
pub mod voice;
pub mod ws_client;

pub use ws_client::{IncomingMessage, OutgoingMessage, WsClient};

use crate::handler::DiscordHandler;
use anyhow::Context;
use once_cell::sync::OnceCell;
use serenity::model::gateway::GatewayIntents;
use serenity::Client;
use std::sync::Arc;
use tokio::sync::Mutex;

const DEFAULT_GATEWAY_URL: &str = "ws://localhost:3537/ws";

// ── TTS Playback Channel ─────────────────────────────────────────────────────
// Dedicated mpsc channel for sequential TTS playback, decoupled from event loop
static TTS_PLAYBACK_TX: OnceCell<tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>> =
    OnceCell::new();

fn get_tts_playback_tx() -> Option<&'static tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>> {
    TTS_PLAYBACK_TX.get()
}

// ── Voice State ──────────────────────────────────────────────────────────────

static DISCORD_VOICE_STATE: OnceCell<Arc<Mutex<DiscordVoiceState>>> = OnceCell::new();

#[derive(Default)]
pub struct DiscordVoiceState {
    pub guild_id: Option<u64>,
    pub user_id: Option<u64>,
}

pub fn init_discord_voice_state() -> Arc<Mutex<DiscordVoiceState>> {
    let state = Arc::new(Mutex::new(DiscordVoiceState::default()));
    DISCORD_VOICE_STATE.set(state.clone()).ok();
    state
}

pub fn get_discord_voice_state() -> Option<Arc<Mutex<DiscordVoiceState>>> {
    DISCORD_VOICE_STATE.get().cloned()
}

pub async fn set_discord_voice_state(guild_id: Option<u64>, user_id: Option<u64>) {
    if let Some(state) = DISCORD_VOICE_STATE.get() {
        let mut guard = state.lock().await;
        guard.guild_id = guild_id;
        guard.user_id = user_id;
    }
}

// ── Songbird Manager ─────────────────────────────────────────────────────────

#[cfg(feature = "songbird")]
static SONGBIRD_MANAGER: OnceCell<Arc<songbird::Songbird>> = OnceCell::new();

#[cfg(feature = "songbird")]
pub fn set_songbird_manager(manager: Arc<songbird::Songbird>) {
    SONGBIRD_MANAGER.set(manager).ok();
}

#[cfg(feature = "songbird")]
pub fn get_songbird_manager() -> Option<&'static Arc<songbird::Songbird>> {
    SONGBIRD_MANAGER.get()
}

// ── Discord Bot ──────────────────────────────────────────────────────────────

pub struct DiscordBot {
    pub host: Arc<dyn host::ChannelHost>,
    pub ws_client: Arc<Mutex<WsClient>>,
    pub credentials: host::DiscordCredentials,
}

impl DiscordBot {
    pub async fn new(
        host: Arc<dyn host::ChannelHost>,
        gateway_url: Option<&str>,
        credentials: host::DiscordCredentials,
    ) -> anyhow::Result<Self> {
        let url = gateway_url.unwrap_or(DEFAULT_GATEWAY_URL);
        let ws_client = WsClient::connect(url).await?;
        Ok(Self {
            host,
            ws_client: Arc::new(Mutex::new(ws_client)),
            credentials,
        })
    }

    pub async fn start(self) -> anyhow::Result<()> {
        // Read from env (.env file)
        let token = std::env::var("DISCORD_BOT_TOKEN").context(
            "DISCORD_BOT_TOKEN not set. Run 'praxis onboard --interactive' to configure.",
        )?;
        let application_id = std::env::var("DISCORD_APPLICATION_ID")
            .context("DISCORD_APPLICATION_ID not set")?
            .parse::<u64>()
            .context("DISCORD_APPLICATION_ID must be a number")?;

        tracing::info!(
            "Discord bot starting with token: {}...",
            &token[..std::cmp::min(10, token.len())]
        );
        self.start_with_token(&token, application_id).await
    }

    pub async fn start_with_token(self, token: &str, application_id: u64) -> anyhow::Result<()> {
        let _voice_state = init_discord_voice_state();
        let handler = DiscordHandler::new(
            self.host.clone(),
            self.ws_client.clone(),
            self.credentials.clone(),
        );

        let mut client_builder = Client::builder(
            token,
            GatewayIntents::GUILDS
                | GatewayIntents::MESSAGE_CONTENT
                | GatewayIntents::DIRECT_MESSAGES
                | GatewayIntents::DIRECT_MESSAGE_REACTIONS
                | GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::GUILD_MESSAGE_REACTIONS
                | GatewayIntents::GUILD_VOICE_STATES
                | GatewayIntents::GUILD_MEMBERS,
        )
        .application_id(serenity::all::ApplicationId::new(application_id))
        .event_handler(handler);

        // Register songbird for voice support
        #[cfg(feature = "songbird")]
        {
            use songbird::driver::{Channels, DecodeConfig, DecodeMode, SampleRate};
            use songbird::SerenityInit;
            let voice_config = songbird::Config::default().decode_mode(DecodeMode::Decode(
                DecodeConfig::new(Channels::Mono, SampleRate::Hz16000),
            ));
            client_builder = client_builder.register_songbird_from_config(voice_config);
        }

        let mut client = client_builder.await?;

        // Spawn event listener for file uploads, feedback, voice TTS
        let host = self.host.clone();
        let http = client.http.clone();
        tokio::spawn(async move {
            listen_for_events(host, http).await;
        });

        if let Err(e) = commands::setup_commands(&client.http).await {
            tracing::error!("Failed to setup commands: {}", e);
        }

        tracing::info!("Discord bot starting...");
        client.start().await?;
        Ok(())
    }
}

/// Listen for events from the event channel and handle them
async fn listen_for_events(host: Arc<dyn host::ChannelHost>, http: Arc<serenity::http::Http>) {
    // Wait a bit for the host event feed to be initialized
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let mut rx = host.events();

    // Initialize TTS playback channel and spawn dedicated playback task
    let (tts_tx, mut tts_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
    let _ = TTS_PLAYBACK_TX.set(tts_tx);
    tokio::spawn(async move {
        while let Some((user_id, audio_data)) = tts_rx.recv().await {
            play_tts_audio(user_id, audio_data).await;
        }
    });

    loop {
        match rx.recv().await {
            Ok(host::ChannelEvent::FileUpload {
                user_id,
                file_name,
                file_path,
                channel_id,
            }) => {
                tracing::info!(
                    "File upload event for user {}: {} to channel {}",
                    user_id,
                    file_name,
                    channel_id
                );
                let target_channel_id = if !channel_id.is_empty() {
                    channel_id.parse::<u64>().unwrap_or(0)
                } else {
                    0
                };

                let send_to = if target_channel_id > 0 {
                    serenity::model::id::ChannelId::new(target_channel_id)
                } else if let Ok(Some(pairing)) = host.pairing_by_user(&user_id) {
                    let discord_user_id = pairing.discord_user_id.parse::<u64>().unwrap_or(0);
                    if discord_user_id > 0 {
                        let user_id_obj = serenity::model::id::UserId::new(discord_user_id);
                        match user_id_obj.create_dm_channel(&http).await {
                            Ok(dm) => dm.id,
                            Err(e) => {
                                tracing::error!("Failed to create DM channel: {}", e);
                                continue;
                            }
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                let req = reqwest::Client::new();
                let file_bytes = tokio::fs::read(&file_path).await.unwrap_or_default();
                let part = reqwest::multipart::Part::bytes(file_bytes).file_name(file_name.clone());
                let form = reqwest::multipart::Form::new().part("file", part);

                let bot_token = std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default();
                let url = format!("https://discord.com/api/v10/channels/{}/messages", send_to);

                if let Ok(resp) = req
                    .post(&url)
                    .header("Authorization", format!("Bot {}", bot_token))
                    .multipart(form)
                    .send()
                    .await
                {
                    if resp.status().is_success() {
                        tracing::info!("File {} sent to channel {}", file_name, send_to);
                    } else {
                        tracing::error!(
                            "Failed to send file: {} - {}",
                            resp.status(),
                            resp.text().await.unwrap_or_default()
                        );
                    }
                }
            }
            Ok(host::ChannelEvent::AgentFeedback { user_id, message }) => {
                tracing::info!("Agent feedback for user {}", user_id);
                if let Ok(Some(pairing)) = host.pairing_by_user(&user_id) {
                    let discord_user_id = pairing.discord_user_id.parse::<u64>().unwrap_or(0);
                    if discord_user_id > 0 {
                        let user_id_obj = serenity::model::id::UserId::new(discord_user_id);
                        if let Ok(dm_channel) = user_id_obj.create_dm_channel(&http).await {
                            let _ = dm_channel
                                .send_message(
                                    &http,
                                    serenity::builder::CreateMessage::default()
                                        .content(format!("[Feedback] {}", message)),
                                )
                                .await;
                        }
                    }
                }
            }
            Ok(host::ChannelEvent::ChannelMessage {
                user_id: _,
                channel_id,
                message,
            }) => {
                let channel_idparsed = channel_id.parse::<u64>().unwrap_or(0);
                if channel_idparsed > 0 {
                    let channel = serenity::model::id::ChannelId::new(channel_idparsed);
                    let _ = channel.say(&http, &message).await;
                }
            }
            Ok(host::ChannelEvent::ChannelEmbed {
                user_id: _,
                channel_id,
                embed,
            }) => {
                let channel_idparsed = channel_id.parse::<u64>().unwrap_or(0);
                if channel_idparsed > 0 {
                    let channel = serenity::model::id::ChannelId::new(channel_idparsed);
                    let mut e = serenity::builder::CreateEmbed::new();
                    if let Some(title) = embed["title"].as_str() {
                        e = e.title(title);
                    }
                    if let Some(description) = embed["description"].as_str() {
                        e = e.description(description);
                    }
                    if let Some(url) = embed["url"].as_str() {
                        e = e.url(url);
                    }
                    if let Some(color) = embed["color"].as_u64() {
                        e = e.color(color as u32);
                    }
                    if let Some(footer) = embed["footer"].as_str() {
                        e = e.footer(serenity::builder::CreateEmbedFooter::new(footer));
                    }
                    if let Some(author) = embed["author"].as_str() {
                        e = e.author(serenity::builder::CreateEmbedAuthor::new(author));
                    }
                    if let Some(thumbnail) = embed["thumbnail"].as_str() {
                        e = e.thumbnail(thumbnail);
                    }
                    if let Some(image) = embed["image"].as_str() {
                        e = e.image(image);
                    }
                    for field in embed["fields"].as_array().into_iter().flatten() {
                        e = e.field(
                            field["name"].as_str().unwrap_or_default(),
                            field["value"].as_str().unwrap_or_default(),
                            field["inline"].as_bool().unwrap_or(false),
                        );
                    }
                    let msg = serenity::builder::CreateMessage::new().embed(e);
                    if let Err(e) = channel.send_message(&http, msg).await {
                        tracing::error!("Failed to send embed to channel {}: {}", channel_id, e);
                    }
                }
            }
            Ok(host::ChannelEvent::VoiceTts {
                user_id,
                audio_data,
            }) => {
                if let Some(tx) = get_tts_playback_tx() {
                    let _ = tx.send((user_id, audio_data));
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("Event channel error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}

/// Play TTS audio in Discord voice channel (called from dedicated playback task)
#[cfg(feature = "songbird")]
async fn play_tts_audio(user_id: String, audio_data: Vec<u8>) {
    tracing::info!("Voice TTS for user {}: {} bytes", user_id, audio_data.len());

    if let Some(voice_state) = get_discord_voice_state() {
        let guard = voice_state.lock().await;
        if let Some(guild_id) = guard.guild_id {
            drop(guard);
            if let Some(manager) = get_songbird_manager() {
                if let Some(handler) = manager.get(songbird::id::GuildId::from(
                    std::num::NonZeroU64::new(guild_id).unwrap(),
                )) {
                    match praxis_voice::tts::audio_bytes_to_pcm(&audio_data) {
                        Ok((samples, original_sample_rate)) if !samples.is_empty() => {
                            tracing::info!(
                                "TTS decoded: {} samples at {} Hz",
                                samples.len(),
                                original_sample_rate
                            );
                            let wav_data =
                                praxis_voice::pcm_to_wav(&samples, original_sample_rate, 1);
                            let duration_ms =
                                (samples.len() as u64 * 1000) / original_sample_rate as u64;
                            let source = songbird::input::Input::from(wav_data);
                            {
                                let mut call = handler.lock().await;
                                call.play_input(source);
                            }
                            tracing::info!(
                                "TTS audio playing in guild {} ({}ms)",
                                guild_id,
                                duration_ms
                            );
                            // Wait for estimated duration + small buffer
                            tokio::time::sleep(tokio::time::Duration::from_millis(
                                duration_ms + 200,
                            ))
                            .await;
                            tracing::info!("TTS audio finished in guild {}", guild_id);
                        }
                        Ok(_) => tracing::warn!("No audio samples"),
                        Err(e) => tracing::error!("Audio decode failed: {}", e),
                    }
                } else {
                    tracing::warn!(
                        "Voice TTS dropped: no songbird handler for guild {} (bot not in that voice channel)",
                        guild_id
                    );
                }
            } else {
                tracing::warn!("Voice TTS dropped: songbird manager not initialised");
            }
        } else {
            tracing::warn!(
                "Voice TTS dropped: bot/user not connected to a voice channel (guild_id is None)"
            );
        }
    } else {
        tracing::warn!("Voice TTS dropped: voice state not initialised");
    }
    // Next TTS from the mpsc channel will play after this returns
}

#[cfg(not(feature = "songbird"))]
async fn play_tts_audio(_user_id: String, _audio_data: Vec<u8>) {
    tracing::debug!("TTS playback skipped - songbird not enabled");
}

/// Start the channel against a host. Credentials are scoped: the caller
/// passes only what this channel declares.
pub async fn start_with_host(
    host: Arc<dyn host::ChannelHost>,
    credentials: host::DiscordCredentials,
) -> anyhow::Result<()> {
    let gateway_url = std::env::var("GATEWAY_WS_URL")
        .map(|s| s.into())
        .unwrap_or_else(|_| DEFAULT_GATEWAY_URL.to_string());

    let token = credentials
        .bot_token
        .clone()
        .filter(|token| !token.trim().is_empty())
        .or_else(|| std::env::var("DISCORD_BOT_TOKEN").ok())
        .context("DISCORD_BOT_TOKEN not set. Run 'praxis onboard --interactive' to configure.")?;

    let application_id = std::env::var("DISCORD_APPLICATION_ID")
        .context("DISCORD_APPLICATION_ID not set")?
        .parse::<u64>()
        .context("DISCORD_APPLICATION_ID must be a number")?;

    let bot = DiscordBot::new(host, Some(&gateway_url), credentials).await?;
    tracing::info!("Discord bot connecting...");
    bot.start_with_token(&token, application_id).await
}

#[cfg(test)]
mod discord_tests {
    use super::*;

    #[tokio::test]
    async fn test_default_gateway_url() {
        assert_eq!(DEFAULT_GATEWAY_URL, "ws://localhost:3537/ws");
    }

    #[tokio::test]
    async fn test_outgoing_message_serialization() {
        let msg = OutgoingMessage::Message {
            user_id: "user123".to_string(),
            content: "Hello".to_string(),
            channel_id: "ch1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        assert!(json.contains("user123"));
    }

    #[tokio::test]
    async fn test_incoming_message_deserialization() {
        let json = r#"{"type":"response","user_id":"user123","content":"Hi there"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Response { user_id, content } => {
                assert_eq!(user_id, "user123");
                assert_eq!(content, "Hi there");
            }
            _ => panic!("Expected Response variant"),
        }
    }

    #[test]
    fn test_discord_voice_state_default() {
        let state = DiscordVoiceState::default();
        assert!(state.guild_id.is_none());
        assert!(state.user_id.is_none());
    }
}
