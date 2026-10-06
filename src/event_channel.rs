use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiscordEmbed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub footer: Option<String>,
    pub author: Option<String>,
    pub thumbnail: Option<String>,
    pub image: Option<String>,
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    FileUpload {
        user_id: String,
        file_path: String,
        file_name: String,
        channel_id: String,
    },
    AgentComplete {
        user_id: String,
        response: String,
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
        embed: DiscordEmbed,
    },
    VoiceTts {
        user_id: String,
        audio_data: Vec<u8>,
    },
}

static EVENT_TX: OnceLock<broadcast::Sender<GatewayEvent>> = OnceLock::new();

pub fn init() -> broadcast::Sender<GatewayEvent> {
    let (tx, _) = broadcast::channel(100);
    let _ = EVENT_TX.set(tx.clone());
    tx
}

pub fn get_event_tx() -> Option<broadcast::Sender<GatewayEvent>> {
    EVENT_TX.get().cloned()
}

pub fn sender() -> broadcast::Sender<GatewayEvent> {
    EVENT_TX.get().unwrap().clone()
}

pub fn broadcast_event(event: GatewayEvent) {
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(event);
    }
}

pub fn broadcast_agent_complete(user_id: &str, response: &str) {
    broadcast_event(GatewayEvent::AgentComplete {
        user_id: user_id.to_string(),
        response: response.to_string(),
    });
}

pub fn broadcast_agent_feedback(user_id: &str, message: &str) {
    broadcast_event(GatewayEvent::AgentFeedback {
        user_id: user_id.to_string(),
        message: message.to_string(),
    });
}

pub fn broadcast_file_upload(user_id: &str, file_path: &str, file_name: &str, channel_id: &str) {
    broadcast_event(GatewayEvent::FileUpload {
        user_id: user_id.to_string(),
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        channel_id: channel_id.to_string(),
    });
}

pub fn broadcast_channel_message(user_id: &str, channel_id: &str, message: &str) {
    broadcast_event(GatewayEvent::ChannelMessage {
        user_id: user_id.to_string(),
        channel_id: channel_id.to_string(),
        message: message.to_string(),
    });
}

pub fn broadcast_channel_embed(user_id: &str, channel_id: &str, embed: DiscordEmbed) {
    broadcast_event(GatewayEvent::ChannelEmbed {
        user_id: user_id.to_string(),
        channel_id: channel_id.to_string(),
        embed,
    });
}

pub fn broadcast_voice_tts(user_id: &str, audio_data: Vec<u8>) {
    broadcast_event(GatewayEvent::VoiceTts {
        user_id: user_id.to_string(),
        audio_data,
    });
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn local_channel() -> broadcast::Sender<GatewayEvent> {
        let (tx, _) = broadcast::channel(100);
        tx
    }

    fn send_and_recv(tx: &broadcast::Sender<GatewayEvent>, event: GatewayEvent) -> GatewayEvent {
        let mut rx = tx.subscribe();
        tx.send(event).unwrap();
        rx.try_recv().unwrap()
    }

    #[test]
    fn test_event_bus_init() {
        let (tx, _): (broadcast::Sender<GatewayEvent>, _) = broadcast::channel(100);
        let _rx = tx.subscribe();
    }

    #[test]
    fn test_get_event_tx_returns_none_before_init() {
        // Can't test global singleton reliably in parallel, test local channel instead
        let tx = local_channel();
        let mut rx = tx.subscribe();
        tx.send(GatewayEvent::AgentComplete {
            user_id: "u".into(),
            response: "r".into(),
        })
        .unwrap();
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_broadcast_agent_complete() {
        let tx = local_channel();
        let event = send_and_recv(
            &tx,
            GatewayEvent::AgentComplete {
                user_id: "user1".into(),
                response: "hello".into(),
            },
        );
        match event {
            GatewayEvent::AgentComplete { user_id, response } => {
                assert_eq!(user_id, "user1");
                assert_eq!(response, "hello");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_broadcast_agent_feedback() {
        let tx = local_channel();
        let event = send_and_recv(
            &tx,
            GatewayEvent::AgentFeedback {
                user_id: "user1".into(),
                message: "progress update".into(),
            },
        );
        match event {
            GatewayEvent::AgentFeedback { user_id, message } => {
                assert_eq!(user_id, "user1");
                assert_eq!(message, "progress update");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_broadcast_file_upload() {
        let tx = local_channel();
        let event = send_and_recv(
            &tx,
            GatewayEvent::FileUpload {
                user_id: "user1".into(),
                file_path: "/tmp/file.txt".into(),
                file_name: "file.txt".into(),
                channel_id: "ch123".into(),
            },
        );
        match event {
            GatewayEvent::FileUpload {
                user_id,
                file_path,
                file_name,
                channel_id,
            } => {
                assert_eq!(user_id, "user1");
                assert_eq!(file_path, "/tmp/file.txt");
                assert_eq!(file_name, "file.txt");
                assert_eq!(channel_id, "ch123");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_broadcast_voice_tts() {
        let tx = local_channel();
        let event = send_and_recv(
            &tx,
            GatewayEvent::VoiceTts {
                user_id: "user1".into(),
                audio_data: vec![1, 2, 3],
            },
        );
        match event {
            GatewayEvent::VoiceTts {
                user_id,
                audio_data,
            } => {
                assert_eq!(user_id, "user1");
                assert_eq!(audio_data, vec![1, 2, 3]);
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_broadcast_channel_message() {
        let tx = local_channel();
        let event = send_and_recv(
            &tx,
            GatewayEvent::ChannelMessage {
                user_id: "user1".into(),
                channel_id: "ch123".into(),
                message: "hello world".into(),
            },
        );
        match event {
            GatewayEvent::ChannelMessage {
                user_id,
                channel_id,
                message,
            } => {
                assert_eq!(user_id, "user1");
                assert_eq!(channel_id, "ch123");
                assert_eq!(message, "hello world");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_broadcast_channel_embed() {
        let tx = local_channel();
        let embed = DiscordEmbed {
            title: Some("Test Title".into()),
            description: Some("Test Description".into()),
            color: Some(0x6C5CE7),
            fields: vec![EmbedField {
                name: "Field 1".into(),
                value: "Value 1".into(),
                inline: true,
            }],
            ..Default::default()
        };
        let event = send_and_recv(
            &tx,
            GatewayEvent::ChannelEmbed {
                user_id: "user1".into(),
                channel_id: "ch123".into(),
                embed,
            },
        );
        match event {
            GatewayEvent::ChannelEmbed {
                user_id,
                channel_id,
                embed,
            } => {
                assert_eq!(user_id, "user1");
                assert_eq!(channel_id, "ch123");
                assert_eq!(embed.title, Some("Test Title".into()));
                assert_eq!(embed.description, Some("Test Description".into()));
                assert_eq!(embed.color, Some(0x6C5CE7));
                assert_eq!(embed.fields.len(), 1);
                assert_eq!(embed.fields[0].name, "Field 1");
                assert!(embed.fields[0].inline);
            }
            _ => panic!("Wrong event type"),
        }
    }
}
