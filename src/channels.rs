//! The kernel's `ChannelHost` implementation: pairing, context, sessions,
//! history, agent control, delivery events and skills for channel frontends.
//!
//! Authority stays here — the channel crate receives only what the seam
//! exposes and can neither read the database nor grant itself anything.
use crate::discord::host::{
    ChannelEvent, ChannelHost, ContextJson, MirroredMessage, Pairing, SkillPage, SkillSummary,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::sync::{broadcast, mpsc};

pub struct KernelChannelHost {
    db: crate::db::Database,
    root: PathBuf,
    skills: crate::skills::SkillRegistry,
}

impl KernelChannelHost {
    pub fn new(db: crate::db::Database, root: PathBuf) -> anyhow::Result<Self> {
        let mut skills = crate::skills::SkillRegistry::new();
        skills.load_from_dir(&root.join("skills")).ok();
        Ok(Self { db, root, skills })
    }
}

#[async_trait]
impl ChannelHost for KernelChannelHost {
    fn pairing_by_discord(&self, discord_user: &str) -> anyhow::Result<Option<Pairing>> {
        Ok(self
            .db
            .get_pairing_by_discord(discord_user)?
            .map(|pairing| Pairing {
                user_id: pairing.user_id,
                discord_user_id: pairing.discord_user_id,
            }))
    }

    fn pairing_by_user(&self, user: &str) -> anyhow::Result<Option<Pairing>> {
        Ok(self
            .db
            .get_pairing_by_internal_user(user)?
            .map(|pairing| Pairing {
                user_id: pairing.user_id,
                discord_user_id: pairing.discord_user_id,
            }))
    }

    fn list_pairings(&self) -> anyhow::Result<Vec<Pairing>> {
        Ok(self
            .db
            .list_all_pairings()?
            .into_iter()
            .map(|pairing| Pairing {
                user_id: pairing.user_id,
                discord_user_id: pairing.discord_user_id,
            })
            .collect())
    }

    fn create_pending_pairing(
        &self,
        code: &str,
        discord_user: &str,
        expires_at: &str,
    ) -> anyhow::Result<()> {
        self.db.create_pending_pairing(code, discord_user, expires_at)
    }

    fn create_pairing(
        &self,
        user: &str,
        discord_user: &str,
        guild: Option<&str>,
    ) -> anyhow::Result<()> {
        self.db.create_pairing(user, discord_user, guild)
    }

    fn load_context(&self, user: &str) -> anyhow::Result<ContextJson> {
        Ok(serde_json::to_value(self.db.load_context(user)?)?)
    }

    fn save_context(&self, _user: &str, context: &ContextJson) -> anyhow::Result<()> {
        self.db
            .save_context(&serde_json::from_value(context.clone())?)
    }

    fn merge_context(&self, user: &str, updates: Value) -> anyhow::Result<ContextJson> {
        Ok(serde_json::to_value(self.db.merge_context(user, updates)?)?)
    }

    fn list_sessions(&self, user: &str) -> anyhow::Result<Vec<(String, String)>> {
        self.db.list_sessions(user)
    }

    fn create_session(&self, user: &str, session: &str, name: Option<&str>) -> anyhow::Result<()> {
        self.db.create_session(user, session, name)
    }

    fn rename_session(&self, user: &str, session: &str, name: &str) -> anyhow::Result<()> {
        self.db.rename_session(user, session, name)
    }

    fn delete_session(&self, user: &str, session: &str) -> anyhow::Result<()> {
        self.db.delete_session(user, session)
    }

    fn context_command(&self, user: &str, line: &str) -> Result<String, String> {
        crate::context_cmd::parse(line)
            .map_err(|error| error.to_string())
            .map(|operation| crate::context_cmd::apply(&self.db, user, &operation))
    }

    fn mirror_message(&self, message: MirroredMessage) -> anyhow::Result<()> {
        let stored = crate::db::messages::Message::discord_mirror(
            message.content,
            message.direction,
            &message.author,
            &message.channel_id,
            message.channel_kind,
        );
        self.db.add_message(&message.user_id, &stored)?;
        Ok(())
    }

    fn clear_messages(&self, user: &str) -> anyhow::Result<()> {
        self.db.clear_messages(user)
    }

    fn clear_session_messages(&self, user: &str, session: &str) -> anyhow::Result<()> {
        self.db.clear_session_messages(user, session)
    }

    async fn stop_agent(&self, user: &str) {
        crate::gateway::agent_loop::stop_agent_loop(user).await;
    }

    async fn question_sender(&self, user: &str) -> Option<mpsc::UnboundedSender<String>> {
        crate::gateway::agent_loop::get_user_input_sender(user).await
    }

    async fn interaction_reply(&self, channel: &str, content: &str, user: &str) -> bool {
        crate::tools::discord_interactive::handle_message_reply(channel, content, user).await
    }

    async fn interaction_reaction(&self, channel: &str, emoji: &str, user: &str) {
        crate::tools::discord_interactive::handle_reaction(channel, emoji, user).await;
    }

    fn events(&self) -> broadcast::Receiver<ChannelEvent> {
        // The channel feed is the kernel's gateway event stream, narrowed to
        // the events a channel delivers. A missing channel binding fails
        // closed: nothing is delivered.
        let (tx, rx) = broadcast::channel(256);
        if let Some(upstream) = crate::event_channel::get_event_tx() {
            let mut upstream = upstream.subscribe();
            tokio::spawn(async move {
                loop {
                    let event = match upstream.recv().await {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let event = match event {
                        crate::event_channel::GatewayEvent::FileUpload {
                            user_id,
                            file_path,
                            file_name,
                            channel_id,
                        } => ChannelEvent::FileUpload {
                            user_id,
                            file_path,
                            file_name,
                            channel_id,
                        },
                        crate::event_channel::GatewayEvent::AgentFeedback { user_id, message } => {
                            ChannelEvent::AgentFeedback { user_id, message }
                        }
                        crate::event_channel::GatewayEvent::ChannelMessage {
                            user_id,
                            channel_id,
                            message,
                        } => ChannelEvent::ChannelMessage {
                            user_id,
                            channel_id,
                            message,
                        },
                        crate::event_channel::GatewayEvent::ChannelEmbed {
                            user_id,
                            channel_id,
                            embed,
                        } => ChannelEvent::ChannelEmbed {
                            user_id,
                            channel_id,
                            embed: serde_json::to_value(embed).unwrap_or_default(),
                        },
                        crate::event_channel::GatewayEvent::VoiceTts {
                            user_id,
                            audio_data,
                        } => ChannelEvent::VoiceTts {
                            user_id,
                            audio_data,
                        },
                        _ => continue,
                    };
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            });
        }
        rx
    }

    fn dashboard_event(&self, user: &str, kind: &str, payload: &str) {
        crate::runtime::events::send(user, kind, payload);
    }

    fn tool_enabled(&self, name: &str) -> anyhow::Result<bool> {
        crate::db::tools::tool_enabled(&self.db, name)
    }

    fn skill_search(&self, directory: &Path, query: &str) -> anyhow::Result<SkillPage> {
        let mut index = crate::skills::SkillIndex::open(&self.db.data_dir(), directory)?;
        index.ensure_indexed()?;
        let results = index.search(query, 8, true)?;
        Ok(skill_page(results.skills, results.next_after))
    }

    fn skill_browse(&self, directory: &Path, cursor: &str) -> anyhow::Result<SkillPage> {
        let mut index = crate::skills::SkillIndex::open(&self.db.data_dir(), directory)?;
        index.ensure_indexed()?;
        let results = index.browse(cursor, 8, true)?;
        Ok(skill_page(results.skills, results.next_after))
    }

    fn registered_skills(&self) -> Vec<SkillSummary> {
        self.skills
            .list()
            .into_iter()
            .map(|skill| SkillSummary {
                name: skill.name.clone(),
                description: skill.description.clone(),
                version: skill.version.clone(),
                skill_hidden: skill.skill_hidden,
                user_only: skill.user_only,
            })
            .collect()
    }

    fn registered_skill(&self, name: &str) -> bool {
        self.skills.get(name).is_some()
    }

    fn skill_exists(&self, directory: &Path, name: &str) -> anyhow::Result<()> {
        crate::skills::lookup_skill(&self.db, directory, name)?;
        Ok(())
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

fn skill_page(skills: Vec<crate::skills::SkillSummary>, next_after: Option<String>) -> SkillPage {
    SkillPage {
        skills: skills
            .into_iter()
            .map(|skill| SkillSummary {
                name: skill.name,
                description: skill.description,
                version: skill.version,
                skill_hidden: skill.skill_hidden,
                user_only: skill.user_only,
            })
            .collect(),
        next_after,
    }
}
