use crate::gateway::GatewayState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

#[path = "ws_task.rs"]
mod task;
#[cfg(test)]
#[path = "ws_task_tests.rs"]
mod task_tests;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsIncoming {
    #[serde(rename = "message")]
    Message {
        user_id: String,
        content: String,
        #[allow(dead_code)]
        channel_id: Option<String>,
    },
    #[serde(rename = "agent_input")]
    AgentInput {
        user_id: String,
        content: String,
    },
    #[serde(rename = "compact")]
    Compact { user_id: String },
    #[serde(rename = "stop")]
    Stop { user_id: String },
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum WsOutgoing {
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
    #[serde(rename = "voice_input_started")]
    #[allow(dead_code)]
    VoiceInputStarted { user_id: String },
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<GatewayState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: GatewayState) {
    // GPU-Session-Hook (pgpu §12.4): Chat/Dashboard geöffnet → Slots
    // vorwärmen. LLM-Kaltstart bis 45 min — der Wake soll beim Öffnen der
    // Session laufen, nicht erst im ersten Turn (X-Router-Wait fängt nur
    // bis 600 s). Fire-and-forget, dedupliziert (30 s/Slot).
    if super::inference::readiness(&state).ready {
        crate::gpu_router::wake_slots_for_session();
    }

    let (mut sender, mut receiver) = socket.split();

    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => {
                tracing::warn!("WebSocket error: {}", e);
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                tracing::debug!("Received WS text: {}", text);

                let incoming: WsIncoming = match serde_json::from_str(&text) {
                    Ok(msg) => msg,
                    Err(e) => {
                        tracing::warn!("Failed to parse WS message: {}", e);
                        let err = WsOutgoing::Error {
                            message: format!("Invalid message format: {}", e),
                        };
                        let _ = sender
                            .send(Message::Text(serde_json::to_string(&err).unwrap()))
                            .await;
                        continue;
                    }
                };

                match incoming {
                    WsIncoming::Message {
                        user_id,
                        content,
                        channel_id,
                    } => {
                        let feedback = WsOutgoing::Feedback {
                            user_id: user_id.clone(),
                            content: "Thinking...".to_string(),
                        };
                        let _ = sender
                            .send(Message::Text(serde_json::to_string(&feedback).unwrap()))
                            .await;

                        match task::handle(
                            &state,
                            &user_id,
                            &content,
                            channel_id.as_deref(),
                            &mut sender,
                            &mut receiver,
                        )
                        .await
                        {
                            Ok(reply) => {
                                let response = WsOutgoing::Response {
                                    user_id,
                                    content: reply,
                                };
                                if sender
                                    .send(Message::Text(serde_json::to_string(&response).unwrap()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::error!("Message handler error: {}", e);
                                let err = WsOutgoing::Error {
                                    message: format!("Error: {}", e),
                                };
                                let _ = sender
                                    .send(Message::Text(serde_json::to_string(&err).unwrap()))
                                    .await;
                            }
                        }
                    }
                    WsIncoming::Compact { user_id } => {
                        match compact_user_history(&state, &user_id).await {
                            Ok(summary) => {
                                let response = WsOutgoing::Response {
                                    user_id,
                                    content: format!(
                                        "Conversation compacted. Summary: {}",
                                        summary
                                    ),
                                };
                                let _ = sender
                                    .send(Message::Text(serde_json::to_string(&response).unwrap()))
                                    .await;
                            }
                            Err(e) => {
                                tracing::error!("Compaction error: {}", e);
                                let err = WsOutgoing::Error {
                                    message: format!("Compaction failed: {}", e),
                                };
                                let _ = sender
                                    .send(Message::Text(serde_json::to_string(&err).unwrap()))
                                    .await;
                            }
                        }
                    }
                    WsIncoming::AgentInput { user_id, content } => {
                        match crate::gateway::agent_loop::get_user_input_sender(&user_id).await {
                            Some(input_sender) => {
                                if let Err(e) = input_sender.send(content.clone()) {
                                    let err = WsOutgoing::Error {
                                        message: format!("Failed to send input: {}", e),
                                    };
                                    let _ = sender
                                        .send(Message::Text(serde_json::to_string(&err).unwrap()))
                                        .await;
                                } else {
                                    let response = WsOutgoing::Feedback {
                                        user_id,
                                        content: format!("Input sent: {}", content),
                                    };
                                    let _ = sender
                                        .send(Message::Text(serde_json::to_string(&response).unwrap()))
                                        .await;
                                }
                            }
                            None => {
                                let err = WsOutgoing::Error {
                                    message: "No active agent loop found for this user".to_string(),
                                };
                                let _ = sender
                                    .send(Message::Text(serde_json::to_string(&err).unwrap()))
                                    .await;
                            }
                        }
                    }
                    WsIncoming::Stop { user_id } => {
                        crate::gateway::agent_loop::stop_agent_loop(&user_id).await;
                    }
                    WsIncoming::Ping => {
                        let pong = WsOutgoing::Pong;
                        let _ = sender
                            .send(Message::Text(serde_json::to_string(&pong).unwrap()))
                            .await;
                    }
                }
            }
            Message::Binary(_) => {
                tracing::debug!("Received binary data");
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

/// Compact the conversation history for a user: generate a summary, store it
/// in the context and replace the message history with the recent (kept)
/// rows. Shared by the WebSocket `/compact` command and the dashboard
/// POST /api/messages/:user_id/compact endpoint.
pub async fn compact_history(
    state: &crate::gateway::GatewayState,
    user_id: &str,
) -> anyhow::Result<String> {
    let _task = super::task_control::begin(user_id)?;
    let ctx = state.db.load_context(user_id)?;
    let (messages, _) = state.db.get_messages_with_token_budget(user_id, usize::MAX)?;
    let Some(last_user) = messages.iter().rposition(|m| m.role == "user").filter(|index| *index > 0) else {
        anyhow::bail!("No older complete turns to compact; the current turn is preserved");
    };
    super::safe_web_search::prepare(&state.db, &state.plugins, std::path::Path::new(&state.config.root_dir),
        user_id, "Summarize earlier conversation turns without executing tools").await?;
    let prefix = &messages[..last_user];
    let summary = super::compaction::summarize(state, user_id, prefix,
        &ctx.settings.compaction_summary, ctx.settings.compaction_template.as_deref()).await?;
    state.db.commit_compaction(user_id, &ctx.session_id, &ctx.settings.compaction_summary, &summary, prefix)?;
    Ok(summary)
}

async fn compact_user_history(state: &GatewayState, user_id: &str) -> anyhow::Result<String> {
    compact_history(state, user_id).await
}

#[cfg(test)]
mod gateway_tests {
    #[test]
    fn test_ws_handler_compiles() {
        assert!(true);
    }
}
