use crate::gateway::GatewayState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub uptime_secs: u64,
    pub version: String,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    pub llm_providers: Vec<String>,
    pub default_provider: String,
    pub inference: super::inference::Readiness,
}

#[derive(serde::Deserialize)]
pub struct ChatRequest {
    user_id: String,
    message: String,
}

#[derive(serde::Serialize)]
pub struct ChatResponse {
    pub success: bool,
    pub response: Option<String>,
    pub error: Option<String>,
}

pub async fn health_check(State(state): State<GatewayState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

pub async fn status(State(state): State<GatewayState>) -> Json<StatusResponse> {
    let providers: Vec<String> = state.llm.provider_names();
    let default_provider = state.llm.default_provider();

    Json(StatusResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        llm_providers: providers,
        default_provider,
        inference: super::inference::readiness(&state),
    })
}

pub async fn stop(axum::extract::Path(user): axum::extract::Path<String>) -> Json<serde_json::Value> {
    crate::gateway::agent_loop::stop_agent_loop(&user).await;
    Json(serde_json::json!({"success":true}))
}

/// Authenticated SSE for local/TUI clients. Uses the same real-time bus as web chat.
pub async fn events(
    axum::extract::Path(user): axum::extract::Path<String>,
) -> axum::response::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = crate::runtime::events::get_or_create(&user).subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok(axum::response::sse::Event::default().event(event.event).data(data)), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    return Some((Ok(axum::response::sse::Event::default().event("stream_abort")
                        .data("{\"event\":\"stream_abort\",\"data\":\"Stream receiver lagged; reload saved history\"}")), rx));
                }
                Err(_) => return None,
            }
        }
    });
    axum::response::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// REST endpoint for web chat - same as WebSocket but via HTTP POST
pub async fn chat_handler(
    State(state): State<GatewayState>,
    Json(req): Json<ChatRequest>,
) -> (axum::http::StatusCode, Json<ChatResponse>) {
    tracing::info!(user_id = %req.user_id, "[GATEWAY] Web chat message received");

    match crate::gateway::message_handler::handle_message(
        &state,
        &req.user_id,
        &req.message,
        Some("web"),
    )
    .await
    {
        Ok(reply) => {
            tracing::info!(user_id = %req.user_id, reply_len = reply.len(), "[GATEWAY] handle_message returned OK");
            (axum::http::StatusCode::OK, Json(ChatResponse {
                success: true,
                response: Some(reply),
                error: None,
            }))
        },
        Err(e) => {
            tracing::warn!(user_id = %req.user_id, "[GATEWAY] handle_message failed: {}", e);
            let status = if e.downcast_ref::<super::inference::SetupError>().is_some() {
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            } else { axum::http::StatusCode::OK };
            (status, Json(ChatResponse {
                success: false,
                response: None,
                error: Some(e.to_string()),
            }))
        },
    }
}
