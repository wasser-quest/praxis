use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type WsStreamPart = futures_util::stream::SplitStream<WsStream>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    #[serde(rename = "response")]
    Response { user_id: String, content: String },
    #[serde(rename = "feedback")]
    Feedback { user_id: String, content: String },
    #[serde(rename = "reasoning")]
    Reasoning { user_id: String, content: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "discord.file.upload")]
    DiscordFileUpload {
        user_id: String,
        filename: String,
        file_path: String,
    },
    #[serde(rename = "event")]
    Event {
        event: String,
        payload: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "message")]
    Message {
        user_id: String,
        content: String,
        channel_id: String,
    },
    #[serde(rename = "compact")]
    Compact { user_id: String },
    #[serde(rename = "voice_input_started")]
    VoiceInputStarted { user_id: String },
}

pub struct WsClient {
    sender: Arc<Mutex<WsSink>>,
    receiver: Arc<Mutex<WsStreamPart>>,
    url: String,
}

impl WsClient {
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with_retries(url, 10, std::time::Duration::from_millis(500)).await
    }

    pub async fn connect_with_retries(
        url: &str,
        max_retries: u32,
        base_delay: std::time::Duration,
    ) -> Result<Self> {
        let mut last_err = None;
        for attempt in 0..max_retries {
            match connect_async(url).await {
                Ok((ws_stream, _)) => {
                    let (sender, receiver) = ws_stream.split();
                    return Ok(Self {
                        sender: Arc::new(Mutex::new(sender)),
                        receiver: Arc::new(Mutex::new(receiver)),
                        url: url.to_string(),
                    });
                }
                Err(e) => {
                    if attempt == 0 {
                        tracing::info!("Gateway not ready yet, waiting for it to start...");
                    }
                    last_err = Some(e);
                    let delay = base_delay * 2u32.pow(attempt.min(4));
                    tokio::time::sleep(delay).await;
                }
            }
        }
        match last_err {
            Some(e) => Err(e.into()),
            None => Err(anyhow::anyhow!(
                "Connection failed after {} retries",
                max_retries
            )),
        }
    }

    pub async fn send(&self, msg: OutgoingMessage) -> Result<()> {
        let text = serde_json::to_string(&msg)?;
        let mut sender = self.sender.lock().await;
        sender.send(Message::Text(text)).await?;
        Ok(())
    }

    pub async fn recv(&self) -> Result<IncomingMessage> {
        let mut receiver = self.receiver.lock().await;
        if let Some(msg) = receiver.next().await {
            let msg = msg?;
            if let Message::Text(text) = msg {
                let incoming: IncomingMessage = serde_json::from_str(&text)?;
                return Ok(incoming);
            }
        }
        Err(anyhow::anyhow!("No message received"))
    }

    pub async fn send_and_recv_until_response<F>(
        &self,
        msg: OutgoingMessage,
        mut on_feedback: F,
    ) -> Result<Result<String, String>>
    where
        F: FnMut(&str),
    {
        {
            let text = serde_json::to_string(&msg)?;
            let mut sender = self.sender.lock().await;
            sender.send(Message::Text(text)).await?;
        }

        loop {
            let incoming = {
                let mut receiver = self.receiver.lock().await;
                match receiver.next().await {
                    Some(Ok(Message::Text(text))) => {
                        serde_json::from_str::<IncomingMessage>(&text)?
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => return Err(anyhow::anyhow!("WebSocket connection closed")),
                    _ => continue,
                }
            };

            match incoming {
                IncomingMessage::Response { content, .. } => {
                    return Ok(Ok(content));
                }
                IncomingMessage::Feedback { content, .. } => {
                    on_feedback(&content);
                }
                // Voice/compaction convenience path consumes final answers only.
                // Discord text delivery handles reasoning as a separate message.
                IncomingMessage::Reasoning { .. } => {}
                IncomingMessage::Error { message } => {
                    return Ok(Err(message));
                }
                IncomingMessage::Event { event, .. } => {
                    tracing::debug!("recv-loop: ignoring event: {}", event);
                }
                IncomingMessage::Pong => {
                    tracing::debug!("recv-loop: pong");
                }
                IncomingMessage::DiscordFileUpload { .. } => {
                    tracing::debug!("recv-loop: ignoring file upload");
                }
            }
        }
    }

    pub async fn close(self) -> Result<()> {
        let mut sender = self.sender.lock().await;
        sender.close().await?;
        Ok(())
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(test)]
mod discord_tests {
    use super::*;

    #[test]
    fn test_outgoing_message_serialization() {
        let msg = OutgoingMessage::Message {
            user_id: "user123".to_string(),
            content: "Hello".to_string(),
            channel_id: "ch1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        assert!(json.contains("user123"));
    }

    #[test]
    fn test_incoming_message_deserialization() {
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
    fn test_incoming_feedback_deserialization() {
        let json = r#"{"type":"feedback","user_id":"u1","content":"thinking..."}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Feedback { user_id, content } => {
                assert_eq!(user_id, "u1");
                assert_eq!(content, "thinking...");
            }
            _ => panic!("Expected Feedback variant"),
        }
    }

    #[test]
    fn test_incoming_error_deserialization() {
        let json = r#"{"type":"error","message":"something went wrong"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Error { message } => {
                assert_eq!(message, "something went wrong");
            }
            _ => panic!("Expected Error variant"),
        }
    }

    #[tokio::test]
    async fn test_ws_client_connect_invalid_url() {
        let result = WsClient::connect("ws://localhost:99999/nonexistent").await;
        assert!(result.is_err());
    }
}
